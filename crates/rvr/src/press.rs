//! `rvr press`: read what a study's samples.lock names through its recipe.toml, keep the events
//! the recipe's cuts pass, and write one `.rvr` file.
//!
//! Ported from: RowVer 13.0.1 `rowver/run.py` `press`, for a recipe of cuts, copied branches and
//! computed columns.
//! `made.cart.source` holds the recipe's TOML and `made.cart.hooks` is left out, as `FORMAT.md`
//! allows for a file without marks.
//! Documentation: `FORMAT.md` (repository root) ("made", "data"); the README of rvr.
//! Differences from the Python presser: ROOT is read with hep-root (`RootFile::read_range`); the
//! inputs are the lock's files, each checked against the lock before it is read (size, adler32, ROOT
//! UUID, entries); `[column]` entries are written after the copies; and each sample's metadata
//! gains `label`, `events_read` and `events_total`. Rows keep the order of their entries, and
//! several files are read at once, each a run of entries at a time.

use crate::samples::remote::{HttpsFile, Storage};
use crate::samples::resolve::{self, adler32_file, File, Lock, Locked};
use crate::samples::spec::Events;
use hep_root::{ByteSource, RootFile, Tree};
use recipe::{Array, Columns, Constant, Expression, Kind, Recipe, Scope, Values};
use rowver::write::{self, Labels};
use rowver::NumericBuffer;
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// How many entries of a file are read and held at a time.
const CHUNK: u64 = 50_000;

/// What an input answers when it stops because another one failed.
const STOPPED: &str = "Another input failed";

pub const USAGE: &str = "usage:
  rvr press [DIR] [--events N|all] [--out FILE] [--workers N]
                                           press DIR/samples.lock through DIR/recipe.toml into
                                           DIR/<DIR's name>.rvr (DIR is . when not given)";

/// `rvr press …` with the words after `press`: the exit status.
pub fn main(args: &[&str]) -> u8 {
    match run(args) {
        Ok(()) => 0,
        Err(why) => {
            eprintln!("rvr press: {why}");
            1
        }
    }
}

fn run(args: &[&str]) -> Result<(), String> {
    let (mut dir, mut events, mut out, mut workers) = (None, None, None, None);
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
            "--events" => events = Some(given()?),
            "--out" => out = Some(PathBuf::from(given()?)),
            "--workers" => {
                let n = given()?;
                workers = Some(n.parse::<usize>().ok().filter(|n| *n > 0).ok_or(format!("--workers is how many files are read at once, not {n:?}"))?);
            }
            "--random" | "--seed" => return Err(format!("{word}: choosing events at random waits for rowver 14, which can record the choice in the file. Press the first events (--events N) or all of them (--events all).")),
            flag if flag.starts_with("--") => return Err(format!("press does not know {flag}.\n{USAGE}")),
            folder if dir.is_none() => dir = Some(PathBuf::from(folder)),
            _ => return Err(format!("press takes one study folder.\n{USAGE}")),
        }
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    let given = dir;
    let dir = given
        .canonicalize()
        .map_err(|e| format!("{}: {e}", given.display()))?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("study")
        .to_string();
    // Named as the folder was given, so that what is printed is what one would type.
    let target = out.unwrap_or_else(|| match given.as_os_str() == "." {
        true => PathBuf::from(format!("{name}.rvr")),
        false => given.join(format!("{name}.rvr")),
    });
    let workers =
        workers.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    press(&dir, &name, &target, events, workers)
}

/// Read a study's two files and refuse a lock that no longer matches its samples.toml.
fn inputs(dir: &Path) -> Result<(Lock, String), String> {
    let read = |file: &str| {
        std::fs::read_to_string(dir.join(file)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound if file == "samples.lock" => format!(
                "{} has no samples.lock: run rvr samples resolve samples.toml there first.",
                dir.display()
            ),
            _ => format!("{}: {e}", dir.join(file).display()),
        })
    };
    let lock: Lock = toml::from_str(&read("samples.lock")?)
        .map_err(|e| format!("samples.lock is not a lock rvr wrote: {e}"))?;
    match std::fs::read(dir.join("samples.toml")) {
        Ok(now) if crate::samples::sha256(&now) != lock.samples_toml_sha256 => {
            return Err(format!("samples.toml has changed since samples.lock was written ({}). Run rvr samples resolve samples.toml, review the new lock, then press.", lock.resolved));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("There is no samples.toml beside samples.lock: pressing the lock as it is.")
        }
        Err(e) => {
            return Err(format!(
                "samples.toml cannot be read to check the lock against it: {e}"
            ))
        }
    }
    Ok((lock, read("recipe.toml")?))
}

