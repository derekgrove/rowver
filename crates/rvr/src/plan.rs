//! The Plan (docs/SPEC.md §2.4): one value of one fixed form, built only here, from the inputs
//! the validators passed and the agreement check found to fit, owning everything the pressing
//! core needs. Whatever the core is given, it can run.

use crate::fill::{Fill, Laid};
use crate::reuse::Earlier;
use crate::samples::Choice;
use crate::texts::Texts;
use rowver::choice::Events;
use rowver::study::lock::{Branch, File, Lock, Sample};
use rowver::write;
use rvr_recipe::{Expression, Kind, Recipe};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

/// A column's kind, and whether it holds a list per row.
pub type Shape = (Kind, bool);

/// A stored column read: its name and expression, and its kind.
pub type Output<'a> = (&'a (String, Expression), Shape);

/// By sample, the branches its files are compared on; and every name the evaluated recipe mentions.
type Compared = (Vec<HashMap<String, Branch>>, BTreeSet<String>);

/// Where a press writes, and what it is of.
pub struct Place {
    /// The folder a relative path in the lock is read from.
    pub base: PathBuf,
    /// The study's name, which the file records.
    pub study: String,
    /// The file the press writes.
    pub target: PathBuf,
    /// The file at the target, opened once for all the Plan takes from it.
    pub at: At,
    /// The lock on the file at the target, for a press that writes it (SPEC §8.12).
    pub locked: Option<rowver::change::Locked>,
    /// What stood at the target, taken before it was found to hold no file to lock.
    pub found: Option<Standing>,
}

/// The file at a press's target, as it was found.
pub enum At {
    /// No file is there, or none is looked for (a press of a file of its own).
    Missing,
    /// One is there that cannot be opened: why.
    Unopened(rowver::Error),
    /// One is there: it, and what checking its header found.
    Opened(Box<rowver::rvr::Reader>, rowver::Result<()>),
}

