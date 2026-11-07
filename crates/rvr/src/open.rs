//! Opening a file a press reads: checked against the lock first (docs/SPEC.md §7.2.3), local or
//! remote, and its tree's branches as an expression reads them.

use crate::held::Held;
use crate::holes::{fault_of, Failed, Fault, Seen};
use crate::samples::remote::{digest_seen, HttpsFile, Storage, Watch, READING};
use crate::samples::resolve::{self, adler32_file};
use hep_root::{ByteSource, RootFile, Tree};
use rowver::study::lock::{File, Lock};
use rvr_recipe::{Array, Columns, Constant, Kind, Recipe, Values};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use rowver::rvr::grouped as count;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Where a study's files can be read from: its folder and its storage.
pub(crate) struct Reach<'a> {
    /// The storage behind the grid proxy, for remote files.
    pub storage: Option<&'a Storage>,
    /// Why a remote file cannot be read, when there is no storage.
    pub offline: &'a str,
    /// The study folder, which a relative path in the lock is relative to.
    pub dir: &'a Path,
    /// What every read shares while it waits; it stops once an input ends the press.
    pub watch: Arc<Watch>,
    /// Whether a local file is read whole for its adler32 before it is opened (a press's, not a look's).
    pub whole: bool,
    /// Where every file counts the bytes it reads (the bars').
    pub read: Arc<AtomicU64>,
}

/// The branches of one open tree over the entries in hand, read when an expression first names them.
pub(crate) struct Branches<'a, S: ByteSource> {
    file: &'a RootFile<S>,
    tree: &'a Tree,
    first: i64,
    last: i64,
    read: RefCell<HashMap<String, Array>>,
    /// The first failure to read a branch, which tells a file that cannot be read from a recipe
    /// that asks the wrong thing of it.
    failure: RefCell<Option<hep_root::Error>>,
}

impl<'a, S: ByteSource> Branches<'a, S> {
    /// Entries `first..last` of a tree.
    pub fn over(file: &'a RootFile<S>, tree: &'a Tree, first: i64, last: i64) -> Self {
        let read = RefCell::new(HashMap::new());
        Branches { file, tree, first, last, read, failure: RefCell::new(None) }
    }

    /// The first failure to read a branch, if one failed.
    pub fn failure(&self) -> Option<hep_root::Error> {
        self.failure.take()
    }
}

/// A hep-root number kind as a `.rvr` kind.
pub(crate) fn kind_of(kind: hep_root::columns::Kind) -> Kind {
    Kind::named(resolve::kind_name(kind)).expect("every hep-root kind has a .rvr name")
}

impl<S: ByteSource> Columns for Branches<'_, S> {
    fn has(&self, name: &str) -> bool {
        self.tree.branch(name).is_some()
    }

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
                let said = rvr_recipe::problem(format!("{name}: {e}"));
                self.failure.borrow_mut().get_or_insert(e);
                return Err(said);
            }
        };
        let held = Values::from_bytes(kind_of(values.kind), &values.bytes);
        let array = match &values.counts {
            Some(counts) => match Array::object(counts, held) {
                Ok(array) => array,
                // Counts that do not add up to the values read are the file's bytes, not the recipe.
                Err(e) => {
                    let said = rvr_recipe::problem(format!("{name}: {}", e.0));
                    self.failure.borrow_mut().get_or_insert(hep_root::Error::format(0, e.0));
                    return Err(said);
                }
            },
            None => Array::Event(held),
        };
        self.read
            .borrow_mut()
            .insert(name.to_owned(), array.clone());
        Ok(Some(array))
    }

}

/// Whether a branch may be asked for: its name appears in the recipe, or among the sample's text
/// fields (for `branch(sample.FIELD)`).
pub(crate) fn mentioned(
    mentions: &BTreeSet<String>,
    sample: &BTreeMap<String, Constant>,
    branch: &str,
) -> bool {
    mentions.contains(branch)
        || sample
            .values()
            .any(|value| matches!(value, Constant::Text(text) if text == branch))
}

