//! `rvr press`: read what a study's samples.lock names through its recipe.toml, keep the events
//! the recipe's cuts pass, and write one `.rvr` file that carries all three texts.
//!
//! Documentation: `FORMAT.md` (repository root) ("made", "data"); the README of rvr.
//! ROOT is read with hep-root (`RootFile::read_range`). Every input is checked against the lock
//! before it is read (size, adler32, ROOT UUID, entries). Rows keep the order of their entries,
//! and several files are read at once, each a run of entries at a time. A random choice reads
//! only the runs that hold a chosen entry.

use crate::holes::{fault_of, Failed, Fault, Seen};
use crate::samples::copies::{Datasets, Reader};
use crate::samples::remote::{
    digest_seen, patiently, read_again, HttpsFile, Refusal, Storage, Watch, READING, STOPPED,
};
use crate::samples::resolve::{self, adler32_file, File, Lock, Locked};
use crate::samples::spec::Events;
use crate::samples::{some, Choice};
use grid_catalog::GridHttps;
use hep_root::{ByteSource, RootFile, Tree};
use rowver::write::{self, Labels};
use rowver::NumericBuffer;
use rucio_client::{Rucio, Server};
use rvr_recipe::{Array, Columns, Constant, Expression, Kind, Recipe, Scope, Values};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// How many entries of a file are read and held at a time.
const CHUNK: u64 = 50_000;

/// How many bytes of kept values a file gathers before they go to the spool on disk.
const SPOOLED: usize = 16 << 20;

/// In a local file, chosen entries further apart than this are read as runs of their own, so a
/// sparse random choice reads the baskets near its entries rather than every basket. A remote
/// file is cut only at a gap of a whole `CHUNK`: one more request costs about what 10 MB of
/// baskets do (measured on CMS Open Data, 2026-10-18).
const GAP: u64 = 2_000;

/// The recipe reference, which lists every function an expression can call.
const RECIPES: &str = "https://gitlab.cern.ch/dgrove/rowver/-/blob/main/crates/rvr-recipe/README.md";

/// The runs of entries a press reads of entries `[from, to)` of a file, as [first, last): every
/// `CHUNK` entries, or for a random choice runs around its chosen entries (those in the range),
/// cut where two are `gap` or more apart.
fn runs(from: u64, to: u64, chosen: Option<&[u64]>, gap: u64) -> Vec<(u64, u64)> {
    let Some(chosen) = chosen else {
        let starts = (from..to).step_by(CHUNK as usize);
        return starts.map(|first| (first, to.min(first + CHUNK))).collect();
    };
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for &e in chosen {
        match runs.last_mut() {
            Some((first, last)) if e - *last < gap && e + 1 - *first <= CHUNK => *last = e + 1,
            _ => runs.push((e, e + 1)),
        }
    }
    runs
}

pub const USAGE: &str = "usage:
  rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N] [--full]
            [--fill [KINDS]] [--restart]
                                           press DIR/samples.lock through DIR/recipe.toml into
                                           DIR/<DIR's name>.rvr (DIR is . when not given); when
                                           only stored columns changed since that file was
                                           pressed, read just those (--full reads everything);
                                           --fill reads only the file's holes (of KINDS, such as
                                           tape,unreachable) and writes it again with them; a
                                           press that stopped part way is picked up where it
                                           stopped (--restart starts it over)";

/// `rvr press …` with the words after `press`: the exit status, 3 when the file at the target has
/// holes (whether or not this press wrote it).
pub fn main(args: &[&str]) -> u8 {
    match run(args) {
        Ok(0) => 0,
        Ok(_) => 3,
        Err(why) => {
            eprintln!("rvr press: {why}");
            1
        }
    }
}

/// Press as the words ask: how many holes the file has.
fn run(args: &[&str]) -> Result<usize, String> {
    let (mut dir, mut out, mut workers, mut full, mut fill) = (None, None, None, false, None);
    let mut restart = false;
    let mut choice = Choice::default();
    let mut words = args.iter();
    while let Some(&word) = words.next() {
        let mut given = || {
            words
                .next()
                .copied()
                .filter(|v| !v.starts_with("--"))
                .ok_or(format!("{word} needs a value"))
        };
        match word {
            "--events" => choice.events = Some(given()?),
            "--random" => choice.random = true,
            "--seed" => choice.seed = Some(Choice::seed(given()?)?),
            "--out" => out = Some(PathBuf::from(given()?)),
            "--full" => full = true,
            "--restart" => restart = true,
            "--fill" => {
                // Its kinds are optional: the next word is kinds when it names kinds, else a
                // folder, else a mistake.
                let next = words.clone().next().copied().filter(|w| !w.starts_with("--"));
                let kinds = next.filter(|w| crate::fill::kinds(Some(w)).is_ok() || !Path::new(w).is_dir());
                if kinds.is_some() {
                    words.next();
                }
                fill = Some(crate::fill::kinds(kinds).map_err(|e| format!("{e} (and it is not a study folder)"))?);
            }
            "--workers" => {
                let n = given()?;
                workers = Some(n.parse::<usize>().ok().filter(|n| *n > 0).ok_or(format!(
                    "--workers is how many files are read at once, not {n:?}"
                ))?);
            }
            flag if flag.starts_with("--") => {
                return Err(format!("press does not know {flag}.\n{USAGE}"))
            }
            folder if dir.is_none() => dir = Some(PathBuf::from(folder)),
            _ => return Err(format!("press takes one study folder.\n{USAGE}")),
        }
    }
    if full && fill.is_some() {
        return Err(format!("--fill reads only a file's holes, --full every entry: give one.\n{USAGE}"));
    }
    let (dir, name, target) = study(dir, out)?;
    let workers =
        workers.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let texts = inputs(&dir)?;
    let lock: Lock = toml::from_str(&texts.lock)
        .map_err(|e| format!("samples.lock is not a lock rvr wrote: {e}"))?;
    choice.check(&lock.sample)?;
    if restart {
        let stopped = crate::journal::Staged::lock(&target)?;
        if stopped.keep.get() {
            println!("Starting over: removed the press that stopped in {}.", stopped.folder.display());
        }
        stopped.keep.set(false);
    }
    press(
        &texts,
        &dir,
        &name,
        &target,
        &|s| choice.of(s),
        workers,
        match (&fill, full) {
            (Some(kinds), _) => On::Holes(kinds),
            (None, true) => On::Nothing,
            (None, false) => On::Earlier,
        },
    )
}

/// A study folder's full path, its name, and the file a press writes (`--out`, or `DIR/<name>.rvr`).
pub fn study(
    given: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<(PathBuf, String, PathBuf), String> {
    let given = given.unwrap_or_else(|| PathBuf::from("."));
    let dir = given
        .canonicalize()
        .map_err(|e| format!("{}: {e}", given.display()))?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("study")
        .to_string();
    let target = out.unwrap_or_else(|| match given.as_os_str() == "." {
        true => PathBuf::from(format!("{name}.rvr")),
        false => given.join(format!("{name}.rvr")),
    });
    Ok((dir, name, target))
}

/// The columns a recipe stores, in the order a file holds them: its copies, then its computed columns.
pub fn outputs(recipe: &Recipe) -> Vec<Output<'_>> {
    recipe
        .copy
        .iter()
        .map(|(n, e)| (n, e, true))
        .chain(recipe.columns.iter().map(|(n, e)| (n, e, false)))
        .collect()
}

/// The three texts of a study, which the file carries.
pub struct Texts {
    pub recipe: String,
    pub samples: String,
    pub lock: String,
}

/// Read a study's three files and refuse a lock that no longer matches its samples.toml.
pub fn inputs(dir: &Path) -> Result<Texts, String> {
    let (texts, lock) = texts(dir)?;
    if !resolved(&texts, &lock) {
        return Err(format!("samples.toml has changed since samples.lock was written ({}). Run rvr samples resolve samples.toml, review the new lock, then press.", lock.resolved));
    }
    Ok(texts)
}

/// Whether the lock was resolved from this samples.toml.
pub fn resolved(texts: &Texts, lock: &Lock) -> bool {
    crate::samples::sha256(texts.samples.as_bytes()) == lock.samples_toml_sha256
}

/// A study's three texts and its lock, read as they are (`inputs` also refuses a stale lock).
pub fn texts(dir: &Path) -> Result<(Texts, Lock), String> {
    let read = |file: &str| {
        std::fs::read_to_string(dir.join(file)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound if file == "samples.lock" => format!(
                "{} has no samples.lock: run rvr samples resolve samples.toml there first.",
                dir.display()
            ),
            std::io::ErrorKind::NotFound if file == "samples.toml" => format!(
                "{} has a samples.lock but no samples.toml. A .rvr file carries both, so press needs both: put samples.toml back (git checkout samples.toml).",
                dir.display()
            ),
            _ => format!("{}: {e}", dir.join(file).display()),
        })
    };
    let lock = read("samples.lock")?;
    let parsed: Lock =
        toml::from_str(&lock).map_err(|e| format!("samples.lock is not a lock rvr wrote: {e}"))?;
    let samples = read("samples.toml")?;
    let texts = Texts {
        recipe: read("recipe.toml")?,
        samples,
        lock,
    };
    Ok((texts, parsed))
}

/// What each sample of the lock reads: `events` of it, written into the lock as a press reads.
pub fn plan(
    lock: &mut Lock,
    events: &dyn Fn(&Locked) -> Result<Events, String>,
) -> Result<Vec<Events>, String> {
    let mut asked = Vec::new();
    for sample in &mut lock.sample {
        let want = events(sample)?;
        resolve::planned(sample, want)?;
        asked.push(want);
    }
    Ok(asked)
}

/// The recipe a press runs, or what stops it.
pub fn read_recipe(source: &str) -> Result<Recipe, String> {
    let recipe = Recipe::read(source).map_err(|e| format!("recipe.toml: {}", e.0))?;
    if recipe.copy.is_empty() && recipe.columns.is_empty() {
        return Err("recipe.toml stores nothing: name branches under [copy] or computed columns under [column].".into());
    }
    Ok(recipe)
}

/// An output column: the name written, the expression it comes from, and whether it is a copy.
pub type Output<'a> = (&'a String, &'a Expression, bool);

/// One file to read, in the lock's order.
#[derive(Clone)]
pub(crate) struct Input<'a> {
    pub(crate) sample: usize,
    pub(crate) tree: &'a str,
    pub(crate) file: &'a File,
    /// For a random choice, the entries chosen in this file, ascending.
    pub(crate) chosen: Option<Vec<u64>>,
    /// When building on an earlier file, the entries it kept of this file, ascending, and its
    /// record of the file.
    pub(crate) kept: Option<Vec<u64>>,
    pub(crate) before: Option<Value>,
    /// For a fill, the holes of the file being filled that are read, as entries `[first, last)`,
    /// each with its own place among the pieces of the spool.
    pub(crate) ranges: Option<Vec<(u64, u64, usize)>>,
    /// Its place among the pieces of the spool, when it is read whole.
    pub(crate) order: usize,
}