impl At {
    /// The file at `target` as it is now, put back first when a change to it was cut short.
    pub fn open(target: &std::path::Path) -> At {
        if let Err(why) = crate::restored(target) {
            return At::Unopened(rowver::Error::Invalid(why));
        }
        match rowver::rvr::Reader::inspect(target) {
            Ok((file, checked)) => At::Opened(Box::new(file), checked),
            Err(rowver::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => At::Missing,
            Err(e) => At::Unopened(e),
        }
    }

    /// The file a press holds locked, read through the descriptor that holds the lock.
    pub fn held(locked: &rowver::change::Locked) -> At {
        match locked.inspect() {
            Ok((file, checked)) => At::Opened(Box::new(file), checked),
            Err(e) => At::Unopened(e),
        }
    }

    /// The file when it opens with its header checked, why not when it is there and does not.
    pub fn file(self) -> Option<rowver::Result<rowver::rvr::Reader>> {
        match self {
            At::Missing => None,
            At::Unopened(e) | At::Opened(_, Err(e)) => Some(Err(e)),
            At::Opened(file, Ok(())) => Some(Ok(*file)),
        }
    }
}

/// Where each sample's choice of events comes from.
pub enum Choose {
    /// samples.toml's, as the options change it.
    Options(Choice),
    /// A file's own (`made.files`), for a press of the study it carries.
    Recorded(Value),
}

impl Choose {
    /// What a sample reads.
    pub fn of(&self, sample: &Sample) -> Result<Events, String> {
        match self {
            Choose::Options(choice) => choice.of(sample),
            Choose::Recorded(files) => Events::read(&files[&sample.name]["events"]).map_err(|e| format!("{}: {e}", sample.name)),
        }
    }
}

/// What a press builds on, as asked.
#[derive(Clone, PartialEq, Eq)]
pub enum On {
    /// Nothing: every column is pressed (`--full`).
    Full,
    /// The file at the target, where it can be.
    Earlier,
    /// The file at the target but for its holes of these kinds (`--fill`).
    Holes(Vec<String>),
    /// Nothing, and a press that is not carried on: what it read goes when it stops.
    Once,
}

/// A press as asked of the core.
pub struct Asked {
    /// Where each sample's choice of events comes from.
    pub choose: Choose,
    /// What it builds on.
    pub on: On,
    /// How many files are read at once.
    pub workers: usize,
    /// Whether the file is signed with your key, when you have one.
    pub sign: bool,
}

/// One file to read, in the lock's order; `sample` is its sample's place in the lock.
#[derive(Clone)]
pub struct Input {
    /// Its sample's place in the lock.
    pub sample: usize,
    /// The tree its events are read from.
    pub tree: String,
    /// The file, as the lock pins it.
    pub file: File,
    /// Where it is read: its local path, or its storage door's HTTPS URL (SPEC §2.4).
    pub read_at: String,
    /// For a random choice, the entries chosen in this file, ascending.
    pub chosen: Option<Vec<u64>>,
    /// Building on an earlier file, the entries it kept of this file, ascending.
    pub kept: Option<Vec<u64>>,
    /// Building on an earlier file, its record of this file.
    pub before: Option<Value>,
    /// For a fill, the holes read, as entries `[first, last)`, each with its place among the spool's pieces.
    pub ranges: Option<Vec<(u64, u64, usize)>>,
    /// Its place among the pieces of the spool, when it is read whole.
    pub order: usize,
    /// For a fill, whether the file's record has no totals yet, so that its other trees are summed too.
    pub unsummed: bool,
}

/// What is taken from the earlier file at the target.
pub enum Base {
    /// Nothing: there is none, `--full`, or it cannot be built on (`Plan::why`).
    Nothing,
    /// Its rows, and the columns it holds as the recipe defines them.
    Columns(Earlier),
    /// Its rows of the samples it holds as the lock does.
    Samples(Fill),
    /// Its rows but for the holes read again: a fill.
    Holes(Fill),
    /// Everything: it holds the study as it is, with this many holes.
    Current(usize),
}

/// Everything the pressing core needs, and nothing more.
pub struct Plan {
    /// The file the press writes.
    pub target: PathBuf,
    /// Where it is staged while it is pressed.
    pub staging: PathBuf,
    /// The study's name, which the file records.
    pub study: String,
    /// The folder a relative path in the lock is read from.
    pub base: PathBuf,
    /// The three study texts, as the file carries them.
    pub texts: Texts,
    /// The lock, each sample's choice of events applied.
    pub lock: Lock,
    /// Each sample's choice of events.
    pub asked: Vec<Events>,
    /// The recipe, as checked.
    pub recipe: Recipe,
    /// The recipe the core evaluates: the whole one, or for a build on columns, what those read.
    pub reader: Recipe,
    /// Each stored column's kind and whether it is a list, in the recipe's order; the derived ones'.
    pub kinds: Vec<Shape>,
    /// Each derived column's kind and whether it is a list.
    pub derived: Vec<Shape>,
    /// The stored columns read, by place in the recipe; the others are copied.
    pub reading: Vec<usize>,
    /// By sample, the branches of its list its files are compared on before they are read.
    pub checked: Vec<HashMap<String, Branch>>,
    /// Every name the evaluated recipe mentions, to choose the branches fetched ahead.
    pub mentions: BTreeSet<String>,
    /// Every file, in the lock's order; and those read (a fill reads only its holes).
    pub work: Vec<Input>,
    /// The files read, each with what is read of it, when they are not every one of `work` (a
    /// fill's, a build on samples', or none); `reads()` gives them.
    own_reads: Option<Vec<Input>>,
    /// For a fill or a build on samples, how each of `work` is laid among the file's old rows.
    pub laid: Vec<Laid>,
    /// What is taken from the earlier file.
    pub from: Base,
    /// Why the file at the target gives nothing, when it is there.
    pub why: Option<String>,
    /// Whether it gives nothing only for the build that made it (`why` then names both builds).
    pub other_build: bool,
    /// How many files are read at once.
    pub workers: usize,
    /// What built this rvr, as the file names it.
    pub toolchain: Value,
    /// Whether the press keeps a journal, so that one stopped is carried on.
    pub carries: bool,
    /// The key the file is signed with: yours, when it is signed and you have one.
    pub signer: Option<[u8; 32]>,
    /// What the journal's first line says this press is.
    pub identity: Value,
    /// The tool keys of the file it replaces at the target, which the new file carries.
    pub tools: serde_json::Map<String, Value>,
    /// The lock on the file at the target, held until the press ends (SPEC §8.12).
    pub locked: Option<rowver::change::Locked>,
    /// What stood at the target where there was no file to lock: a link to nothing, or nothing.
    pub found: Option<Standing>,
    /// Why the file it only adds columns to cannot be changed in place, so it is written anew.
    pub anew: Option<String>,
    /// Why a press that would write the file is refused before it reads: it lies on a read-only filesystem.
    pub refused: Option<String>,
}

impl Plan {
    /// The files read, each with what is read of it.
    pub fn reads(&self) -> &[Input] {
        self.own_reads.as_deref().unwrap_or(&self.work)
    }