/// Refuse a file that differs from what the lock recorded of it.
pub(crate) fn changed(file: &File, bytes: Option<u64>, adler32: Option<String>) -> Result<(), Failed> {
    match resolve::differs(file, bytes, adler32.as_deref()) {
        None => Ok(()),
        Some(how) => Err(Failed::new(Fault::Changed, format!("{how}. It has changed since samples.lock was written: if the change is wanted, run rvr samples resolve samples.toml and review the new lock."))),
    }
}

/// Check one input against the lock and open it from `read_at` (its path, or its door's HTTPS URL),
/// a failure said by the lock's name for it; one gone says to put it back and `then`, or to resolve again and run `command`.
pub(crate) fn open(reach: &Reach, file: &File, read_at: &str, (then, command): (&str, &str)) -> Result<RootFile<Held>, Failed> {
    let at = &file.at;
    // Storage that could not be set up is not the file's fault: every remote file would fail the same.
    let offline = || Failed::new(Fault::Save, format!("{at}: {}", reach.offline));
    let again = Some((READING, reach.watch.clone()));
    if !read_at.contains("://") {
        // A relative path is the lock's own: relative to the study folder.
        let path = resolve::local(read_at, reach.dir).map_err(|why| Failed::new(Fault::Save, why))?;
        // A file whose folder is not there either may be on a disk not mounted: not the file's fault.
        let folder = path.parent().is_none_or(Path::exists);
        let meta = std::fs::metadata(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound if !folder => Failed::new(Fault::Save, format!(
                "{at} cannot be read: its folder is not there either (a disk not mounted?)"
            )),
            std::io::ErrorKind::NotFound => Failed::new(Fault::Gone, format!(
                "{at} no longer exists: put it back and {then} (if it moved, run rvr samples resolve samples.toml again, then {command})."
            )),
            _ => Failed::new(Fault::Save, rowver::plain_io(at, &e)),
        })?;
        // A file that cannot be read for want of rights, or on a mount that went, is not the
        // file's fault.
        let unreadable = |e: std::io::Error| Failed::new(Fault::Save, rowver::plain_io(at, &e));
        // Read whole for its adler32 only when the lock has one to compare with, and a press asks.
        let adler32 = match file.adler32 {
            Some(_) if reach.whole => {
                let adler32 = adler32_file(&path).map_err(unreadable)?;
                reach.read.fetch_add(meta.len(), Ordering::Relaxed);
                Some(adler32)
            }
            _ => None,
        };
        changed(file, Some(meta.len()), adler32)?;
        let local = hep_root::LocalFile::open(&path)
            .map_err(|e| Failed::new(fault_of(&e, None), format!("{at}: {e}")))?;
        return opened(file, Box::new(local), None, None, &reach.read);
    }
    let storage = reach.storage.ok_or_else(offline)?;
    let https = read_at;
    // The storage's size and adler32 are asked while the file is opened at the size the lock
    // gives; a changed file is reported before anything the opening found.
    std::thread::scope(|scope| {
        let agent = storage.agent();
        let digest = scope.spawn(move || {
            digest_seen(&agent, at, https, storage.waits, storage.proxy.is_some(), &storage.watch)
        });
        let seen = Arc::new(Seen::default());
        let remote = HttpsFile::reading(storage, at, https, file.bytes);
        let root = opened(file, Box::new(remote.seeing(&seen)), again, Some(seen), &reach.read);
        let (bytes, adler32) = digest
            .join()
            .map_err(|_| Failed::new(Fault::Bug, format!("{at}: asking its storage stopped unexpectedly")))??;
        changed(file, bytes, adler32)?;
        root
    })
}

/// Where the sample `named` is among the `names` of `file` (the first sample when none is named).
pub(crate) fn sample_at(names: &[&str], named: Option<&str>, file: &str) -> Result<usize, String> {
    if names.is_empty() {
        return Err(format!("{file} has no samples: add one to samples.toml and resolve again"));
    }
    let Some(name) = named else { return Ok(0) };
    names.iter().position(|s| *s == name).ok_or_else(|| format!("{file} has no sample named {name}; it has {}", names.join(", ")))
}

/// Where the sample `named` is in the lock (the first sample when none is named).
pub(crate) fn sample_in(lock: &Lock, named: Option<&str>) -> Result<usize, String> {
    let names: Vec<&str> = lock.samples.iter().map(|s| s.name.as_str()).collect();
    sample_at(&names, named, "samples.lock")
}