/// Where a study's files can be read from: its folder, its storage and its datasets' copies.
pub(crate) struct Reach<'a> {
    pub storage: Option<&'a Storage>,
    /// Where each file of a catalog dataset can be read.
    pub datasets: Option<&'a Datasets>,
    /// Why a remote file cannot be read, when there is no storage.
    pub offline: &'a str,
    /// The study folder, which a relative path in the lock is relative to.
    pub dir: &'a Path,
    /// What every read shares while it waits; it stops once an input ends the press.
    pub watch: Arc<Watch>,
    /// Whether a local file is read whole for its adler32 before it is opened (a press reads it;
    /// a look at its branches or first events does not).
    pub whole: bool,
}

/// What every input of one press shares.
struct Shared<'a> {
    recipe: &'a Recipe,
    outputs: &'a [Output<'a>],
    reach: Reach<'a>,
    /// Every name the recipe mentions, to choose the branches fetched ahead.
    mentions: &'a BTreeSet<String>,
    /// Where each input's kept values go as they are gathered, by output and input.
    spool: &'a write::Spool,
    /// Where each input finished is noted, so that a press stopped part way is picked up; not
    /// written for a press that cannot be (`carries` false).
    journal: &'a crate::journal::Journal,
    carries: bool,
}

/// The branches of one open tree over the entries in hand, read when an expression first names them.
pub(crate) struct Branches<'a, S: ByteSource> {
    file: &'a RootFile<S>,
    tree: &'a Tree,
    names: &'a [String],
    first: i64,
    last: i64,
    read: RefCell<HashMap<String, Array>>,
    /// The first failure to read a branch, which tells a file that cannot be read from a recipe
    /// that asks the wrong thing of it.
    failure: RefCell<Option<hep_root::Error>>,
}

impl<'a, S: ByteSource> Branches<'a, S> {
    /// Entries `first..last` of a tree whose branches are `names`.
    pub fn over(file: &'a RootFile<S>, tree: &'a Tree, names: &'a [String], first: i64, last: i64) -> Self {
        let read = RefCell::new(HashMap::new());
        Branches { file, tree, names, first, last, read, failure: RefCell::new(None) }
    }

    /// The first failure to read a branch, if one failed.
    pub fn failure(&self) -> Option<hep_root::Error> {
        self.failure.take()
    }
}

/// A hep-root number kind as a `.rvr` kind.
fn kind_of(kind: hep_root::columns::Kind) -> Kind {
    use hep_root::columns::Kind as R;
    let name = match kind {
        R::Bool => "b1",
        R::I8 => "i1",
        R::U8 => "u1",
        R::I16 => "i2",
        R::U16 => "u2",
        R::I32 => "i4",
        R::U32 => "u4",
        R::I64 => "i8",
        R::U64 => "u8",
        R::F32 => "f4",
        R::F64 => "f8",
    };
    Kind::named(name).expect("every hep-root kind has a .rvr name")
}

impl<S: ByteSource> Columns for Branches<'_, S> {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        if let Some(held) = self.read.borrow().get(name) {
            return Ok(Some(held.clone()));
        }
        let Some(branch) = self.tree.branch(name) else {
            return Ok(None);
        };
        let values = match self.file.read_range(branch, self.first, self.last) {
            Ok(values) => values,
            Err(e) => {
                let said = rvr_recipe::Error(format!("{name}: {e}"));
                self.failure.borrow_mut().get_or_insert(e);
                return Err(said);
            }
        };
        let held = Values::from_bytes(kind_of(values.kind), &values.bytes);
        let array = match &values.counts {
            Some(counts) => Array::object(counts, held)?,
            None => Array::Event(held),
        };
        self.read
            .borrow_mut()
            .insert(name.to_owned(), array.clone());
        Ok(Some(array))
    }

    fn begins(&self, start: &str) -> bool {
        self.names.iter().any(|name| name.starts_with(start))
    }
}

/// One output column gathered over the kept events of a file, then of a sample.
struct Gathered {
    name: String,
    kind: Kind,
    bytes: Vec<u8>,
    /// For a list column, how many members each kept event holds.
    counts: Option<Vec<usize>>,
}

/// What one input contributed; its values are in the spool.
pub(crate) struct Taken {
    /// Each output's kind and whether it is a list, unless no entry was read.
    pub(crate) kinds: Vec<(Kind, bool)>,
    /// How many events it kept.
    pub(crate) rows: usize,
    /// Its record in `made.files`, its holes included.
    pub(crate) record: Value,
    /// Entries read, before any cut.
    pub(crate) entries: u64,
    /// The entries chosen that lie in its holes.
    pub(crate) missing: u64,
    /// The events still kept after each cut.
    pub(crate) cutflow: Vec<u64>,
}

/// The metadata fields an expression can read: numbers, text, and truth values.
pub(crate) fn fields(metadata: &Map<String, Value>) -> BTreeMap<String, Constant> {
    let read = |value: &Value| match value {
        Value::Bool(truth) => Some(Constant::Truth(*truth)),
        Value::String(text) => Some(Constant::Text(text.clone())),
        Value::Number(number) => match number.as_i64() {
            Some(whole) if !number.to_string().contains(['.', 'e', 'E']) => {
                Some(Constant::Whole(whole))
            }
            _ => number.as_f64().map(Constant::Float),
        },
        _ => None,
    };
    metadata
        .iter()
        .filter_map(|(name, value)| Some((name.clone(), read(value)?)))
        .collect()
}

/// The branch a copied column names: a branch by name, or `branch(...)` of a sample field.
fn copied(scope: &Scope, from: &Expression) -> Result<Array, String> {
    let name = match from {
        Expression::Name(name) => name.clone(),
        Expression::Call(_, arguments) if arguments.len() == 1 => {
            match scope.evaluate(&arguments[0]).map_err(|e| e.0)? {
                Array::Constant(Constant::Text(name)) => name,
                _ => return Err("branch(...) needs the name of a branch as text".into()),
            }
        }
        _ => return Err("a copied column names one branch".into()),
    };
    scope
        .columns
        .column(&name)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("there is no branch named {name}"))
}

/// Which events one cut lets through: those it does not apply to, and those that pass it.
fn lets_through(cut: &rvr_recipe::Cut, scope: &Scope, entries: usize) -> Result<Vec<bool>, String> {
    let named = |error: rvr_recipe::Error| format!("[keep] {}: {}", cut.name, error.0);
    let applies = match &cut.when {
        None => None,
        Some(when) => match scope.evaluate(when).map_err(named)? {
            Array::Constant(Constant::Missing) => return Ok(vec![true; entries]),
            _ => Some(scope.mask(when, entries).map_err(named)?),
        },
    };
    if applies
        .as_ref()
        .is_some_and(|applies| !applies.contains(&true))
    {
        return Ok(vec![true; entries]);
    }
    let mut pass = scope.mask(&cut.pass, entries).map_err(named)?;
    for (pass, applies) in pass.iter_mut().zip(applies.iter().flatten()) {
        *pass |= !applies;
    }
    Ok(pass)
}

/// Apply the cuts to one run of a file's events, counting what each leaves.
pub(crate) fn kept(
    recipe: &Recipe,
    scope: &Scope,
    mut keep: Vec<bool>,
    cutflow: &mut [u64],
) -> Result<Vec<bool>, String> {
    let entries = keep.len();
    for (index, cut) in recipe.keep.iter().enumerate() {
        for (keep, through) in keep.iter_mut().zip(lets_through(cut, scope, entries)?) {
            *keep &= through;
        }
        cutflow[index] += keep.iter().filter(|keep| **keep).count() as u64;
    }
    Ok(keep)
}

/// Give an output column the kind the recipe asks it to be stored as.
fn stored(recipe: &Recipe, column: &str, array: Array) -> Array {
    match (recipe.stored.get(column), array) {
        (Some(kind), Array::Event(values)) => Array::Event(values.cast(*kind)),
        (Some(kind), Array::Object { offsets, values }) => Array::Object {
            offsets,
            values: values.cast(*kind),
        },
        (_, array) => array,
    }
}

/// Add one run's kept events of a column to what the file has gathered.
fn gather(held: &mut Gathered, column: Array, keep: &[bool], at: &str) -> Result<(), String> {
    let name = &held.name;
    let (values, offsets) = match &column {
        Array::Event(values) => (values, None),
        Array::Object { offsets, values } => (values, Some(offsets)),
        Array::Constant(_) => return Err(format!("{name} is a fixed value, not a column")),
    };
    if values.kind != held.kind || offsets.is_some() != held.counts.is_some() {
        return Err(format!(
            "{name} is not the same kind of column in {at} as in the entries before"
        ));
    }
    let w = held.kind.width();
    let bytes = values.to_bytes();
    for (event, _) in keep.iter().enumerate().filter(|(_, keep)| **keep) {
        let (from, to) = match offsets {
            Some(offsets) => (offsets[event], offsets[event + 1]),
            None => (event, event + 1),
        };
        held.bytes.extend_from_slice(&bytes[from * w..to * w]);
        if let Some(counts) = &mut held.counts {
            counts.push(to - from);
        }
    }
    Ok(())
}

/// Send what a file has gathered since it last did to the spool, keeping each column's kind:
/// its columns, then its entry numbers.
fn spooled(
    spool: &write::Spool,
    gathered: &mut [Option<Gathered>],
    entry: &mut Vec<i64>,
    order: write::Order,
) -> Result<(), String> {
    let failed = |e: rowver::Error| format!("its kept values could not be held on disk ({e}): free disk space beside the output");
    let entries = write::array(&std::mem::take(entry));
    spool
        .add(gathered.len(), order, write::Values::Flat(entries))
        .map_err(failed)?;
    for (index, held) in gathered
        .iter_mut()
        .enumerate()
        .filter_map(|(i, h)| Some((i, h.as_mut()?)))
    {
        let buffer = NumericBuffer {
            kind: held.kind.name().into(),
            count: held.bytes.len() / held.kind.width(),
            bytes: std::mem::take(&mut held.bytes),
        };
        let values = match &mut held.counts {
            Some(counts) => write::Values::Jagged {
                counts: std::mem::take(counts),
                content: buffer,
            },
            None => write::Values::Flat(buffer),
        };
        spool.add(index, order, values).map_err(failed)?;
    }
    Ok(())
}

/// Refuse a file that differs from what the lock recorded of it.
fn changed(file: &File, bytes: Option<u64>, adler32: Option<String>) -> Result<(), Failed> {
    match resolve::differs(file, bytes, adler32.as_deref()) {
        None => Ok(()),
        Some(how) => Err(Failed::new(Fault::Changed, format!("{how}. It has changed since samples.lock was written: if the change is wanted, run rvr samples resolve samples.toml and review the new lock."))),
    }
}