    /// The stored columns read, each with its expression and kind.
    pub fn outputs(&self) -> Vec<Output<'_>> {
        self.reading.iter().map(|&at| (&self.recipe.columns[at], self.kinds[at])).collect()
    }
}

/// What every command that reads events takes from a study: its inputs validated and checked together.
pub struct Checked {
    /// The recipe, as its validator read it.
    pub recipe: Recipe,
    /// The lock, each sample's choice of events applied.
    pub lock: Lock,
    /// Each sample's choice of events.
    pub asked: Vec<Events>,
    /// Each stored column's kind and whether it is a list.
    pub kinds: Vec<Shape>,
    /// Each derived column's kind and whether it is a list.
    pub derived: Vec<Shape>,
    /// Where each file is read, by sample and file in the lock's order (`agree::doors`).
    pub read_at: Vec<Vec<String>>,
}

/// `lines` of the validator of `file`, each named with it.
fn named(file: &str, lines: &str) -> Vec<String> {
    lines.lines().map(|line| if line.starts_with(file) { line.to_owned() } else { format!("{file}: {line}") }).collect()
}

/// A study's texts through their validators and the agreement check, or every problem found.
pub fn checked(texts: &Texts, choose: &Choose) -> Result<Checked, String> {
    let mut said: Vec<String> = Vec::new();
    let samples = rowver::study::samples::read(&texts.samples).map_err(|p| said.extend(named("samples.toml", &p.to_string()))).is_ok();
    let recipe = crate::texts::read_recipe(&texts.recipe).map_err(|e| said.extend(e.lines().map(str::to_owned))).ok();
    let lock = match Lock::read(&texts.lock) {
        Ok(lock) => Some(lock),
        Err(p) => {
            said.extend(p.to_string().lines().map(|p| format!("samples.lock does not check out: {p}. Run rvr samples resolve samples.toml again.")));
            None
        }
    };
    agreed(texts, samples, recipe, lock, said, choose)
}

/// The agreement check (docs/SPEC.md §2.3) on what the validators made of `texts` (`samples`
/// whether samples.toml passed, `said` what they refused), or every problem found.
pub fn agreed(texts: &Texts, samples: bool, recipe: Option<Recipe>, lock: Option<Lock>, mut said: Vec<String>, choose: &Choose) -> Result<Checked, String> {
    let mut planned = None;
    if let Some(lock) = lock {
        if samples {
            said.extend(crate::agree::lock(texts, &lock));
        }
        let read_at = crate::agree::doors(&lock).map_err(|e| said.extend(e)).ok();
        let (lock, asked, problems) = crate::agree::choices(lock, choose);
        said.extend(problems);
        // A host with no door only adds its line: the recipe is still checked against the lock.
        planned = Some((lock, asked, read_at));
    }
    let mut kinds = None;
    if let (Some(recipe), Some((lock, asked, _))) = (&recipe, &planned) {
        kinds = crate::agree::recipe(recipe, lock, asked).map_err(|e| said.extend(e)).ok();
    }
    match (recipe, planned, kinds) {
        (Some(recipe), Some((lock, asked, Some(read_at))), Some([kinds, derived])) if said.is_empty() => Ok(Checked { recipe, lock, asked, kinds, derived, read_at }),
        _ => Err(said.join("\n")),
    }
}