/// What `--events` asks of a sample, or what samples.toml asked when it is not given.
fn wanted(sample: &Locked, events: Option<&str>) -> Result<Events, String> {
    let want = match events {
        None => resolve::asked(sample),
        Some("all") => Events::All,
        Some(n) => Events::First(
            n.replace('_', "")
                .parse()
                .map_err(|_| format!("--events is a number of events or all, not {n:?}"))?,
        ),
    };
    if matches!(want, Events::Random { .. }) {
        return Err(format!("{}: samples.toml chooses its events at random, which waits for rowver 14 (the file must be able to record the choice). Give --events N or --events all, or set choose = \"first\".", sample.name));
    }
    Ok(want)
}

/// The recipe a press runs, or what stops it.
fn read_recipe(source: &str) -> Result<Recipe, String> {
    let recipe = Recipe::read(source).map_err(|e| format!("recipe.toml: {}", e.0))?;
    if recipe.copy.is_empty() && recipe.columns.is_empty() {
        return Err("recipe.toml stores nothing: name branches under [copy] or computed columns under [column].".into());
    }
    Ok(recipe)
}

/// An output column: the name written, the expression it comes from, and whether it is a copy.
type Output<'a> = (&'a String, &'a Expression, bool);

/// One file to read, in the lock's order.
struct Input<'a> {
    sample: usize,
    tree: &'a str,
    file: &'a File,
}

/// What every input of one press shares.
struct Shared<'a> {
    recipe: &'a Recipe,
    outputs: &'a [Output<'a>],
    storage: Option<&'a Storage>,
    offline: &'a str,
    /// Every name the recipe mentions, to choose the branches fetched ahead.
    mentions: &'a BTreeSet<String>,
    /// The study folder, which a relative path in the lock is relative to.
    dir: &'a Path,
    /// Set once any input has failed, so the others stop reading.
    failed: AtomicBool,
}