/// The first of a sample's files with entries, opened, with its entry in the lock.
pub(crate) fn first_readable<'a>(lock: &'a Lock, sample: usize, dir: &Path) -> Result<(RootFile<Held>, &'a File), String> {
    let s = &lock.samples[sample];
    let file = s
        .files
        .iter()
        .find(|f| f.entries != 0)
        .ok_or_else(|| format!("sample {} has no file with events: try another sample, with --sample NAME", s.name))?;
    let watch = Arc::new(Watch::default());
    let (storage, offline) = connect(&[(sample, file)], &watch);
    let read = Default::default();
    let reach = Reach { storage: storage.as_ref(), offline: &offline, dir, watch, whole: false, read };
    let url = match file.at.contains("://") {
        true => crate::samples::remote::https(&file.at)?,
        false => file.at.clone(),
    };
    open(&reach, file, &url, ("run rvr try again", "rvr try")).map(|root| (root, file)).map_err(|failed| failed.why)
}

/// Open a checked input and refuse one whose ROOT UUID is not the lock's; `again`, `seen` and
/// `bytes` as `Held`'s.
pub(crate) fn opened(
    file: &File,
    source: Box<dyn ByteSource>,
    again: Option<(&'static [u64], Arc<Watch>)>,
    seen: Option<Arc<Seen>>,
    bytes: &Arc<AtomicU64>,
) -> Result<RootFile<Held>, Failed> {
    let at = &file.at;
    let source = Held {
        shown: source.describe(),
        inner: source,
        held: Mutex::new(Vec::new()),
        again,
        seen: seen.clone(),
        bytes: bytes.clone(),
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
    if file.uuid != uuid {
        return Err(Failed::new(Fault::Changed, format!("{at} is not the file samples.lock recorded: its ROOT UUID was {}, it is now {uuid}. Run rvr samples resolve samples.toml and review the new lock.", file.uuid)));
    }
    Ok(root)
}

/// Where a failure is among entries from `first`: its event's entry, or the entries of the `part` it was read in.
pub(crate) fn entries(error: &rvr_recipe::Error, first: u64, part: &std::ops::Range<usize>) -> String {
    match error.1 {
        Some(row) => format!("entry {}", count(first + row as u64)),
        None if part.end > part.start => rowver::rvr::span(first + part.start as u64, first + part.end as u64),
        None => "no entries".into(),
    }
}

/// What to do about a failure reading a file's events: a name that is not a branch, with the
/// nearest branches and defined names; a damaged file (hep-root says `decompression failed`, or
/// names a compression algorithm it does not know).
pub(crate) fn hint(error: &rvr_recipe::Error, branches: &[String], recipe: &Recipe) -> String {
    if let Some(name) = error.missing() {
        let defined = recipe.defines.keys().map(String::as_str);
        let mut names: Vec<&str> = branches.iter().map(String::as_str).chain(defined).collect();
        names.sort();
        names.dedup();
        let near = rvr_recipe::meant(name.trim(), names.into_iter());
        return format!(".{near} rvr branches lists every branch the file has.");
    }
    match error.0.contains("decompression failed") || error.0.contains("compression algorithm") {
        false => String::new(),
        true => ". The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.".into(),
    }
}

/// Connect to what the files need: the storage behind the grid proxy, for remote files. With no
/// remote file, nothing; with storage that cannot be set up, the reason a remote file cannot be read.
pub(crate) fn connect(files: &[(usize, &File)], watch: &Arc<Watch>) -> (Option<Storage>, String) {
    if !files.iter().any(|(_, f)| f.at.contains("://")) {
        return (None, String::new());
    }
    let proxy = crate::samples::proxy();
    proxy.say();
    match Storage::new(proxy.path.as_deref(), proxy.ca.as_deref()) {
        Ok(mut storage) => {
            storage.waits = READING;
            storage.watch = watch.clone();
            (Some(storage), String::new())
        }
        Err(why) => (None, why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC 7.2.3
    #[test]
    fn checksums_compare_as_numbers_and_only_when_both_are_known() {
        let file = |adler32: Option<&str>| File {
            at: "f.root".into(),
            bytes: 10,
            adler32: adler32.map(str::to_owned),
            uuid: String::new(),
            entries: 0,
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