/// Every file of the lock a press reads, in order, with what its choice reads of it.
fn inputs(lock: &Lock, asked: &[Events], read_at: &[Vec<String>]) -> Vec<Input> {
    let mut work = Vec::new();
    for (sample, s) in lock.samples.iter().enumerate() {
        let entries: Vec<u64> = s.files.iter().map(|f| f.entries).collect();
        let chosen: Vec<Option<Vec<u64>>> = match asked[sample] {
            Events::Random { n, seed } => rowver::choice::per_file(&entries, n, seed).into_iter().map(Some).collect(),
            _ => vec![None; entries.len()],
        };
        for ((file, chosen), read_at) in s.files.iter().zip(chosen).zip(&read_at[sample]).filter(|((f, _), _)| f.stop != Some(0)) {
            let order = work.len();
            work.push(Input { sample, tree: s.tree.clone(), file: file.clone(), read_at: read_at.clone(), chosen, kept: None, before: None, ranges: None, order, unsummed: false });
        }
    }
    work
}

/// Building on `earlier`, each input with the entries the file kept of it and its record; or why
/// it cannot be: a file's kept entries that lie outside those this press chooses of it.
fn built_on(work: &mut [Input], lock: &Lock, earlier: &mut Earlier) -> Result<(), String> {
    for input in work.iter_mut() {
        let key = (lock.samples[input.sample].name.clone(), input.file.at.clone());
        let record = &earlier.file().header()["made"]["files"][&key.0]["files"][&key.1];
        input.before = Some(record.clone()).filter(|r| !r.is_null());
        let kept = earlier.kept.remove(&key).unwrap_or_default();
        let mut holes = input.before.as_ref().map(crate::take::ranges).unwrap_or_default();
        holes.sort_unstable();
        if !among(&kept, input.file.stop.unwrap_or(input.file.entries), input.chosen.as_deref(), &holes) {
            return Err(format!("its rows of {} are not among the entries this press chooses of it", input.file.at));
        }
        input.kept = Some(kept);
    }
    Ok(())
}

/// Whether every one of `kept` (ascending) is below `limit`, among `chosen` (ascending) when there is
/// a random choice, and in none of `holes` (by their first entry): all walked once, together.
fn among(kept: &[u64], limit: u64, chosen: Option<&[u64]>, holes: &[(u64, u64)]) -> bool {
    let (mut c, mut h) = (0, 0);
    for &entry in kept {
        if entry >= limit {
            return false;
        }
        if let Some(chosen) = chosen {
            while chosen.get(c).is_some_and(|e| *e < entry) {
                c += 1;
            }
            if chosen.get(c) != Some(&entry) {
                return false;
            }
        }
        while holes.get(h).is_some_and(|(_, last)| *last <= entry) {
            h += 1;
        }
        if holes[h..].iter().take_while(|(first, _)| *first <= entry).any(|(_, last)| entry < *last) {
            return false;
        }
    }
    true
}