/// A source that keeps the ranges it was last told to hold, so that the basket reads of a chunk
/// are answered from memory: every basket the recipe may read of a remote file, fetched in a
/// few requests at once instead of branch after branch. A read that held ranges cover only in
/// part fetches just the rest.
pub(crate) struct Held {
    inner: Box<dyn ByteSource>,
    /// Disjoint ranges, in order of position.
    held: Mutex<Vec<(u64, Vec<u8>)>>,
    /// For a remote file: how long a read that failed waits to be asked again, and the watch
    /// that ends the waiting (another input failed). Fetching ahead is never asked again.
    again: Option<(&'static [u64], Arc<Watch>)>,
    /// For a remote file, what its last failed request said.
    seen: Option<Arc<Seen>>,
    /// What the file is, for what is printed while a read waits.
    shown: String,
}

/// The parts of `offset..offset + len` that no held range covers.
fn gaps(held: &[(u64, Vec<u8>)], offset: u64, len: usize) -> Vec<(u64, usize)> {
    let (end, mut at, mut out) = (offset + len as u64, offset, Vec::new());
    for (start, bytes) in held {
        let stop = start + bytes.len() as u64;
        if stop <= at {
            continue;
        }
        if *start >= end {
            break;
        }
        if *start > at {
            out.push((at, (start - at) as usize));
        }
        at = stop;
        if at >= end {
            break;
        }
    }
    if at < end {
        out.push((at, (end - at) as usize));
    }
    out
}

/// Copy into `buf` (which starts at `offset`) whatever `pieces` hold of it.
fn copy_from(pieces: &[(u64, Vec<u8>)], offset: u64, buf: &mut [u8]) {
    let end = offset + buf.len() as u64;
    for (start, bytes) in pieces {
        let (from, to) = ((*start).max(offset), (start + bytes.len() as u64).min(end));
        if from < to {
            let (f, t) = ((from - offset) as usize, (to - offset) as usize);
            buf[f..t].copy_from_slice(&bytes[(from - start) as usize..(to - start) as usize]);
        }
    }
}

impl Held {
    /// Hold these spans, joined where they lie within 16 kB (a storage that answers many
    /// ranges in one request carries a short gap faster than it seeks past one). What is
    /// already held of them is kept and not fetched again; the rest of what was held is dropped.
    fn hold(&self, spans: Vec<(u64, u64)>) -> hep_root::Result<()> {
        const NEAR: u64 = 16 * 1024;
        let ranges: Vec<(u64, usize)> = spans.iter().map(|&(at, len)| (at, len as usize)).collect();
        let wanted = crate::samples::remote::joined(&ranges, NEAR);
        let overlaps = |start: u64, len: usize| {
            wanted
                .iter()
                .any(|&(at, l)| start < at + l as u64 && at < start + len as u64)
        };
        let mut kept = std::mem::take(&mut *self.held.lock().unwrap());
        kept.retain(|(start, bytes)| overlaps(*start, bytes.len()));
        let missing: Vec<(u64, usize)> = wanted
            .iter()
            .flat_map(|&(at, len)| gaps(&kept, at, len))
            .collect();
        let fetched = match self.inner.read_ranges(&missing) {
            Ok(fetched) => {
                self.answered();
                fetched
            }
            Err(e) => {
                *self.held.lock().unwrap() = kept;
                return Err(e);
            }
        };
        kept.extend(missing.iter().map(|r| r.0).zip(fetched));
        kept.sort_unstable_by_key(|(start, _)| *start);
        *self.held.lock().unwrap() = kept;
        Ok(())
    }
}

impl Held {
    /// Read from the file itself, asked again while a remote read fails as a busy storage does.
    fn fetch(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        match &self.again {
            Some((waits, watch)) => {
                let read = read_again(waits, watch, &self.shown, || {
                    self.inner.read_ranges(ranges)
                });
                if read.is_ok() {
                    self.answered();
                }
                read
            }
            None => self.inner.read_ranges(ranges),
        }
    }

    /// A remote read succeeded: what failed before it is past.
    fn answered(&self) {
        if let Some(seen) = &self.seen {
            seen.clear();
        }
    }

    /// For a remote file, what its last failed request said.
    pub(crate) fn seen(&self) -> Option<&Seen> {
        self.seen.as_deref()
    }
}

impl ByteSource for Held {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
        let [read] = &mut self.read_ranges(&[(offset, buf.len())])?[..] else {
            unreachable!("one range asked, one answered")
        };
        buf.copy_from_slice(read);
        Ok(())
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn read_ranges(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        let held = self.held.lock().unwrap();
        if held.is_empty() {
            drop(held);
            return self.fetch(ranges);
        }
        let mut out: Vec<Vec<u8>> = ranges.iter().map(|&(_, len)| vec![0; len]).collect();
        for (buf, &(offset, _)) in out.iter_mut().zip(ranges) {
            copy_from(&held, offset, buf);
        }
        let missing: Vec<(u64, usize)> = ranges
            .iter()
            .flat_map(|&(at, len)| gaps(&held, at, len))
            .collect();
        drop(held);
        if !missing.is_empty() {
            let fetched: Vec<(u64, Vec<u8>)> = missing
                .iter()
                .map(|r| r.0)
                .zip(self.fetch(&missing)?)
                .collect();
            for (buf, &(offset, _)) in out.iter_mut().zip(ranges) {
                copy_from(&fetched, offset, buf);
            }
        }
        Ok(out)
    }
}

/// Whether a branch may be asked for: its name, or the name it would have as an attribute or a
/// count of a collection, appears in the recipe or among the sample's text fields.
fn mentioned(
    mentions: &BTreeSet<String>,
    sample: &BTreeMap<String, Constant>,
    branch: &str,
) -> bool {
    let mut parts = branch
        .match_indices('_')
        .map(|(at, _)| (&branch[..at], &branch[at + 1..]));
    mentions.contains(branch)
        || parts.any(|(collection, attribute)| {
            mentions.contains(collection) && mentions.contains(attribute)
        })
        || branch
            .strip_prefix('n')
            .is_some_and(|collection| mentions.contains(collection))
        || sample
            .values()
            .any(|value| matches!(value, Constant::Text(text) if text == branch))
}

/// The on-disk spans of the baskets of `branches` holding entries `first..last`.
fn spans(branches: &[&hep_root::Branch], first: i64, last: i64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for branch in branches {
        let baskets =
            (branch.write_basket.max(0) as usize).min(branch.basket_entry.len().saturating_sub(1));
        for i in 0..baskets {
            let (seek, bytes) = (
                branch.basket_seek.get(i).copied().unwrap_or(0),
                branch.basket_bytes.get(i).copied().unwrap_or(0),
            );
            if branch.basket_entry[i] < last
                && branch.basket_entry[i + 1] > first
                && seek > 0
                && bytes > 0
            {
                out.push((seek as u64, bytes as u64));
            }
        }
    }
    out
}

/// Check one input against the lock and open it, local, remote, or from a dataset's copies (with
/// the reader that says which copy served it); a failure says what it means for the file.
pub(crate) fn open(reach: &Reach, file: &File) -> Result<(RootFile<Held>, Option<Reader>), Failed> {
    let at = &file.at;
    // No usable grid proxy is not the file's fault: every remote file would fail the same.
    let offline = || Failed::new(Fault::Save, format!("{at}: {}", reach.offline));
    let again = Some((READING, reach.watch.clone()));
    if file.lfn.is_some() {
        let (Some(datasets), Some(storage)) = (reach.datasets, reach.storage) else {
            return Err(offline());
        };
        // A copy failing as a busy storage does is asked again, as a read is.
        let busy = |refused: &Refusal| refused.busy.then(|| refused.why.clone());
        let name = at.rsplit('/').next().unwrap_or(at);
        let seen = Arc::new(Seen::default());
        let reader = patiently(READING, &reach.watch, name, busy, || datasets.open(storage, file, &seen))
            .map_err(|refused| match reach.watch.stop.load(Ordering::Acquire) {
                true => Failed::new(Fault::Stopped, format!("{at}: {STOPPED}")),
                false => Failed::new(refused.fault, refused.why),
            })?;
        let handle = reader.handle();
        return opened(file, Box::new(reader), again, Some(seen)).map(|root| (root, Some(handle)));
    }
    if !at.contains("://") {
        // A relative path is the lock's own: relative to the study folder.
        let path = reach.dir.join(at);
        // A file whose folder is not there either may be on a disk not mounted: not the file's fault.
        let folder = path.parent().is_none_or(Path::exists);
        let meta = std::fs::metadata(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound if !folder => Failed::new(Fault::Save, format!(
                "{at} cannot be read: its folder is not there either (a disk not mounted?)"
            )),
            std::io::ErrorKind::NotFound => Failed::new(Fault::Gone, format!(
                "{at} no longer exists: put it back and run rvr press --fill (if it moved, run rvr samples resolve samples.toml again, then rvr press)."
            )),
            _ => Failed::new(Fault::Save, format!("{at} could not be read: {e}")),
        })?;
        // A file that cannot be read for want of rights, or on a mount that went, is not the
        // file's fault.
        let unreadable = |e: std::io::Error| Failed::new(Fault::Save, format!("{at} could not be read: {e}"));
        // Read whole for its adler32 only when the lock has one to compare with, and a press asks.
        let adler32 = match file.adler32 {
            Some(_) if reach.whole => Some(adler32_file(&path).map_err(unreadable)?),
            _ => None,
        };
        changed(file, Some(meta.len()), adler32)?;
        let local = hep_root::LocalFile::open(&path)
            .map_err(|e| Failed::new(fault_of(&e, None), format!("{at}: {e}")))?;
        return opened(file, Box::new(local), None, None).map(|root| (root, None));
    }
    let storage = reach.storage.ok_or_else(offline)?;
    // The storage's size and adler32 are asked while the file is opened at the size the lock
    // gives; a changed file is reported before anything the opening found.
    std::thread::scope(|scope| {
        let agent = storage.agent();
        let digest = scope.spawn(move || {
            digest_seen(&agent, at, storage.waits, storage.proxy.is_some(), &storage.watch)
        });
        let seen = Arc::new(Seen::default());
        let root = HttpsFile::new(storage, at, file.bytes)
            .map_err(Failed::from)
            .and_then(|remote| opened(file, Box::new(remote.seeing(&seen)), again, Some(seen)));
        let (bytes, adler32) = digest
            .join()
            .map_err(|_| Failed::from(format!("{at}: asking its storage stopped unexpectedly")))??;
        changed(file, bytes, adler32)?;
        root.map(|root| (root, None))
    })
}

/// Where the sample `named` is in the lock (the first sample when none is named).
pub(crate) fn sample_at(lock: &Lock, named: Option<&str>) -> Result<usize, String> {
    if lock.sample.is_empty() {
        return Err("samples.lock has no samples: add one to samples.toml and resolve again".into());
    }
    let Some(name) = named else { return Ok(0) };
    lock.sample.iter().position(|s| s.name == name).ok_or_else(|| {
        let names: Vec<&str> = lock.sample.iter().map(|s| s.name.as_str()).collect();
        format!("samples.lock has no sample named {name}; it has {}", names.join(", "))
    })
}