/// The branches of one open tree over the entries in hand, read when an expression first names them.
struct Branches<'a, S: ByteSource> {
    file: &'a RootFile<S>,
    tree: &'a Tree,
    names: &'a [String],
    first: i64,
    last: i64,
    read: RefCell<HashMap<String, Array>>,
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
    fn column(&self, name: &str) -> recipe::Result<Option<Array>> {
        if let Some(held) = self.read.borrow().get(name) {
            return Ok(Some(held.clone()));
        }
        let Some(branch) = self.tree.branch(name) else {
            return Ok(None);
        };
        let fail = |error: String| recipe::Error(format!("{name}: {error}"));
        let values = self
            .file
            .read_range(branch, self.first, self.last)
            .map_err(|e| fail(e.to_string()))?;
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

/// What one input contributed.
struct Taken {
    gathered: Vec<Gathered>,
    /// The entry number of each kept event.
    entry: Vec<i64>,
    record: Value,
    /// Entries read, before any cut.
    entries: u64,
    /// The events still kept after each cut.
    cutflow: Vec<u64>,
}

/// The metadata fields an expression can read: numbers, text, and truth values.
fn fields(metadata: &Map<String, Value>) -> BTreeMap<String, Constant> {
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
fn lets_through(cut: &recipe::Cut, scope: &Scope, entries: usize) -> Result<Vec<bool>, String> {
    let named = |error: recipe::Error| format!("[keep] {}: {}", cut.name, error.0);
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
fn kept(
    recipe: &Recipe,
    scope: &Scope,
    entries: usize,
    cutflow: &mut [u64],
) -> Result<Vec<bool>, String> {
    let mut keep = vec![true; entries];
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

/// Refuse a file that differs from what the lock recorded of it.
fn changed(file: &File, bytes: Option<u64>, adler32: Option<String>) -> Result<(), String> {
    match resolve::differs(file, bytes, adler32.as_deref()) {
        None => Ok(()),
        Some(how) => Err(format!("{how}. It has changed since samples.lock was written: if the change is wanted, run rvr samples resolve samples.toml and review the new lock.")),
    }
}

/// A source that keeps the ranges it was last told to hold, so that the basket reads of a chunk
/// are answered from memory: every basket the recipe may read of a remote file, fetched in a
/// few requests at once instead of branch after branch. A read that held ranges cover only in
/// part fetches just the rest.
struct Held {
    inner: Box<dyn ByteSource>,
    /// Disjoint ranges, in order of position.
    held: Mutex<Vec<(u64, Vec<u8>)>>,
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
        let fetched = self.inner.read_ranges(&missing)?;
        kept.extend(missing.iter().map(|r| r.0).zip(fetched));
        kept.sort_unstable_by_key(|(start, _)| *start);
        *self.held.lock().unwrap() = kept;
        Ok(())
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
            return self.inner.read_ranges(ranges);
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
                .zip(self.inner.read_ranges(&missing)?)
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

/// Check one input against the lock and open it, local or remote.
fn open(shared: &Shared, file: &File) -> Result<RootFile<Held>, String> {
    let at = &file.at;
    if !at.contains("://") {
        // A relative path is the lock's own: relative to the study folder.
        let path = shared.dir.join(at);
        let meta = std::fs::metadata(&path).map_err(|_| {
            format!(
                "{at} no longer exists. If it moved, run rvr samples resolve samples.toml again."
            )
        })?;
        // Read whole for its adler32 only when the lock has one to compare with.
        let adler32 = match file.adler32 {
            Some(_) => Some(adler32_file(&path).map_err(|e| format!("{at}: {e}"))?),
            None => None,
        };
        changed(file, Some(meta.len()), adler32)?;
        let local = hep_root::LocalFile::open(&path).map_err(|e| format!("{at}: {e}"))?;
        return opened(file, Box::new(local));
    }
    let storage = shared
        .storage
        .ok_or_else(|| format!("{at}: {}", shared.offline))?;
    // The storage's size and adler32 are asked while the file is opened at the size the lock
    // gives; a changed file is reported before anything the opening found.
    std::thread::scope(|scope| {
        let digest = scope.spawn(|| storage.digest(at));
        let root = HttpsFile::new(storage, at, file.bytes)
            .and_then(|remote| opened(file, Box::new(remote)));
        let (bytes, adler32) = digest
            .join()
            .map_err(|_| format!("{at}: asking its storage stopped unexpectedly"))??;
        changed(file, bytes, adler32)?;
        root
    })
}

/// Open a checked input and refuse one whose ROOT UUID is not the lock's.
fn opened(file: &File, source: Box<dyn ByteSource>) -> Result<RootFile<Held>, String> {
    let at = &file.at;
    let source = Held {
        inner: source,
        held: Mutex::new(Vec::new()),
    };
    let root = RootFile::open(source)
        .map_err(|e| format!("{at} is not a ROOT file this reader can open: {e}"))?;
    let uuid = root.header().uuid_string();
    if file.uuid.as_ref().is_some_and(|locked| *locked != uuid) {
        return Err(format!("{at} is not the file samples.lock recorded: its ROOT UUID was {}, it is now {uuid}. Run rvr samples resolve samples.toml and review the new lock.", file.uuid.as_deref().unwrap_or("?")));
    }
    Ok(root)
}

/// Read one input a run of entries at a time, keeping the events the cuts pass.
fn take(
    shared: &Shared,
    sample: &BTreeMap<String, Constant>,
    input: &Input,
) -> Result<Taken, String> {
    let Shared {
        recipe, outputs, ..
    } = *shared;
    let at = &input.file.at;
    let located = |error: String| format!("{at}: {error}");
    let file = open(shared, input.file)?;
    let tree = file
        .tree_at(input.tree)
        .map_err(|e| located(format!("its tree {:?}: {e}", input.tree)))?;
    let total = tree.entries.max(0) as u64;
    if input.file.entries.is_some_and(|locked| locked != total) {
        return Err(located(format!("its tree {} has {total} entries, not the {} samples.lock recorded. Run rvr samples resolve samples.toml and review the new lock.", input.tree, input.file.entries.unwrap())));
    }
    let entries = input.file.stop.map_or(total, |stop| stop.min(total));
    let names: Vec<String> = tree
        .walk()
        .into_iter()
        .map(|(_, b)| b.name.clone())
        .collect();
    let mut sample = sample.clone();
    sample.insert("file".into(), Constant::Text(at.clone()));
    let remote = at.contains("://");
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
    let (mut first, mut entry) = (0_u64, Vec::new());
    while first < entries {
        if shared.failed.load(Ordering::Acquire) {
            return Err(STOPPED.into());
        }
        let last = entries.min(first + CHUNK);
        let count = (last - first) as usize;
        if remote {
            // Fetching ahead only saves round trips; a branch that cannot be fetched is reported
            // when an expression asks for it.
            let _ = file
                .source()
                .inner()
                .hold(spans(&wanted, first as i64, last as i64));
        }
        let branches = Branches {
            file: &file,
            tree: &tree,
            names: &names,
            first: first as i64,
            last: last as i64,
            read: RefCell::new(HashMap::new()),
        };
        let scope = Scope {
            columns: &branches,
            sample: &sample,
            objects: &recipe.objects,
            collections: &recipe.collections,
            helpers: &recipe.helpers,
        };
        let keep = kept(recipe, &scope, count, &mut cutflow).map_err(located)?;
        for (index, (column, from, plain)) in outputs.iter().enumerate() {
            let array = if *plain {
                copied(&scope, from)
            } else {
                scope
                    .evaluate(from)
                    .and_then(|value| recipe::per_event(value, count))
                    .map_err(|e| e.0)
            };
            let array = stored(
                recipe,
                column,
                array.map_err(|e| located(format!("{column}: {e}")))?,
            );
            let (kind, listed) = match &array {
                Array::Event(values) => (values.kind, false),
                Array::Object { values, .. } => (values.kind, true),
                Array::Constant(_) => {
                    return Err(located(format!("{column} is a fixed value, not a column")))
                }
            };
            let held = gathered[index].get_or_insert_with(|| Gathered {
                name: (*column).clone(),
                kind,
                bytes: Vec::new(),
                counts: listed.then(Vec::new),
            });
            gather(held, array, &keep, at)?;
        }
        entry.extend(
            (first..last)
                .filter(|event| keep[(event - first) as usize])
                .map(|event| event as i64),
        );
        first = last;
    }
    let mut record =
        json!({"tree": input.tree, "uuid": file.header().uuid_string(), "entries": total});
    if let Some(stop) = input.file.stop {
        record["stop"] = stop.into();
    }
    Ok(Taken {
        gathered: gathered.into_iter().flatten().collect(),
        entry,
        record,
        entries,
        cutflow,
    })
}

/// Read every input with a few workers; what each contributed, in the lock's order.
fn take_all(
    shared: &Shared,
    lock: &Lock,
    work: &[Input],
    workers: usize,
) -> Result<Vec<Taken>, String> {
    let fields: Vec<BTreeMap<String, Constant>> =
        lock.sample.iter().map(|s| fields(&s.metadata)).collect();
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<Option<Result<Taken, String>>>> =
        Mutex::new(work.iter().map(|_| None).collect());
    let finished = AtomicUsize::new(0);
    let crashed = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..workers.min(work.len()).max(1))
            .map(|_| {
                scope.spawn(|| loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    if at >= work.len() || shared.failed.load(Ordering::Acquire) {
                        break;
                    }
                    let input = &work[at];
                    let taken = take(shared, &fields[input.sample], input);
                    match &taken {
                        Ok(taken) => {
                            let n = finished.fetch_add(1, Ordering::Relaxed) + 1;
                            println!(
                                "  {n:>w$}/{}  {:>11} entries  {:>9} kept   {}",
                                work.len(),
                                count(taken.entries),
                                count(taken.entry.len() as u64),
                                input.file.at.rsplit('/').next().unwrap_or(&input.file.at),
                                w = work.len().to_string().len()
                            );
                        }
                        Err(_) => shared.failed.store(true, Ordering::Release),
                    }
                    done.lock().unwrap_or_else(|e| e.into_inner())[at] = Some(taken);
                })
            })
            .collect();
        // Joined by hand: a scope left with a panicked thread would panic in turn.
        readers
            .into_iter()
            .fold(false, |crashed, reader| reader.join().is_err() || crashed)
    });
    if crashed {
        return Err("a file reader stopped unexpectedly (a bug in rvr: please report it with the command you ran)".into());
    }
    let done = done.into_inner().unwrap_or_else(|e| e.into_inner());
    let errors = || done.iter().flatten().filter_map(|t| t.as_ref().err());
    if let Some(error) = errors().find(|e| *e != STOPPED).or(errors().next()) {
        return Err(error.clone());
    }
    done.into_iter()
        .collect::<Option<Result<Vec<_>, _>>>()
        .ok_or("a file was left unread")?
}

/// What a press counts over every input.
struct Tally {
    read: u64,
    cutflow: Vec<u64>,
    /// The `data.files` records, in order.
    files: Vec<Value>,
}

/// Join one sample's inputs into its piece of the file, checking each column keeps one kind.
fn joined(
    sample: &Locked,
    inputs: &[&Input],
    taken: &mut impl Iterator<Item = Taken>,
    outputs: &[Output],
    kinds: &mut [Option<(Kind, bool)>],
    tally: &mut Tally,
) -> Result<(Joined, Map<String, Value>), String> {
    let mut gathered: Vec<Option<Gathered>> = outputs.iter().map(|_| None).collect();
    let (mut entry, mut records) = (Vec::new(), Map::new());
    for input in inputs {
        let taken = taken.next().ok_or("a file was left unread")?;
        for (index, piece) in taken.gathered.into_iter().enumerate() {
            let found = (piece.kind, piece.counts.is_some());
            if *kinds[index].get_or_insert(found) != found {
                return Err(format!(
                    "{} is not the same kind of column in {} as in the files before it",
                    piece.name, input.file.at
                ));
            }
            match &mut gathered[index] {
                Some(held) => {
                    held.bytes.extend(piece.bytes);
                    if let (Some(counts), Some(more)) = (&mut held.counts, piece.counts) {
                        counts.extend(more);
                    }
                }
                empty => *empty = Some(piece),
            }
        }
        for (total, kept) in tally.cutflow.iter_mut().zip(&taken.cutflow) {
            *total += kept;
        }
        tally.read += taken.entries;
        tally
            .files
            .push(json!({"sample": sample.name, "path": input.file.at, "rows": taken.entry.len()}));
        records.insert(input.file.at.clone(), taken.record);
        entry.extend(taken.entry);
    }
    Ok(((gathered, entry), records))
}

/// One sample's gathered columns (none where it kept no event of any file read) and entries.
type Joined = (Vec<Option<Gathered>>, Vec<i64>);

/// A sample's piece of the file. A column it gathered nothing of (a sample of empty files, or
/// one read not at all) is empty, of the kind the other samples gave it.
fn piece(
    name: &str,
    (gathered, entry): Joined,
    outputs: &[Output],
    kinds: &[Option<(Kind, bool)>],
) -> Result<write::Piece, String> {
    let columns = gathered.into_iter().zip(outputs).zip(kinds).map(|((held, (column, _, _)), kind)| {
        let held = match (held, kind) {
            (Some(held), _) => held,
            (None, Some((kind, listed))) => Gathered { name: (*column).clone(), kind: *kind, bytes: Vec::new(), counts: listed.then(Vec::new) },
            (None, None) => return Err(format!("no sample has an event to take {column} from, so its kind is unknown: press more events")),
        };
        let buffer = NumericBuffer { kind: held.kind.name().into(), count: held.bytes.len() / held.kind.width(), bytes: held.bytes };
        let values = match held.counts {
            Some(counts) => write::Values::Jagged { counts, content: buffer },
            None => write::Values::Flat(buffer),
        };
        Ok((held.name, values))
    });
    Ok(write::Piece {
        name: name.to_owned(),
        columns: columns.collect::<Result<_, String>>()?,
        entry,
    })
}

/// A whole number with thousands separated, as rvr prints them.
fn count(n: u64) -> String {
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

/// Press a study: check, read, keep, write `target` (through a temporary file beside it).
fn press(
    dir: &Path,
    study: &str,
    target: &Path,
    events: Option<&str>,
    workers: usize,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let (mut lock, source) = inputs(dir)?;
    let recipe = read_recipe(&source)?;
    if let Some(dataset) = lock
        .sample
        .iter()
        .find(|s| s.file.iter().any(|f| f.lfn.is_some()))
    {
        return Err(format!("{} reads a central dataset. Pressing datasets is not built yet: rvr press reads files through a storage's HTTPS door, and choosing a site's door for each dataset file is still to come. Name files on storage instead (files = \"root://…\"), or press the dataset with the Python presser.", dataset.name));
    }
    for sample in &mut lock.sample {
        let want = wanted(sample, events)?;
        resolve::planned(sample, want)?;
    }
    let outputs: Vec<Output> = recipe
        .copy
        .iter()
        .map(|(n, e)| (n, e, true))
        .chain(recipe.columns.iter().map(|(n, e)| (n, e, false)))
        .collect();
    let work: Vec<Input> = lock
        .sample
        .iter()
        .enumerate()
        .flat_map(|(sample, s)| {
            s.file
                .iter()
                .filter(|f| f.stop != Some(0))
                .map(move |file| Input {
                    sample,
                    tree: &s.tree,
                    file,
                })
        })
        .collect();
    let remote = work.iter().any(|i| i.file.at.contains("://"));
    let (storage, offline) = if remote {
        let (proxy, said, ca) = crate::samples::proxy();
        match Storage::new(proxy.as_deref(), ca.as_deref()) {
            Ok(storage) => (Some(storage), said),
            Err(why) => (None, why),
        }
    } else {
        (None, String::new())
    };
    let read: u64 = lock.sample.iter().map(|s| s.events_read).sum();
    println!(
        "Pressing {} samples, {} files, {} entries, {} at a time:",
        lock.sample.len(),
        work.len(),
        count(read),
        workers.min(work.len()).max(1)
    );
    let mentions = recipe.mentions();
    let shared = Shared {
        recipe: &recipe,
        outputs: &outputs,
        storage: storage.as_ref(),
        offline: &offline,
        mentions: &mentions,
        dir,
        failed: AtomicBool::new(false),
    };
    let mut taken = take_all(&shared, &lock, &work, workers)?.into_iter();
    let mut tally = Tally {
        read: 0,
        cutflow: vec![0; recipe.keep.len()],
        files: Vec::new(),
    };
    let (mut pieces, mut made_files) = (Vec::new(), Map::new());
    let mut kinds: Vec<Option<(Kind, bool)>> = vec![None; outputs.len()];
    for (index, sample) in lock.sample.iter().enumerate() {
        let inputs: Vec<&Input> = work.iter().filter(|i| i.sample == index).collect();
        let (joined, records) = joined(
            sample, &inputs, &mut taken, &outputs, &mut kinds, &mut tally,
        )?;
        pieces.push(joined);
        let mut metadata = sample.metadata.clone();
        metadata.insert("label".into(), sample.label.clone().into());
        metadata.insert("events_read".into(), sample.events_read.into());
        metadata.insert("events_total".into(), sample.events_total.into());
        made_files.insert(
            sample.name.clone(),
            json!({"files": records, "metadata": metadata}),
        );
    }
    let pieces = lock
        .sample
        .iter()
        .zip(pieces)
        .map(|(sample, joined)| piece(&sample.name, joined, &outputs, &kinds))
        .collect::<Result<Vec<_>, String>>()?;
    let failed = |e: rowver::Error| e.to_string();
    let (mut data, arrays) =
        write::pack_with(pieces, Labels::new(), tally.files, workers).map_err(failed)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let mut made = json!({
        "study": study,
        "built": grid_proxy::utc(now),
        "by": concat!("rvr ", env!("CARGO_PKG_VERSION")),
        "cart": {"sha256": write::text_checksum(&source), "source": source},
        "files": made_files,
    });
    if let Some(title) = &recipe.title {
        made["title"] = title.as_str().into();
    }
    let names: Vec<&str> = lock.sample.iter().map(|s| s.name.as_str()).collect();
    let written: Vec<&str> = outputs.iter().map(|(n, _, _)| n.as_str()).collect();
    data["sample_cols"] = write::sample_cols(&names, &made["files"], &written)
        .map_err(failed)?
        .into();
    made["sha256"] = write::made_checksum(&made).map_err(failed)?.into();
    let rows = data["rows"].as_u64().unwrap_or(0);
    let staged = target.with_extension("rvr.partial");
    let written = write::write(
        &staged,
        &json!({"rowver": 13, "data": data, "made": made}),
        &arrays,
    )
    .map_err(|e| e.to_string())
    .and_then(|()| std::fs::rename(&staged, target).map_err(|e| e.to_string()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("{} could not be written: {e}", target.display()));
    }
    let bytes = std::fs::metadata(target).map_or(0, |m| m.len());
    println!("Read {} entries; kept after each cut:", count(tally.read));
    for (cut, kept) in recipe.keep.iter().zip(&tally.cutflow) {
        println!("  {:30} {:>13}", cut.name, count(*kept));
    }
    println!(
        "Wrote {} ({} rows, {} columns, {:.1} MB) in {:.1} s.",
        target.display(),
        count(rows),
        outputs.len(),
        bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    Ok(())
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