/// Whether the file's columns `names` are of the kinds this press makes them, or the first that is not.
pub(crate) fn same_kinds(file: &rowver::rvr::Reader, recipe: &Recipe, kinds: &[(Kind, bool)], names: &[&str]) -> Result<(), String> {
    for ((name, _), (kind, listed)) in recipe.columns.iter().zip(kinds).filter(|((n, _), _)| names.contains(&n.as_str())) {
        let Some(record) = file.record(name) else { continue };
        let jagged = record["kind"] == "jagged";
        let held = if jagged { &record["content"]["kind"] } else { &record["kind"] };
        if held.as_str() != Some(kind.name()) || jagged != *listed {
            return Err(format!("its column {name} is of another kind than this press makes it ({}{})", if *listed { "a list of " } else { "" }, kind.name()));
        }
    }
    Ok(())
}

/// What the earlier file, `at` the target, gives, and why it gives nothing when it is there (and
/// whether only for the build that made it).
fn base(place: &Place, at: At, texts: &Texts, checked: &Checked, on: &On) -> Result<(Base, Option<(String, bool)>), String> {
    let Checked { recipe, lock, asked, kinds, .. } = checked;
    let all: Vec<&str> = recipe.columns.iter().map(|(n, _)| n.as_str()).collect();
    match on {
        On::Full | On::Once => Ok((Base::Nothing, None)),
        On::Holes(holes) => {
            let file = match at.file() {
                None => return Err(format!("{} is not there: rvr press makes it, and --fill fills its holes", place.target.display())),
                Some(file) => file.map_err(|e| rowver::plain(&place.target, &e))?,
            };
            let fill = crate::fill::plan(file, texts, lock, asked, holes)?;
            same_kinds(&fill.file, recipe, kinds, &all).map_err(|why| format!("{} cannot be filled, as {why}: rvr press presses it again", place.target.display()))?;
            Ok((Base::Holes(fill), None))
        }
        On::Earlier => match at.file().map(|file| crate::reuse::read(file, texts, recipe, lock, asked, kinds)) {
            None => Ok((Base::Nothing, None)),
            Some(Err(crate::reuse::Unbuilt::Build(said))) => Ok((Base::Nothing, Some((said, true)))),
            Some(Err(crate::reuse::Unbuilt::Why(why))) => Ok((Base::Nothing, Some((why, false)))),
            Some(Ok(base)) => Ok((base, None)),
        },
    }
}