/// How many of a sample's files are tried, in the lock's order, for one with a copy on disk.
const ON_DISK: usize = 10;

/// The first of a sample's files that can be read now, opened, with its entry in the lock. A file
/// of a dataset with no copy on disk (only on tape) is passed over, as is one with no entries
/// when `entries` is asked for.
pub(crate) fn first_readable<'a>(
    lock: &'a Lock,
    sample: usize,
    dir: &Path,
    entries: bool,
) -> Result<(RootFile<Held>, &'a File), String> {
    let s = &lock.sample[sample];
    let files: Vec<(usize, &File)> = s
        .file
        .iter()
        .filter(|f| !entries || f.entries != Some(0))
        .take(ON_DISK)
        .map(|f| (sample, f))
        .collect();
    if files.is_empty() {
        return Err(format!("{} has no file with events", s.name));
    }
    let watch = Arc::new(Watch::default());
    let (storage, datasets, offline) = connect(lock, &files, &watch)?;
    let reach = Reach { storage: storage.as_ref(), datasets: datasets.as_ref(), offline: &offline, dir, watch, whole: false };
    let mut tape = None;
    for &(_, file) in &files {
        match open(&reach, file) {
            Ok((root, _)) => return Ok((root, file)),
            Err(failed) if failed.fault == Fault::Tape => tape = tape.or(Some(failed.why)),
            Err(failed) => return Err(failed.why),
        }
    }
    let why = tape.unwrap_or_default();
    Err(match files.len() {
        1 => why,
        n => format!("none of the first {n} files of {} has a copy on disk. The first: {why}", s.name),
    })
}