/// The Plan of a press, or every reason it cannot be made, one a line.
pub fn build(mut place: Place, texts: Texts, asked: Asked) -> Result<Plan, String> {
    let toolchain = crate::press::toolchain();
    rowver::rvr::toolchain(&toolchain).map_err(|e| format!("this rvr cannot record what built it, as every file it writes does ({e}): build it again from a git checkout of its repository, or install a released rvr"))?;
    let checked = checked(&texts, &asked.choose)?;
    let signer = match asked.sign {
        true => crate::key::secret()?,
        false => None,
    };
    // A press that replaces the file at its target keeps its tool keys (docs/FORMAT.md §11), those
    // of one whose header opens though its arrays do not too; it stops over one that only its tool
    // keys keep from opening, for they would go with it (SPEC §8.12).
    let at = std::mem::replace(&mut place.at, At::Missing);
    let tools = match &at {
        At::Missing => serde_json::Map::new(),
        At::Opened(file, Ok(())) => rowver::rvr::tool_keys(file.header()),
        At::Unopened(e) | At::Opened(_, Err(e)) => {
            let text = match &place.locked {
                Some(locked) => locked.header_text(),
                None => rowver::rvr::header_text(&place.target),
            };
            match text.as_deref().map(rowver::rvr::checked_header) {
                Ok(Ok(header)) => rowver::rvr::tool_keys(&header),
                Ok(Err(_)) if text.as_deref().is_ok_and(rowver::change::shut_by_tools) => {
                    let shown = place.target.display();
                    let named = rowver::shell(&shown.to_string());
                    let why = match e {
                        rowver::Error::Tool { fact, .. } | rowver::Error::Breaks { fact, .. } => fact.clone(),
                        rowver::Error::Json(json) => format!("its header is not JSON as docs/FORMAT.md §1 reads it ({json})"),
                        e => e.to_string(),
                    };
                    return Err(format!("{shown} does not open: {why}. A press carries over the tool keys of the file it replaces, so it stops: drop the tool keys that break it (rvr tool {named} names them), or remove the file, then press again."));
                }
                _ => serde_json::Map::new(),
            }
        }
    };
    let (mut from, why) = base(&place, at, &texts, &checked, &asked.on)?;
    let (mut why, other_build) = match why {
        Some((why, build)) => (Some(why), build),
        None => (None, false),
    };
    let Checked { recipe, lock, asked: events, kinds, derived, read_at } = checked;
    let mut work = inputs(&lock, &events, &read_at);
    if let Base::Columns(earlier) = &mut from {
        if let Err(not) = built_on(&mut work, &lock, earlier) {
            (from, why) = (Base::Nothing, Some(not));
            for input in &mut work {
                (input.kept, input.before) = (None, None);
            }
        }
    }
    let reading: Vec<usize> = match &from {
        Base::Columns(earlier) => (0..recipe.columns.len()).filter(|at| !earlier.reused[*at]).collect(),
        Base::Current(_) => Vec::new(),
        _ => (0..recipe.columns.len()).collect(),
    };
    // A file that holds the study as it is, recipe and all, is current, or filled: not pressed.
    if let Base::Columns(earlier) = &from {
        if reading.is_empty() && earlier.file().header()["made"]["recipe"]["text"] == texts.recipe.as_str() {
            from = Base::Current(rowver::rvr::holes(earlier.file().header()).len());
        }
    }
    // Whether the file may be written, and why a change of it would be written anew: asked of one
    // not locked (rvr status) with one probe.
    let (unwritable, anew_why) = match (&from, &place.locked) {
        (Base::Current(_), _) => (None, None),
        (_, Some(locked)) => (locked.unwritable(), locked.anew()),
        (_, None) => rowver::change::probe(&place.target),
    };
    // A file that cannot be changed in place is written anew, decided before anything is read
    // (SPEC §8.12).
    let anew = match &mut from {
        Base::Columns(earlier) if earlier.in_place => {
            earlier.in_place = anew_why.is_none();
            anew_why
        }
        _ => None,
    };
    // A file on a read-only filesystem is written neither in place nor anew beside it: a press
    // that would write it is refused before anything is staged (SPEC §8.12).
    let refused = read_only(&place.target, unwritable);
    let reader = match &from {
        Base::Columns(_) => {
            let names: Vec<&String> = reading.iter().map(|at| &recipe.columns[*at].0).collect();
            crate::reuse::only(&recipe, &names)
        }
        _ => recipe.clone(),
    };
    let (own_reads, laid) = match &from {
        Base::Samples(fill) | Base::Holes(fill) => {
            let (reads, laid) = crate::fill::laid(&work, &lock, &fill.files, &fill.entries)?;
            (Some(reads), laid)
        }
        Base::Columns(_) if reading.is_empty() => (Some(Vec::new()), Vec::new()),
        Base::Current(_) => (Some(Vec::new()), Vec::new()),
        _ => (None, Vec::new()),
    };
    let reads = own_reads.as_ref().map_or(work.len(), Vec::len);
    let (checked_branches, mentions) = compared(&lock, &reader, &reading.iter().map(|at| &recipe.columns[*at].1).collect::<Vec<_>>())?;
    let identity = json!({
        "recipe": write::text_checksum(&texts.recipe),
        "samples": write::text_checksum(&texts.samples),
        "lock": write::text_checksum(&texts.lock),
        "events": events.iter().map(|e| e.record()).collect::<Vec<_>>(),
        "reading": reading.iter().map(|at| recipe.columns[*at].0.as_str()).collect::<Vec<_>>(),
        "on": match (&from, &asked.on) {
            (Base::Holes(fill), On::Holes(kinds)) => json!({"fill": fill.file.header()["data"]["sha256"], "kinds": kinds}),
            (Base::Samples(fill), _) => json!({"samples": fill.file.header()["data"]["sha256"]}),
            (Base::Columns(earlier), _) => json!({"earlier": earlier.file().header()["data"]["sha256"]}),
            _ => Value::Null,
        },
        "toolchain": toolchain,
        "inputs": reads,
    });
    let carries = asked.on != On::Once && crate::reuse::identified().is_ok();
    Ok(Plan {
        staging: crate::journal::folder(&place.target),
        target: place.target,
        study: place.study,
        base: place.base,
        texts,
        lock,
        asked: events,
        recipe,
        reader,
        kinds,
        derived,
        reading,
        checked: checked_branches,
        mentions,
        work,
        own_reads,
        laid,
        from,
        why,
        other_build,
        workers: asked.workers,
        toolchain,
        carries,
        signer,
        identity,
        tools,
        found: place.found,
        locked: place.locked,
        anew,
        refused,
    })
}