/// Open a checked input and refuse one whose ROOT UUID is not the lock's; `again` and `seen` as
/// `Held`'s.
fn opened(
    file: &File,
    source: Box<dyn ByteSource>,
    again: Option<(&'static [u64], Arc<Watch>)>,
    seen: Option<Arc<Seen>>,
) -> Result<RootFile<Held>, Failed> {
    let at = &file.at;
    let source = Held {
        shown: source.describe(),
        inner: source,
        held: Mutex::new(Vec::new()),
        again,
        seen: seen.clone(),
    };
    let root = RootFile::open(source).map_err(|e| {
        let fault = fault_of(&e, seen.as_deref());
        match e {
            hep_root::Error::Remote(_) | hep_root::Error::SourceChanged(_) | hep_root::Error::Io(_) => {
                Failed::new(fault, format!("{at} could not be read: {e}"))
            }
            e => Failed::new(fault, format!("{at} is not a ROOT file this reader can open: {e}")),
        }
    })?;
    let uuid = root.header().uuid_string();
    if file.uuid.as_ref().is_some_and(|locked| *locked != uuid) {
        return Err(Failed::new(Fault::Changed, format!("{at} is not the file samples.lock recorded: its ROOT UUID was {}, it is now {uuid}. Run rvr samples resolve samples.toml and review the new lock.", file.uuid.as_deref().unwrap_or("?"))));
    }
    Ok(root)
}

/// What to do about a failure reading a file's events: a name that is not a branch, with the
/// nearest branches; a function that is not one, with the nearest functions and the recipe's
/// helpers; a damaged file (hep-root says `decompression failed`), whose every copy is damaged
/// when it is a dataset's.
pub(crate) fn hint(error: &str, branches: &[String], recipe: &Recipe, dataset: bool) -> String {
    let missing = ["there is no branch named ", "There is no column named "]
        .iter()
        .find_map(|said| error.rsplit_once(said).map(|(_, name)| name.trim()));
    if let Some(name) = missing {
        // A collection is named by the prefix of its branches (Jet for Jet_pt), so those count too.
        let prefixes = branches.iter().filter_map(|b| b.split_once('_').map(|(p, _)| p));
        let mut names: Vec<&str> = branches.iter().map(String::as_str).chain(prefixes).collect();
        names.sort();
        names.dedup();
        let near = resolve::nearest(name, names.into_iter());
        let near = match near.is_empty() {
            true => String::new(),
            false => format!(" Did you mean: {}?", near.join(", ")),
        };
        return format!(".{near} rvr branches lists every branch and collection the file has.");
    }
    if let Some((_, name)) = error.rsplit_once("There is no operation or helper named ") {
        let helpers = recipe.helpers.keys().map(String::as_str);
        let near = resolve::nearest(name.trim(), rvr_recipe::FUNCTIONS.iter().copied().chain(helpers));
        let near = match near.is_empty() {
            true => String::new(),
            false => format!(" Did you mean: {}?", near.join(", ")),
        };
        return format!(".{near} Every function, with an example, is in the recipe reference: {RECIPES}");
    }
    match (error.contains("decompression failed"), dataset) {
        (false, _) => String::new(),
        (true, true) => ". The file is damaged there, and every copy of a dataset's file holds the same bytes: report the file to the dataset's owners.".into(),
        (true, false) => ". The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.".into(),
    }
}

/// How many entries of `[first, last)` an input's choice chose: all, or a random choice's.
pub(crate) fn chosen_in(input: &Input, first: u64, last: u64) -> u64 {
    match &input.chosen {
        Some(chosen) => {
            (chosen.partition_point(|e| *e < last) - chosen.partition_point(|e| *e < first)) as u64
        }
        None => last.saturating_sub(first),
    }
}

/// An input's record as the lock gives it: its tree, UUID (`null` when the lock has none) and
/// entries, and what the choice reads of it.
fn planned(input: &Input) -> Value {
    let mut record = json!({"tree": input.tree, "uuid": input.file.uuid, "entries": input.file.entries.unwrap_or(0)});
    if let Some(stop) = input.file.stop {
        record["stop"] = stop.into();
    }
    if let Some(chosen) = &input.chosen {
        record["take"] = chosen.len().into();
    }
    record
}

/// A failure as a hole, or what ends the press: one that is no kind of hole, or any building on an
/// earlier file (whose own holes are its only ones).
fn hole_or_end(mut failed: Failed, input: &Input) -> Result<Failed, Failed> {
    if failed.fault.kind().is_some() && input.kept.is_some() {
        failed.why += " Building on the earlier file, a file that cannot be read stops the press (rvr press --full starts over, and makes holes of it).";
        return Err(failed);
    }
    match failed.fault.kind() {
        Some(_) => Ok(failed),
        None => Err(failed),
    }
}

/// A hole as a file records it.
fn hole((first, last, failed): &(u64, u64, Failed)) -> Value {
    json!({"entries": [first, last], "kind": failed.fault.kind(), "why": failed.why})
}

/// A hole's entries, as `[first, last)`.
pub(crate) fn range(hole: &Value) -> (u64, u64) {
    let entry = |at: usize| hole["entries"][at].as_u64().unwrap_or(0);
    (entry(0), entry(1))
}

/// The holes a file's record holds, as `[first, last)`.
pub(crate) fn ranges(record: &Value) -> Vec<(u64, u64)> {
    record["holes"].as_array().into_iter().flatten().map(range).collect()
}

/// Read one input a run of entries at a time, keeping the events the cuts pass. A file that
/// cannot be opened, and a run of it that cannot be read, are holes; the error is what ends the
/// press (a mistake in the recipe, a failure that is not the file's own, or another input's).
fn take(
    shared: &Shared,
    sample: &BTreeMap<String, Constant>,
    input: &Input,
    order: usize,
) -> Result<Taken, Failed> {
    let Shared {
        recipe, outputs, ..
    } = *shared;
    let at = &input.file.at;
    let located = |error: String| format!("{at}: {error}");
    let nothing = |record: Value, missing: u64| Taken {
        kinds: Vec::new(),
        rows: 0,
        record,
        entries: 0,
        missing,
        cutflow: vec![0; recipe.keep.len()],
    };
    // A file a random choice takes nothing from is recorded from the lock, not opened, when the
    // lock has its UUID; a dataset file has none in the lock, so it is opened for it.
    if input.chosen.as_ref().is_some_and(Vec::is_empty) && input.file.uuid.is_some() && input.file.entries.is_some() {
        if input.kept.as_ref().is_some_and(|kept| !kept.is_empty()) {
            return Err(located("none of the events the earlier file kept were chosen (a bug in rvr: press with --full, and please report it)".into()).into());
        }
        return Ok(nothing(planned(input), 0));
    }
    // The entries the choice reads of the file, by the lock, and (building on an earlier file)
    // the holes that file has in it, which are this file's too.
    let limit = input.file.stop.or(input.file.entries).unwrap_or(0);
    let carried = input.before.as_ref().map(ranges).unwrap_or_default();
    let missed = |holes: &[(u64, u64)]| holes.iter().map(|(first, last)| chosen_in(input, *first, *last)).sum();
    // A file the earlier one could not open, or chose nothing of, is not opened again: its
    // record is the earlier one.
    if let (Some(before), true) = (&input.before, missed(&carried) == chosen_in(input, 0, limit)) {
        return Ok(nothing(before.clone(), missed(&carried)));
    }
    // What it reads: every entry the choice reads, or for a fill the holes asked, each range
    // with its own place among the spool's pieces.
    let to_read = input.ranges.clone().unwrap_or_else(|| vec![(0, limit, order)]);
    let chosen_here: u64 = to_read.iter().map(|(from, to, _)| chosen_in(input, *from, *to)).sum();
    // A file that cannot be opened is one hole of every entry chosen of what it reads.
    let unread = |failed: Failed| {
        let failed = hole_or_end(failed, input)?;
        let ranges = to_read.iter().filter(|(from, to, _)| chosen_in(input, *from, *to) > 0);
        let holes: Vec<Value> = ranges.map(|&(from, to, _)| hole(&(from, to, failed.clone()))).collect();
        let mut record = planned(input);
        if !holes.is_empty() {
            record["holes"] = holes.into();
        }
        Ok(nothing(record, chosen_here))
    };
    let (file, served) = match open(&shared.reach, input.file) {
        Ok(opened) => opened,
        Err(failed) => return unread(failed),
    };
    let tree = match file.tree_at(input.tree) {
        Ok(tree) => tree,
        Err(e) => {
            let fault = fault_of(&e, file.source().inner().seen());
            return unread(Failed::new(fault, located(format!("its tree {:?}: {e}", input.tree))));
        }
    };
    let total = tree.entries.max(0) as u64;
    if input.file.entries.is_some_and(|locked| locked != total) {
        return unread(Failed::new(Fault::Changed, located(format!("its tree {} has {total} entries, not the {} samples.lock recorded. Run rvr samples resolve samples.toml and review the new lock.", input.tree, input.file.entries.unwrap()))));
    }
    let entries = input.file.stop.map_or(total, |stop| stop.min(total));
    let chosen = input.chosen.as_deref();
    let names: Vec<String> = tree
        .walk()
        .into_iter()
        .map(|(_, b)| b.name.clone())
        .collect();
    let mut sample = sample.clone();
    sample.insert("file".into(), Constant::Text(at.clone()));
    let remote = at.contains("://") || input.file.lfn.is_some();
    let wanted: Vec<&hep_root::Branch> = if remote {
        tree.walk()
            .into_iter()
            .map(|(_, b)| b)
            .filter(|b| {
                hep_root::columns::Kind::of(b).is_ok()
                    && mentioned(shared.mentions, &sample, &b.name)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut gathered: Vec<Option<Gathered>> = outputs.iter().map(|_| None).collect();
    let mut cutflow = vec![0_u64; recipe.keep.len()];
    let (mut entry, mut rows) = (Vec::new(), 0);
    let mut holes: Vec<(u64, u64, Failed)> = Vec::new();
    // Building on an earlier file, the events kept must be the ones it kept.
    let again = |entry: &[i64], from: usize| {
        match &input.kept {
        Some(kept) if kept.get(from..from + entry.len()).is_none_or(|kept| kept.iter().zip(entry).any(|(k, e)| *k as i64 != *e)) => Err(Failed::from(located("the events read again are not the ones the earlier file kept (a bug in rvr: press with --full, and please report it)".into()))),
        _ => Ok(()),
    }
    };
    let gap = if remote { CHUNK } else { GAP };
    'ranges: for (at_range, &(from, to, order)) in to_read.iter().enumerate() {
        let (to, mut piece, mut ended) = (to.min(entries), 0, false);
        let here = chosen.map(|c| &c[c.partition_point(|e| *e < from)..c.partition_point(|e| *e < to)]);
        for (first, last) in runs(from, to, here, gap) {
            if shared.reach.watch.stop.load(Ordering::Acquire) {
                return Err(Failed::new(Fault::Stopped, STOPPED));
            }
            let count = (last - first) as usize;
            // A random choice starts each run with only its chosen entries.
            let start = match chosen {
                Some(chosen) => {
                    let from = chosen.partition_point(|e| *e < first);
                    let to = chosen.partition_point(|e| *e < last);
                    let mut start = vec![false; count];
                    for e in &chosen[from..to] {
                        start[(e - first) as usize] = true;
                    }
                    start
                }
                None => vec![true; count],
            };
            // Building on an earlier file: its kept events and no others, without running the cuts;
            // a run in one of its holes is not read.
            if carried.iter().any(|(from, to)| *from <= first && last <= *to) {
                continue;
            }
            let start = match &input.kept {
                Some(kept) => {
                    let mut only = vec![false; count];
                    let from = kept.partition_point(|e| *e < first);
                    for &e in kept[from..].iter().take_while(|e| **e < last) {
                        only[(e - first) as usize] = start[(e - first) as usize];
                    }
                    only
                }
                None => start,
            };
            if remote {
                // Fetching ahead only saves round trips; a branch that cannot be fetched is reported
                // when an expression asks for it. What failed before the run's reads says nothing
                // of them.
                let _ = file
                    .source()
                    .inner()
                    .hold(spans(&wanted, first as i64, last as i64));
                if let Some(seen) = file.source().inner().seen() {
                    seen.clear();
                }
            }
            let branches = Branches::over(&file, &tree, &names, first as i64, last as i64);
            let scope = Scope {
                columns: &branches,
                sample: &sample,
                objects: &recipe.objects,
                collections: &recipe.collections,
                helpers: &recipe.helpers,
            };
            // A failure reading the events says where they are, and what to do when the file is damaged.
            let within = |error: String| {
                format!(
                    "{at}, tree {}, entries {} to {}: {error}{}",
                    input.tree,
                    self::count(first),
                    self::count(last - 1),
                    hint(&error, &names, recipe, input.file.lfn.is_some())
                )
            };
            // What each column held before the run, put back if the run fails part way: a run is
            // kept whole or not at all.
            let before: Vec<Option<(usize, usize)>> = gathered
                .iter()
                .map(|g| g.as_ref().map(|g| (g.bytes.len(), g.counts.as_ref().map_or(0, Vec::len))))
                .collect();
            let mut counted = vec![0_u64; recipe.keep.len()];
            let read = kept(recipe, &scope, start, &mut counted).and_then(|keep| {
                for (index, (column, from, plain)) in outputs.iter().enumerate() {
                    let array = if *plain {
                        copied(&scope, from)
                    } else {
                        scope
                            .evaluate(from)
                            .and_then(|value| rvr_recipe::per_event(value, count))
                            .map_err(|e| e.0)
                    };
                    // A copied branch's own name already begins what failed of it.
                    let named = |e: String| match e.starts_with(&format!("{column}: ")) {
                        true => e,
                        false => format!("{column}: {e}"),
                    };
                    let array = stored(recipe, column, array.map_err(named)?);
                    let (kind, listed) = match &array {
                        Array::Event(values) => (values.kind, false),
                        Array::Object { values, .. } => (values.kind, true),
                        Array::Constant(_) => return Err(format!("{column} is a fixed value, not a column")),
                    };
                    let held = gathered[index].get_or_insert_with(|| Gathered {
                        name: (*column).clone(),
                        kind,
                        bytes: Vec::new(),
                        counts: listed.then(Vec::new),
                    });
                    gather(held, array, &keep, at)?;
                }
                Ok(keep)
            });
            let keep = match read {
                Ok(keep) => keep,
                Err(error) => {
                    for (held, before) in gathered.iter_mut().zip(&before) {
                        match (held.as_mut(), before) {
                            (Some(held), Some((bytes, counts))) => {
                                held.bytes.truncate(*bytes);
                                if let Some(held) = &mut held.counts {
                                    held.truncate(*counts);
                                }
                            }
                            _ => *held = None,
                        }
                    }
                    // A branch that could not be read is the file's; anything else is the recipe's.
                    let fault = branches
                        .failure()
                        .map_or(Fault::Mistake, |e| fault_of(&e, file.source().inner().seen()));
                    let mut failed = Failed::new(fault, within(error));
                    // A local file that cannot be opened now at all (a mount gone) is not the file's
                    // fault; one that can, but not read there, is damaged there.
                    if !remote && matches!(fault, Fault::Damaged | Fault::Save) {
                        failed.fault = match std::fs::File::open(shared.reach.dir.join(at)) {
                            Ok(_) => Fault::Damaged,
                            Err(_) => Fault::Save,
                        };
                    }
                    let failed = hole_or_end(failed, input)?;
                    // Only a damaged basket leaves the rest of the file worth reading: otherwise the
                    // rest of this range, and every range after it, is a hole.
                    if failed.fault.local() {
                        holes.push((first, last, failed));
                        continue;
                    }
                    let rest = to_read[at_range + 1..].iter().map(|&(from, to, _)| (from, to.min(entries), failed.clone()));
                    let rest: Vec<_> = rest.filter(|(from, to, _)| chosen_in(input, *from, *to) > 0).collect();
                    holes.push((first, to, failed));
                    holes.extend(rest);
                    ended = true;
                    break;
                }
            };
            for (total, kept) in cutflow.iter_mut().zip(counted) {
                *total += kept;
            }
            entry.extend(
                (first..last)
                    .filter(|event| keep[(event - first) as usize])
                    .map(|event| event as i64),
            );
            let held: usize = gathered
                .iter()
                .flatten()
                .map(|g| g.bytes.len() + g.counts.as_ref().map_or(0, |c| c.len() * 8))
                .sum();
            if held + entry.len() * 8 >= SPOOLED {
                again(&entry, rows)?;
                rows += entry.len();
                spooled(shared.spool, &mut gathered, &mut entry, (order, piece)).map_err(|e| Failed::new(Fault::Save, located(e)))?;
                piece += 1;
            }
        }
        if !entry.is_empty() {
            again(&entry, rows)?;
            rows += entry.len();
            spooled(shared.spool, &mut gathered, &mut entry, (order, piece)).map_err(|e| Failed::new(Fault::Save, located(e)))?;
        }
        if ended {
            break 'ranges;
        }
    }
    if input.kept.as_ref().is_some_and(|kept| kept.len() != rows) {
        return Err(located("fewer events were read again than the earlier file kept (a bug in rvr: press with --full, and please report it)".into()).into());
    }
    let mut record =
        json!({"tree": input.tree, "uuid": file.header().uuid_string(), "entries": total});
    if let Some(stop) = input.file.stop {
        record["stop"] = stop.into();
    }
    if let Some(chosen) = chosen {
        record["take"] = chosen.len().into();
    }
    // A copy is counted as having served the file only when it was read to its end.
    if let Some(reader) = served.as_ref().filter(|_| holes.iter().all(|(_, _, failed)| failed.fault.local())) {
        reader.finish();
    }
    if let Some((rse, host)) = served.and_then(|reader| reader.served()) {
        record["served"] = json!({"rse": rse, "host": host});
    }
    // Building on an earlier file, its holes are this file's: what it kept was read around them.
    let missing = match input.before.as_ref().filter(|_| !carried.is_empty()) {
        Some(before) => {
            record["holes"] = before["holes"].clone();
            missed(&carried)
        }
        None => {
            if !holes.is_empty() {
                record["holes"] = holes.iter().map(hole).collect();
            }
            holes.iter().map(|(first, last, _)| chosen_in(input, *first, *last)).sum()
        }
    };
    Ok(Taken {
        kinds: gathered
            .into_iter()
            .flatten()
            .map(|g| (g.kind, g.counts.is_some()))
            .collect(),
        rows,
        record,
        entries: chosen_here - missing,
        missing,
        cutflow,
    })
}

/// The places among the spool's pieces an input's values take: its own, or for a fill each hole's.
fn orders(input: &Input) -> Vec<usize> {
    match &input.ranges {
        Some(ranges) => ranges.iter().map(|(_, _, order)| *order).collect(),
        None => vec![input.order],
    }
}

/// Read every input with a few workers, but those an earlier press of this one finished
/// (`picked`, by place), noting each in the journal; what each contributed, in the lock's order.
fn take_all(
    shared: &Shared,
    lock: &Lock,
    work: &[Input],
    workers: usize,
    picked: BTreeMap<usize, Taken>,
) -> Result<Vec<Taken>, Failed> {
    let fields: Vec<BTreeMap<String, Constant>> =
        lock.sample.iter().map(|s| fields(&s.metadata)).collect();
    let next = AtomicUsize::new(0);
    let watch = &shared.reach.watch;
    let todo: Vec<usize> = (0..work.len()).filter(|at| !picked.contains_key(at)).collect();
    let finished = AtomicUsize::new(picked.len());
    let mut done: Vec<Option<Result<Taken, Failed>>> = work.iter().map(|_| None).collect();
    for (at, taken) in picked {
        done[at] = Some(Ok(taken));
    }
    let done = Mutex::new(done);
    // For the tests of picking up (a debug build): stop as a crash would once this many inputs
    // are noted.
    let stop_after = std::env::var("RVR_STOP_AFTER").ok().filter(|_| cfg!(debug_assertions));
    let stop_after: Option<usize> = stop_after.and_then(|n| n.parse().ok());
    let (ticking, ended) = std::sync::mpsc::channel::<()>();
    let crashed = std::thread::scope(|scope| {
        let keeper = scope.spawn(move || if shared.carries { shared.journal.keep_up(shared.spool, crate::journal::EVERY, ended) });
        let readers: Vec<_> = (0..workers.min(todo.len()).max(1))
            .map(|_| {
                scope.spawn(|| loop {
                    let Some(&at) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    if watch.stop.load(Ordering::Acquire) {
                        break;
                    }
                    let input = &work[at];
                    let taken = take(shared, &fields[input.sample], input, input.order);
                    match &taken {
                        Ok(taken) => {
                            // Building on an earlier file, its holes are carried, not found again.
                            let holes = taken.record["holes"].as_array().filter(|_| input.kept.is_none());
                            let holes: &[Value] = holes.map_or(&[], Vec::as_slice);
                            // A file holed for a passing reason is read again by a press that picks up.
                            if shared.carries && !holes.iter().any(|hole| crate::holes::passing(hole["kind"].as_str().unwrap_or(""))) {
                                shared.journal.finished(&crate::journal::line(at, taken, shared.spool.held(&orders(input))));
                            }
                            let n = finished.fetch_add(1, Ordering::Relaxed) + 1;
                            let name = input.file.at.rsplit('/').next().unwrap_or(&input.file.at);
                            println!(
                                "  {n:>w$}/{}  {:>11} entries  {:>9} kept   {name}",
                                work.len(),
                                count(taken.entries),
                                count(taken.rows as u64),
                                w = work.len().to_string().len()
                            );
                            for hole in holes {
                                println!("    hole ({}): {}", hole["kind"].as_str().unwrap_or(""), hole["why"].as_str().unwrap_or(""));
                            }
                            if stop_after == Some(n) {
                                match shared.journal.checkpoint(shared.spool) {
                                    Ok(n) => println!("Stopped with {n} files done, as RVR_STOP_AFTER asks."),
                                    Err(why) => println!("Stopped as RVR_STOP_AFTER asks, but {why}."),
                                }
                                std::process::exit(1);
                            }
                        }
                        Err(_) => watch.stop.store(true, Ordering::Release),
                    }
                    done.lock().unwrap_or_else(|e| e.into_inner())[at] = Some(taken);
                })
            })
            .collect();
        // Joined by hand: a scope left with a panicked thread would panic in turn.
        let crashed = readers
            .into_iter()
            .fold(false, |crashed, reader| reader.join().is_err() || crashed);
        drop(ticking);
        keeper.join().is_err() || crashed
    });
    // A bug in rvr, which another press may get past: what was read is kept.
    if crashed {
        return Err(Failed::new(Fault::Bug, "a file reader stopped unexpectedly (a bug in rvr: please report it with the command you ran)"));
    }
    let done = done.into_inner().unwrap_or_else(|e| e.into_inner());
    let errors = || done.iter().flatten().filter_map(|t| t.as_ref().err());
    // A mistake in the study comes first; then the network gone, which is no file's fault: the
    // press stops, and keeps what it read.
    if let Some(mistake) = errors().find(|e| e.fault == Fault::Mistake) {
        return Err(mistake.clone());
    }
    if let Some(why) = watch.gone() {
        return Err(Failed::new(Fault::Save, why));
    }
    if let Some(error) = errors().find(|e| e.fault != Fault::Stopped) {
        return Err(error.clone());
    }
    if let Some(error) = errors().next() {
        return Err(error.clone());
    }
    done.into_iter()
        .collect::<Option<Result<Vec<_>, _>>>()
        .ok_or_else(|| Failed::from("a file was left unread".to_string()))?
}

/// What a press counts over every input.
struct Tally {
    read: u64,
    /// Every hole, with its sample.
    holes: Vec<(String, Value)>,
    cutflow: Vec<u64>,
    /// The `data.files` records, in order.
    files: Vec<Value>,
}

/// How many events one sample kept, and its file records, checking each column keeps one kind.
fn joined(
    sample: &Locked,
    inputs: &[&Input],
    taken: &mut impl Iterator<Item = Taken>,
    outputs: &[Output],
    kinds: &mut [Option<(Kind, bool)>],
    tally: &mut Tally,
) -> Result<(usize, Map<String, Value>, u64), String> {
    let (mut rows, mut records, mut missing) = (0, Map::new(), 0);
    for input in inputs {
        let taken = taken.next().ok_or("a file was left unread")?;
        for ((index, found), (name, _, _)) in taken.kinds.into_iter().enumerate().zip(outputs) {
            if *kinds[index].get_or_insert(found) != found {
                return Err(format!(
                    "{name} is not the same kind of column in {} as in the files before it",
                    input.file.at
                ));
            }
        }
        for (total, kept) in tally.cutflow.iter_mut().zip(&taken.cutflow) {
            *total += kept;
        }
        tally.read += taken.entries;
        missing += taken.missing;
        for hole in taken.record["holes"].as_array().into_iter().flatten() {
            tally.holes.push((sample.name.clone(), hole.clone()));
        }
        tally
            .files
            .push(json!({"sample": sample.name, "path": input.file.at, "rows": taken.rows}));
        records.insert(input.file.at.clone(), taken.record);
        rows += taken.rows;
    }
    Ok((rows, records, missing))
}

/// A whole number with thousands separated, as rvr prints them.
pub(crate) fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What built this rvr, as `made.toolchain` records it.
pub fn toolchain() -> Value {
    json!({
        "rvr": env!("CARGO_PKG_VERSION"),
        "rowver": rowver::VERSION.to_string(),
        "commit": env!("RVR_COMMIT"),
        "rustc": env!("RVR_RUSTC"),
        "target": env!("RVR_TARGET"),
    })
}

/// Connect to what the files need: the storage behind the grid proxy for remote files, and Rucio
/// for the files of central datasets. With no remote file, nothing; with no usable proxy, the
/// reason a remote file cannot be read.
pub(crate) fn connect(
    lock: &Lock,
    files: &[(usize, &File)],
    watch: &Arc<Watch>,
) -> Result<(Option<Storage>, Option<Datasets>, String), String> {
    let remote = files
        .iter()
        .any(|(_, f)| f.at.contains("://") || f.lfn.is_some());
    let mut datasets = None;
    let (storage, offline) = if remote {
        let proxy = crate::samples::proxy();
        // Said once, however many times a command connects.
        static WARNED: std::sync::Once = std::sync::Once::new();
        if let Some(short) = &proxy.short {
            WARNED.call_once(|| eprintln!("rvr: {short}"));
        }
        let catalog: Vec<(&File, &str)> = files
            .iter()
            .filter(|(_, f)| f.lfn.is_some())
            .map(|&(sample, f)| (f, lock.sample[sample].asked.first().map_or("", String::as_str)))
            .collect();
        if let Some((file, _)) = catalog.first() {
            let path = proxy
                .path
                .as_ref()
                .ok_or_else(|| format!("{} is in a central dataset. {}", file.at, proxy.said))?;
            let ca = proxy.ca.clone().map(std::path::PathBuf::from);
            let transport: Arc<dyn grid_catalog::Transport> =
                Arc::new(GridHttps::new(path.into(), ca));
            let asked = std::time::Instant::now();
            let server = Server::cms();
            datasets = Some(Datasets::ask(&Rucio::new(server.clone(), transport), &catalog)?);
            watch.heard_from(&format!("{}:443", server.host()));
            println!(
                "Rucio said where the {} can be read in {:.1} s.",
                some(catalog.len(), "dataset file", "dataset files"),
                asked.elapsed().as_secs_f64()
            );
        }
        match Storage::new(proxy.path.as_deref(), proxy.ca.as_deref()) {
            Ok(mut storage) => {
                storage.waits = READING;
                storage.watch = watch.clone();
                (Some(storage), proxy.said)
            }
            Err(why) => (None, why),
        }
    } else {
        (None, String::new())
    };
    Ok((storage, datasets, offline))
}

/// What a press builds on, besides what it reads.
pub enum On<'a> {
    /// A full press.
    Nothing,
    /// The file at the target, for its kept events and every column whose definition is the same,
    /// when the same study, cuts and event choice pressed it.
    Earlier,
    /// The file at the target but for its holes of these kinds: a fill.
    Holes(&'a [String]),
    /// A full press that is not picked up: what it read goes when it fails (rvr reproduce).
    Once,
}

/// Press a study's texts: check, read, keep, write `target` (through a temporary file beside
/// it), building on what `on` says. Relative local paths in the lock are read from `base`;
/// `events` says what each sample reads. How many holes the file has.
pub fn press(
    texts: &Texts,
    base: &Path,
    study: &str,
    target: &Path,
    events: &dyn Fn(&Locked) -> Result<Events, String>,
    workers: usize,
    on: On,
) -> Result<usize, String> {
    let started = std::time::Instant::now();
    let mut lock: Lock = toml::from_str(&texts.lock)
        .map_err(|e| format!("samples.lock is not a lock rvr wrote: {e}"))?;
    let source = &texts.recipe;
    let recipe = read_recipe(source)?;
    let asked = plan(&mut lock, events)?;
    if let Some(sample) = lock.sample.iter().find(|s| s.metadata.contains_key("events_missing")) {
        return Err(format!("{} has a key events_missing in samples.toml, a name rvr press gives what a file with holes misses: give it another name, and resolve again.", sample.name));
    }
    let outputs = outputs(&recipe);
    let filling = match on {
        On::Holes(kinds) => Some(crate::fill::plan(target, texts, &lock, &asked, kinds)?),
        _ => None,
    };
    let mut earlier = match matches!(on, On::Earlier)
        .then(|| crate::reuse::earlier(target, texts, &recipe, &lock, &asked, &outputs))
        .flatten()
    {
        Some(Ok(earlier)) => Some(earlier),
        Some(Err(why)) => {
            println!(
                "Pressing every column: {} cannot be built on, as {why}.",
                target.display()
            );
            None
        }
        None => None,
    };
    // With an earlier file, only the columns it cannot give are read, for the events it kept:
    // the cuts are not run again.
    let reading: Vec<Output> = match &earlier {
        Some(earlier) => outputs
            .iter()
            .zip(&earlier.reused)
            .filter(|(_, reused)| !**reused)
            .map(|(output, _)| *output)
            .collect(),
        None => outputs.clone(),
    };
    // A file that holds the study as it is, but for its holes, is filled, not pressed again.
    let same = |earlier: &&crate::reuse::Earlier| {
        reading.is_empty() && earlier.file().header()["made"]["recipe"]["text"] == texts.recipe.as_str()
    };
    if let Some(earlier) = earlier.as_ref().filter(same) {
        let holes = rowver::rvr::holes(earlier.file().header()).len();
        if holes > 0 {
            println!(
                "{} holds this study as it is, with {}: rvr press --fill reads them again (rvr press --full presses everything again).",
                target.display(),
                some(holes, "hole", "holes")
            );
            return Ok(holes);
        }
    }
    let uncut = earlier.as_ref().map(|_| {
        let names: Vec<&String> = reading.iter().map(|(n, _, _)| *n).collect();
        crate::reuse::only(&recipe, &names)
    });
    let reader = uncut.as_ref().unwrap_or(&recipe);
    let mut work: Vec<Input> = lock
        .sample
        .iter()
        .enumerate()
        .flat_map(|(sample, s)| {
            let entries: Vec<u64> = s.file.iter().map(|f| f.entries.unwrap_or(0)).collect();
            let chosen: Vec<Option<Vec<u64>>> = match asked[sample] {
                Events::Random { n, seed } => rowver::choice::per_file(&entries, n, seed)
                    .into_iter()
                    .map(Some)
                    .collect(),
                _ => vec![None; entries.len()],
            };
            s.file
                .iter()
                .zip(chosen)
                .filter(|(f, _)| f.stop != Some(0))
                .map(move |(file, chosen)| Input {
                    sample,
                    tree: &s.tree,
                    file,
                    chosen,
                    kept: None,
                    before: None,
                    ranges: None,
                    order: 0,
                })
        })
        .collect();
    for (order, input) in work.iter_mut().enumerate() {
        input.order = order;
    }
    let kept: usize = earlier
        .iter()
        .flat_map(|e| e.kept.values())
        .map(Vec::len)
        .sum();
    for input in &mut work {
        if let Some(earlier) = &mut earlier {
            let key = (
                lock.sample[input.sample].name.clone(),
                input.file.at.clone(),
            );
            let record = &earlier.file().header()["made"]["files"][&key.0]["files"][&key.1];
            input.before = Some(record.clone()).filter(|r| !r.is_null());
            input.kept = Some(earlier.kept.remove(&key).unwrap_or_default());
        }
    }
    // A fill reads only the holes asked; every other row is the file's own.
    let (holes, laid) = match &filling {
        Some(filling) => crate::fill::laid(&work, &lock, &filling.files, &filling.entries)?,
        None => (Vec::new(), Vec::new()),
    };
    let reads = match &filling {
        Some(_) => &holes,
        None => &work,
    };
    // The press is staged in a folder beside its target with a journal of the inputs it finished,
    // so that a press stopped part way is picked up where it stopped, when it is this one.
    let identity = json!({
        "recipe": write::text_checksum(&texts.recipe),
        "samples": write::text_checksum(&texts.samples),
        "lock": write::text_checksum(&texts.lock),
        "events": asked.iter().map(|e| e.record()).collect::<Vec<_>>(),
        "reading": reading.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>(),
        "on": match (&filling, &earlier, &on) {
            (Some(filling), _, On::Holes(kinds)) => json!({"fill": filling.file.header()["data"]["sha256"], "kinds": kinds}),
            (_, Some(earlier), _) => json!({"earlier": earlier.file().header()["data"]["sha256"]}),
            _ => Value::Null,
        },
        "toolchain": toolchain(),
        "inputs": reads.len(),
    });
    let staged = crate::journal::Staged::lock(target)?;
    // A press is picked up unless it is a reproduction, or its build names no one build.
    let carries = !matches!(on, On::Once) && crate::reuse::same_build(&json!({ "toolchain": toolchain() })).is_ok();
    let mut picked = (!matches!(on, On::Once)).then(|| crate::journal::pick_up(&staged, &identity)).flatten();
    let resumed = picked.as_mut().map(|p| std::mem::take(&mut p.taken)).unwrap_or_default();
    let picked_up = !resumed.is_empty();
    let watch = Arc::new(Watch::press(crate::samples::remote::QUIET, crate::samples::remote::AGAIN));
    let files: Vec<(usize, &File)> = reads
        .iter()
        .enumerate()
        .filter(|(at, _)| !resumed.contains_key(at))
        .map(|(_, i)| (i.sample, i.file))
        .collect();
    let (storage, datasets, offline) = connect(&lock, &files, &watch)?;
    let (spool, journal) = crate::journal::stage(target, &staged, &identity, picked)?;
    let read: u64 = lock.sample.iter().map(|s| s.events_read).sum();
    match &earlier {
        _ if filling.is_some() => {
            let entries = crate::fill::ranged(reads);
            let holes = reads.iter().flat_map(|i| i.ranges.iter().flatten()).count();
            println!(
                "Filling {}: reading {} of {}, {} entries, {} at a time; every other row is the file's own:",
                target.display(),
                some(holes, "hole", "holes"),
                some(reads.len(), "file", "files"),
                count(entries),
                workers.min(reads.len()).max(1)
            );
        }
        Some(_) => {
            let names: Vec<&str> = reading.iter().map(|(n, _, _)| n.as_str()).collect();
            println!(
                "{} has the events this recipe keeps ({}) and {} of its {} columns.",
                target.display(),
                count(kept as u64),
                outputs.len() - reading.len(),
                outputs.len()
            );
            match names.is_empty() {
                true => println!("Nothing to read: every column is copied from it."),
                false => println!(
                    "Reading only {} for those events, {} {} at a time:",
                    names.join(", "),
                    some(work.len(), "file", "files"),
                    workers.min(work.len()).max(1)
                ),
            }
        }
        None => println!(
            "Pressing {}, {}, {} entries, {} at a time:",
            some(lock.sample.len(), "sample", "samples"),
            some(work.len(), "file", "files"),
            count(read),
            workers.min(work.len()).max(1)
        ),
    }
    let mentions = reader.mentions();
    let shared = Shared {
        recipe: reader,
        outputs: &reading,
        reach: Reach {
            storage: storage.as_ref(),
            datasets: datasets.as_ref(),
            offline: &offline,
            dir: base,
            watch,
            whole: true,
        },
        mentions: &mentions,
        spool: &spool,
        journal: &journal,
        carries,
    };
    // What was read made durable, and the folder kept when that holds any input.
    let save = || {
        let saved = carries.then(|| journal.checkpoint(&spool));
        staged.keep.set(!matches!(saved, None | Some(Ok(0))));
        saved
    };
    let read = match take_all(&shared, &lock, reads, workers, resumed) {
        Ok(read) => read,
        // A mistake in the study, or a file that will not be read as it is (whose remedy is
        // another press), is pressed again from the start; anything else keeps what was read for
        // the next press to pick up.
        Err(failed) if failed.fault == Fault::Mistake || failed.fault.kind().is_some_and(|k| !crate::holes::passing(k)) => {
            return Err(failed.why)
        }
        Err(failed) => return Err(ended(failed, target, &save(), reads.len())),
    };
    // What the last inputs gave is made durable before the file is written, when that is worth a
    // sync: a press of a few seconds is read again soon enough.
    let worth = picked_up || started.elapsed() >= crate::journal::EVERY;
    if carries && worth {
        let _ = journal.checkpoint(&spool);
    }
    let staged_file = staged.folder.join("pressed.rvr");
    // Every input is read: a failure from here on in writing (the disk, the key, the spool, the
    // copy of a filled file's rows, which `fits` checked to hold every column) keeps what was
    // read. The others (a column of no known kind, nothing read, a fill that cannot merge) the
    // same reads would meet again, so the press starts over.
    let stuck = |why: String| format!("{} Mend it, then {}.", sentence(&why), again(&save(), reads.len()));
    let mut taken = match &filling {
        // A fill that read none of its holes again, each failing as before, leaves the file.
        Some(filling) if read.iter().map(|t| t.missing).sum::<u64>() == crate::fill::ranged(reads) && crate::fill::same_kinds(filling, &lock, reads, &read) => {
            let holes = rowver::rvr::holes(filling.file.header()).len();
            let asked = reads.iter().flat_map(|i| i.ranges.iter().flatten()).count();
            println!("None of the {} could be read: {} is as it was.", some(asked, "hole", "holes"), target.display());
            return Ok(holes);
        }
        Some(filling) => {
            let old = crate::fill::spool_old(&spool, &filling.file, &reading, &laid).map_err(&stuck)?;
            crate::fill::merged(&work, &lock, filling, &laid, read, &old)?
        }
        None => read,
    }
    .into_iter();
    let mut tally = Tally {
        read: 0,
        holes: Vec::new(),
        cutflow: vec![0; recipe.keep.len()],
        files: Vec::new(),
    };
    let (mut samples, mut made_files) = (Vec::new(), Map::new());
    let mut kinds: Vec<Option<(Kind, bool)>> = vec![None; reading.len()];
    let mut missing = Vec::new();
    for (index, sample) in lock.sample.iter().enumerate() {
        let inputs: Vec<&Input> = work.iter().filter(|i| i.sample == index).collect();
        let (rows, records, lacks) = joined(
            sample, &inputs, &mut taken, &reading, &mut kinds, &mut tally,
        )?;
        samples.push(write::Sample {
            name: sample.name.clone(),
            label: sample.label.clone(),
            rows,
        });
        let mut metadata = sample.metadata.clone();
        metadata.insert("events_read".into(), (sample.events_read - lacks).into());
        metadata.insert("events_total".into(), sample.events_total.into());
        missing.push(lacks);
        made_files.insert(
            sample.name.clone(),
            json!({"events": asked[index].record(), "files": records, "metadata": metadata}),
        );
    }
    // What each sample misses is said only in a file with holes, and then on every sample.
    if !tally.holes.is_empty() {
        for (sample, lacks) in lock.sample.iter().zip(&missing) {
            made_files[&sample.name]["metadata"]["events_missing"] = (*lacks).into();
        }
    }
    for line in datasets.iter().flat_map(Datasets::summary) {
        println!("  {line}");
    }
    if kinds.iter().any(Option::is_none) && tally.read == 0 {
        if let Some((_, hole)) = tally.holes.first() {
            return Err(format!("nothing could be read: every entry the press chose is in a hole, so no column's kind is known, and {} was not written. The first: {}", target.display(), hole["why"].as_str().unwrap_or("")));
        }
    }
    for ((column, _, _), kind) in reading.iter().zip(&kinds) {
        if kind.is_none() {
            return Err(format!("no sample has an event to take {column} from, so its kind is unknown: press more events"));
        }
    }
    let failed = |e: rowver::Error| e.to_string();
    let entry = match spool.take(reading.len()).map_err(|e| stuck(e.to_string()))? {
        Some(spooled) => write::Source::Spooled(spooled),
        None => write::Source::Values(write::Values::Flat(write::array::<i64>(&[]))),
    };
    let rows = write::Rows {
        samples,
        files: tally.files,
        entry,
        labels: Labels::new(),
    };
    // Each column is copied from the earlier file, or packed from what the spool holds of it,
    // which is nothing when no sample kept an event of it.
    let mut fresh = reading
        .iter()
        .map(|(name, _, _)| *name)
        .zip(&kinds)
        .enumerate();
    let mut columns = Vec::new();
    for (index, (name, _, _)) in outputs.iter().enumerate() {
        if let Some(earlier) = earlier.as_ref().filter(|e| e.reused[index]) {
            let copied = write::Source::Copied(earlier.file(), (*name).clone());
            columns.push(((*name).clone(), copied));
            continue;
        }
        let (at, (_, kind)) = fresh.next().ok_or("a column was left unread")?;
        let source = match spool.take(at).map_err(|e| stuck(e.to_string()))? {
            Some(spooled) => write::Source::Spooled(spooled),
            None => {
                let (kind, listed) = kind.expect("every kind was found");
                let buffer = NumericBuffer {
                    kind: kind.name().into(),
                    count: 0,
                    bytes: Vec::new(),
                };
                write::Source::Values(match listed {
                    true => write::Values::Jagged {
                        counts: Vec::new(),
                        content: buffer,
                    },
                    false => write::Values::Flat(buffer),
                })
            }
        };
        columns.push(((*name).clone(), source));
    }
    let (mut data, laid) = write::stream(rows, columns, workers, &staged_file)
        .map_err(|e| stuck(format!("{} could not be written: {e}", target.display())))?;
    let built_on = earlier.is_some();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let text = |text: &str| json!({"sha256": write::text_checksum(text), "text": text});
    let mut made = Map::new();
    made.insert("study".into(), study.into());
    if let Some(title) = &recipe.title {
        made.insert("title".into(), title.as_str().into());
    }
    made.insert("built".into(), voms_proxy::utc(now).into());
    made.insert(
        "by".into(),
        concat!("rvr ", env!("CARGO_PKG_VERSION")).into(),
    );
    made.insert("toolchain".into(), toolchain());
    let mut pressed = text(source);
    pressed["format"] = "rvr-recipe".into();
    pressed["columns"] = outputs.iter().map(|(n, _, _)| n.as_str()).collect();
    made.insert("recipe".into(), pressed);
    made.insert("samples".into(), text(&texts.samples));
    made.insert("lock".into(), text(&texts.lock));
    made.insert("files".into(), Value::Object(made_files));
    let mut made = Value::Object(made);
    let names: Vec<&str> = lock.sample.iter().map(|s| s.name.as_str()).collect();
    let written: Vec<&str> = outputs.iter().map(|(n, _, _)| n.as_str()).collect();
    data["sample_cols"] = write::sample_cols(&names, &made["files"], &written)
        .map_err(failed)?
        .into();
    made["sha256"] = write::made_checksum(&made).map_err(failed)?.into();
    let rows = data["rows"].as_u64().unwrap_or(0);
    let mut header = json!({"rowver": rowver::VERSION, "data": data, "made": made});
    let signer = crate::key::secret().map_err(&stuck)?;
    if let Some(secret) = &signer {
        rowver::sign::sign(&mut header, secret).map_err(|e| stuck(format!("the file could not be signed with your key: {e}")))?;
    }
    // A press of more than a few seconds is on disk before what would pick it up goes.
    let durable = || match worth {
        true => std::fs::File::open(&staged_file).and_then(|f| f.sync_all()),
        false => Ok(()),
    };
    let written = write::write_laid(&staged_file, &header, laid)
        .map_err(|e| e.to_string())
        .and_then(|()| durable().map_err(|e| e.to_string()))
        .and_then(|()| std::fs::rename(&staged_file, target).map_err(|e| e.to_string()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged_file);
        return Err(stuck(format!("{} could not be written: {e}", target.display())));
    }
    if worth {
        let _ = std::fs::File::open(crate::journal::parent(target)).and_then(|f| f.sync_all());
    }
    drop((journal, spool));
    drop(staged);
    let bytes = std::fs::metadata(target).map_or(0, |m| m.len());
    if !built_on {
        let read = match missing.iter().sum::<u64>() {
            _ if filling.is_some() => format!("{} entries in the holes", count(tally.read)),
            0 => format!("{} entries", count(tally.read)),
            lacks => format!("{} of {} entries", count(tally.read), count(tally.read + lacks)),
        };
        match recipe.keep.is_empty() {
            true => println!("Read {read}; with no cuts, every one is kept."),
            false => println!("Read {read}; kept after each cut:"),
        }
        for (cut, kept) in recipe.keep.iter().zip(&tally.cutflow) {
            println!("  {:30} {:>13}", cut.name, count(*kept));
        }
    }
    if let Some(secret) = &signer {
        let key = rowver::sign::public_key(secret);
        println!("Signed with your key {}", crate::key::short(&key));
    }
    let with = match tally.holes.len() {
        0 => ".".to_string(),
        n => format!(", with {}:", some(n, "hole", "holes")),
    };
    println!(
        "Wrote {} ({} rows, {}, {:.1} MB) in {:.1} s{with}",
        target.display(),
        count(rows),
        some(outputs.len(), "column", "columns"),
        bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    for (sample, hole) in &tally.holes {
        let kind = hole["kind"].as_str().unwrap_or("");
        println!("  {kind:11} {sample}  {}", hole["why"].as_str().unwrap_or(""));
    }
    if !tally.holes.is_empty() {
        println!("rvr press --fill reads them again once they can be read.");
    }
    Ok(tally.holes.len())
}

/// `text` ending as a sentence does.
fn sentence(text: &str) -> String {
    match text.ends_with(['.', '?', '!']) {
        true => text.to_string(),
        false => format!("{text}."),
    }
}

/// What pressing again does after a press stopped: carry on from the inputs `saved` of `inputs`.
fn again(saved: &Option<Result<usize, String>>, inputs: usize) -> String {
    match saved {
        None | Some(Ok(0)) => "press again".into(),
        Some(Ok(done)) => format!("press again, which carries on from the {done} of {} done (rvr press --restart starts over)", some(inputs, "file", "files")),
        Some(Err(lost)) => format!("press again, which carries on from what this press saved before ({lost}; rvr press --restart starts over)"),
    }
}

/// What a press that stopped part way says: why, and what pressing again does.
fn ended(failed: Failed, target: &Path, saved: &Option<Result<usize, String>>, inputs: usize) -> String {
    let why = match failed.fault {
        Fault::Save => format!("{} It is not a file's fault, so no file was made a hole of it: fix it, then", sentence(&failed.why)),
        Fault::Bug => format!("{} Then", sentence(&failed.why)),
        _ => format!("{} Once it can be read,", sentence(&failed.why)),
    };
    format!("{why} {}. {} was not written.", again(saved, inputs), target.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source of known bytes that records every range asked of it.
    struct Asked(Vec<u8>, std::sync::Arc<Mutex<Vec<(u64, usize)>>>);

    impl ByteSource for Asked {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
            self.1.lock().unwrap().push((offset, buf.len()));
            buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
            Ok(())
        }

        fn describe(&self) -> String {
            "memory".into()
        }
    }

    #[test]
    fn held_ranges_answer_reads_and_only_the_rest_is_fetched() {
        let bytes: Vec<u8> = (0..=255).cycle().take(200_000).collect();
        let log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let held = Held {
            inner: Box::new(Asked(bytes.clone(), log.clone())),
            held: Mutex::new(Vec::new()),
            again: None,
            seen: None,
            shown: String::new(),
        };
        held.hold(vec![(1_000, 100), (1_200, 100), (100_000, 50)])
            .unwrap();
        // The two near spans were joined into one; the far one stayed apart.
        assert_eq!(*log.lock().unwrap(), [(1_000, 300), (100_000, 50)]);
        log.lock().unwrap().clear();

        // Held bytes come from memory; a read reaching past them fetches only what is missing.
        let read = held.read_ranges(&[(1_050, 200), (100_000, 80)]).unwrap();
        assert_eq!(read[0], bytes[1_050..1_250]);
        assert_eq!(read[1], bytes[100_000..100_080]);
        assert_eq!(*log.lock().unwrap(), [(100_050, 30)]);
        log.lock().unwrap().clear();

        // The next chunk keeps what it shares with the last and fetches only the rest.
        held.hold(vec![(100_000, 50), (100_050, 100)]).unwrap();
        assert_eq!(*log.lock().unwrap(), [(100_050, 100)]);
        let mut buf = vec![0; 150];
        held.read_at(100_000, &mut buf).unwrap();
        assert_eq!(buf, bytes[100_000..100_150]);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn runs_are_every_chunk_or_the_chosen_entries_cut_at_gaps() {
        assert_eq!(
            runs(0, 120_000, None, GAP),
            [(0, 50_000), (50_000, 100_000), (100_000, 120_000)]
        );
        assert_eq!(runs(0, 0, None, GAP), []);
        assert_eq!(runs(70_000, 125_000, None, GAP), [(70_000, 120_000), (120_000, 125_000)]);
        let chosen = [3, 10, 5_000, 5_001, 60_000];
        assert_eq!(
            runs(0, 70_000, Some(&chosen), GAP),
            [(3, 11), (5_000, 5_002), (60_000, 60_001)]
        );
        assert_eq!(
            runs(0, 70_000, Some(&chosen), CHUNK),
            [(3, 5_002), (60_000, 60_001)]
        );
        let dense: Vec<u64> = (0..120_000).step_by(10).collect();
        assert_eq!(
            runs(0, 120_000, Some(&dense), GAP),
            [(0, 49_991), (50_000, 99_991), (100_000, 119_991)]
        );
    }

    #[test]
    fn gaps_are_what_no_held_range_covers() {
        let held = vec![(10, vec![0; 10]), (30, vec![0; 10])];
        assert_eq!(gaps(&held, 0, 50), [(0, 10), (20, 10), (40, 10)]);
        assert_eq!(gaps(&held, 12, 5), []);
        assert_eq!(gaps(&held, 15, 20), [(20, 10)]);
        assert_eq!(gaps(&[], 5, 5), [(5, 5)]);
    }

    #[test]
    fn checksums_compare_as_numbers_and_only_when_both_are_known() {
        let file = |adler32: Option<&str>| File {
            at: "f.root".into(),
            bytes: 10,
            adler32: adler32.map(str::to_owned),
            modified: None,
            lfn: None,
            events: None,
            disk: None,
            uuid: None,
            entries: None,
            stop: None,
            take: None,
        };
        assert!(changed(&file(Some("0a1b2c3d")), Some(10), Some("a1b2c3d".into())).is_ok());
        assert!(changed(&file(None), Some(10), Some("0a1b2c3d".into())).is_ok());
        assert!(changed(&file(Some("0a1b2c3d")), Some(10), None).is_ok());
        assert!(changed(&file(Some("0a1b2c3d")), Some(10), Some("0a1b2c3e".into())).is_err());
        assert!(changed(&file(Some("0a1b2c3d")), Some(11), Some("0a1b2c3d".into())).is_err());
        // A storage that gives no size is not taken to say 0 bytes.
        assert!(changed(&file(Some("0a1b2c3d")), None, Some("0a1b2c3d".into())).is_ok());
    }
}