/// Refuse a target that leads to anything but an `.rvr` file, links followed, before anything is read or asked (SPEC §8.12).
pub fn a_file_or_none(target: &std::path::Path) -> Result<(), String> {
    let what = match std::fs::metadata(target) {
        Ok(there) if there.is_dir() => "a folder",
        Ok(there) if !there.is_file() => "a pipe, socket or device",
        Ok(there) => {
            // One that cannot be read is said by what reads it next; an empty one holds nothing to lose.
            let mut head = [0_u8; 8];
            return match std::fs::File::open(target).map(|mut file| std::io::Read::read_exact(&mut file, &mut head)) {
                Ok(Ok(())) if &head == rowver::rvr::MAGIC => Ok(()),
                _ if there.len() == 0 => Ok(()),
                Ok(Ok(())) => Err(format!("{} is not an .rvr file (it does not begin as one does), and rvr press writes only over an .rvr file: move it away, or name another file with --out", target.display())),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(format!("{} is not an .rvr file (it is shorter than one's first bytes), and rvr press writes only over an .rvr file: move it away, or name another file with --out", target.display())),
                Ok(Err(_)) | Err(_) => Ok(()),
            };
        }
        Err(_) => return Ok(()),
    };
    Err(format!("{} is {what}, not a file: move it away, or name a file with --out", target.display()))
}

/// What stands at a path, links not followed: its kind, device and inode (none where the system has no inodes).
pub type Standing = (std::fs::FileType, u64, u64);

/// What stands at `path`, links not followed.
pub fn standing(path: &std::path::Path) -> Option<Standing> {
    let there = std::fs::symlink_metadata(path).ok()?;
    #[cfg(unix)]
    return Some((there.file_type(), std::os::unix::fs::MetadataExt::dev(&there), std::os::unix::fs::MetadataExt::ino(&there)));
    #[cfg(not(unix))]
    Some((there.file_type(), 0, 0))
}

/// What a press of the file at `target` is told when the file lies on a read-only filesystem (`kind`, why it may not be written).
pub(crate) fn read_only(target: &std::path::Path, kind: Option<std::io::ErrorKind>) -> Option<String> {
    (kind == Some(std::io::ErrorKind::ReadOnlyFilesystem)).then(|| format!("{} lies on a read-only filesystem: rvr press cannot write it or a new file beside it; copy the study to a disk you may write", target.display()))
}

/// By sample, the branches of its list that its files must have as it lists them: those the
/// evaluated recipe reads by name (a defined name is not one), by text, or through a field of the
/// sample; and every name the recipe mentions, for what is fetched ahead.
fn compared(lock: &Lock, reader: &Recipe, columns: &[&Expression]) -> Result<Compared, String> {
    let roots: Vec<&Expression> = reader
        .keep
        .iter()
        .flat_map(rvr_recipe::Cut::expressions)
        .chain(columns.iter().copied())
        .chain(reader.totals.iter().filter(|total| total.tree.is_none()).map(|total| &total.value))
        .collect();
    let defined = crate::reuse::reached(reader, &roots);
    let mut mentions = BTreeSet::new();
    let (mut by_name, mut fields, mut by_text) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for expression in roots.iter().copied().chain(defined.iter().map(|d| &reader.defines[d])) {
        rvr_recipe::names(expression, &mut mentions);
        rvr_recipe::reads(expression, &mut by_name);
        rvr_recipe::branch_names(expression, &mut fields, &mut by_text);
    }
    by_name.retain(|name| !reader.defines.contains_key(name));
    by_name.extend(by_text);
    let mut checked = Vec::new();
    for s in &lock.samples {
        let list = lock.branches_of(s).expect("the lock validator checks every sample names a branch list it holds");
        let named = |b: &&Branch| by_name.contains(&b.name) || fields.iter().any(|f| s.metadata.get(f).and_then(Value::as_str) == Some(b.name.as_str()));
        checked.push(list.list.iter().filter(named).map(|b| (b.name.clone(), b.clone())).collect());
    }
    Ok((checked, mentions))
}

#[cfg(test)]
mod tests {
    // SPEC 2.3; FORMAT 3
    #[test]
    fn kept_entries_are_among_the_chosen_and_out_of_every_hole() {
        let among = super::among;
        assert!(among(&[0, 2, 9], 10, None, &[(3, 5)]));
        assert!(!among(&[0, 10], 10, None, &[]), "past the limit");
        assert!(!among(&[4], 10, None, &[(3, 5)]), "in a hole");
        assert!(!among(&[7], 10, None, &[(0, 2), (6, 8)]), "in a later hole");
        assert!(among(&[1, 5], 10, Some(&[1, 3, 5]), &[]));
        assert!(!among(&[1, 4], 10, Some(&[1, 3, 5]), &[]), "not chosen");
    }

    // SPEC 2.4, 3.8.1
    #[test]
    fn the_plan_reads_a_storage_file_from_its_doors_https_url() {
        use rowver::study::lock::File;
        let file = |at: &str| File { at: at.into(), bytes: 1, adler32: None, uuid: "u".into(), entries: 3, stop: None, take: None };
        let list = rowver::study::lock::Branches::new(Vec::new()).sha256;
        let mut lock = rowver::study::lock::Lock::read(&LOCK.replace("BR", &list)).unwrap();
        lock.samples[0].files = vec![file("root://eospublic.cern.ch//eos/opendata/x.root"), file("davs://h.org:1094/d/y.root"), file("./z.root")];
        let read_at = crate::agree::doors(&lock).unwrap();
        let urls: Vec<String> = super::inputs(&lock, &[rowver::choice::Events::All], &read_at).into_iter().map(|i| i.read_at).collect();
        assert_eq!(urls, ["https://eospublic.cern.ch:443/eos/opendata/x.root", "https://h.org:1094/d/y.root", "./z.root"]);
        lock.samples[0].files.push(file("root://no-door.invalid//d/w.root"));
        assert!(crate::agree::doors(&lock).unwrap_err()[0].contains("no-door.invalid has no WebDAV door"));
    }

    const LOCK: &str = "format = \"rvr-samples-lock\"\nversion = 1\nresolved = \"2026-11-03T12:00:00Z\"\nsamples_toml_sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n[[sample]]\nname = \"a\"\nlabel = \"a\"\nasked = [\"./z.root\"]\ntree = \"T\"\nbranches = \"BR\"\nevents = \"all\"\nevents_total = 3\nevents_read = 3\nmetadata = {}\n\n[[sample.file]]\nat = \"./z.root\"\nbytes = 1\nadler32 = \"00000001\"\nuuid = \"31e851e4-b994-11f1-ba5c-1a45bb13c76a\"\nentries = 3\n\n[[branches]]\nsha256 = \"BR\"\nlist = []\n";
}
