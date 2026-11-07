//! What a press has finished, in `<target>.partial/` beside the spool, so that one stopped carries
//! on (SPEC §7.5): a line for the press, then each input finished, trusted only up to a checkpoint.

use crate::take::Taken;
use rowver::write::Spool;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How often finished inputs are made durable (a checkpoint costs two syncs of ~4 ms: too much per file).
pub const EVERY: Duration = Duration::from_secs(2);

/// The journal's name in the folder, and the spool's.
const JOURNAL: &str = "journal.jsonl";
const SPOOL: &str = "spool";

/// The folder a press stages `target` in: its name with `.partial` added.
pub fn folder(target: &Path) -> PathBuf {
    rowver::write::beside(target, ".partial")
}

/// Make the folder `folder`, staging `target`: no wider open than the file there, as what it holds may
/// be a copy of it (each who may read it may open the folder, each who may write it write there).
fn private_folder(folder: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mode = match std::fs::metadata(target) {
            Ok(there) => {
                let mode = std::os::unix::fs::PermissionsExt::mode(&there.permissions()) & 0o666;
                mode | (mode & 0o444) >> 2 | 0o700
            }
            Err(_) => 0o777,
        };
        std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), mode).create(folder)
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        std::fs::create_dir(folder)
    }
}

/// The folder `target` is in.
pub fn parent(target: &Path) -> &Path {
    target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// Remove `path`, a folder or a file, if it is there.
fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Whether a press is under way in `folder` now: one holds its journal locked.
pub fn under_way(folder: &Path) -> bool {
    let Ok(file) = File::open(folder.join(JOURNAL)) else {
        return false;
    };
    matches!(file.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock))
}

/// The folder a press is staged in, locked through its journal; removed when dropped unless `keep`.
// The journal is read and written through the descriptor locked, shared, and no other opened while
// it is held: where flock is POSIX locks (Linux NFS), closing any descriptor of it lets the lock go.
pub struct Staged {
    /// The folder.
    pub folder: PathBuf,
    journal: Arc<File>,
    /// Whether the folder stays when the press ends, for the next press to pick up.
    pub keep: Cell<bool>,
}

impl Staged {
    /// The folder `target` is staged in, made when missing and locked, or why another press has it.
    pub fn lock(target: &Path) -> Result<Staged, String> {
        let folder = folder(target);
        let failed = |e: std::io::Error| {
            format!("{} could not be staged in {}: {}. Make its folder writable, or remove {} if it is there.", target.display(), folder.display(), rowver::described(&e), folder.display())
        };
        // A lock held for a moment is rvr status asking, or a press ending (its folder going, so
        // the journal locked may no longer be the one there): asked again.
        for _ in 0..5 {
            // Only the staging folder is made: a target in a folder that is not there is a mistake.
            // No wider open than the file there, as what it holds may be a copy of it.
            match private_folder(&folder, target) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!("{} cannot be written: the folder {} is not there. Make it, or give --out a path in a folder that is.", target.display(), parent(target).display()));
                }
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(failed(e)),
                _ => {}
            }
            let mut options = File::options();
            let journal = options.read(true).write(true).create(true).truncate(false);
            let journal = match journal.open(folder.join(JOURNAL)) {
                // The folder went as it was opened (a press ending): asked again.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                opened => opened.map_err(failed)?,
            };
            // A filesystem that has no locks is pressed on without one.
            let locked = !matches!(journal.try_lock(), Err(std::fs::TryLockError::WouldBlock));
            if locked && still_there(&journal, &folder.join(JOURNAL)) {
                // A press that ends before it is staged leaves a journal it found, and no folder it made.
                let keep = journal.metadata().map_err(failed)?.len() > 0;
                return Ok(Staged { folder, journal: Arc::new(journal), keep: Cell::new(keep) });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(format!(
            "another rvr press of {} is under way (it holds {}): let it finish, or stop it, then press again.",
            target.display(),
            folder.display()
        ))
    }
}

/// Whether `file` is the file at `path` still (not one removed while it was opened).
fn still_there(file: &File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(open), Ok(there)) = (file.metadata(), std::fs::metadata(path)) else {
            return false;
        };
        open.dev() == there.dev() && open.ino() == there.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        true
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.keep.get() {
            let _ = remove(&self.folder);
        }
    }
}

/// What an earlier press of this one finished, ready to go on with.
pub struct Picked {
    done: Done,
    /// What each input finished gave, by its place in the press.
    pub taken: BTreeMap<usize, Taken>,
    spool: Spool,
}

/// What an earlier press of this one (`press`) left in `staged`, or `None`, saying why it starts
/// over; a build that `carries` no press picks up nothing.
pub fn pick_up(staged: &Staged, press: &Value, carries: bool) -> Option<Picked> {
    let folder = &staged.folder;
    let over = |why: &str| println!("Starting over: the press that stopped in {} {why}.", folder.display());
    match found(unfinished_in(&staged.journal), press) {
        Found::Nothing => None,
        Found::Same(done) if done.inputs.is_empty() => None,
        Found::Same(_) if !carries => {
            over("is not carried on: this press keeps no journal (rvr refill and rvr reproduce keep none, nor does a build of rvr whose commit is unknown or -dirty)");
            None
        }
        Found::Same(done) => {
            let inputs = press["inputs"].as_u64().unwrap_or(0) as usize;
            let taken: Option<BTreeMap<usize, Taken>> = done
                .inputs
                .iter()
                .map(|(at, line)| Some((*at, taken(line).filter(|_| *at < inputs)?)))
                .collect();
            let held: Vec<&Value> = done.inputs.values().map(|line| &line["held"]).collect();
            match taken.map(|taken| (taken, Spool::reopen(&folder.join(SPOOL), &held))) {
                Some((taken, Ok(spool))) => Some(Picked { done, taken, spool }),
                _ => {
                    over("cannot be picked up: its journal and its spool do not agree");
                    None
                }
            }
        }
        Found::Other(why) => {
            over(&format!("was another press: {why}"));
            None
        }
    }
}

/// The spool and journal of a press (`press`) of `target` staged in `staged`: `picked`'s, or new ones.
pub fn stage(target: &Path, staged: &Staged, press: &Value, picked: Option<Picked>) -> Result<(Spool, Journal), String> {
    let folder = &staged.folder;
    let failed = |e: &dyn std::fmt::Display| {
        format!("{} could not be staged in {}: {e}. Make it writable, or free disk space there.", target.display(), folder.display())
    };
    let file = staged.journal.clone();
    if let Some(Picked { done, spool, .. }) = picked {
        let journal = Journal::on(file, folder.join(JOURNAL), press, Some(&done)).map_err(|e| failed(&e))?;
        staged.keep.set(false);
        let inputs = press["inputs"].as_u64().unwrap_or(0) as usize;
        println!(
            "Picking up {} where it stopped: {} of {} done (journal from {}).",
            target.display(),
            done.inputs.len(),
            crate::samples::some(inputs, "file", "files"),
            done.when
        );
        return Ok((spool, journal));
    }
    // An old journal is emptied on disk before its spool goes, so its checkpoints never come back
    // beside another press's values; then everything but the journal, which is held locked, goes.
    if staged.keep.get() {
        file.set_len(0).and_then(|()| file.sync_data()).map_err(|e| failed(&e))?;
    }
    for entry in std::fs::read_dir(folder).map_err(|e| failed(&e))? {
        let path = entry.map_err(|e| failed(&e))?.path();
        if path.file_name() != Some(JOURNAL.as_ref()) {
            remove(&path).map_err(|e| failed(&e))?;
        }
    }
    let spool = Spool::create(&folder.join(SPOOL)).map_err(|e| failed(&e))?;
    let journal = Journal::on(file, folder.join(JOURNAL), press, None).map_err(|e| failed(&e))?;
    staged.keep.set(false);
    Ok((spool, journal))
}

/// What an earlier press of the same study finished.
pub struct Done {
    /// Each input finished, by its place in the press: its journal line.
    pub inputs: BTreeMap<usize, Value>,
    /// When its journal was last written, UTC.
    pub when: String,
    /// How many inputs the journal names as finished, a checkpoint covering them or not.
    pub named: usize,
    /// Where its last checkpoint line ends: what follows is not trusted, and is cut off.
    end: u64,
}

/// What a press finds in its folder.
pub enum Found {
    /// No journal, or none that can be read.
    Nothing,
    /// The journal of this press.
    Same(Done),
    /// Another press's, and how it differs.
    Other(String),
}

/// The journal's whole lines, each with where it ends, and when it was written; a line cut short is left out.
fn lines(mut file: &File) -> Option<(Vec<(Value, u64)>, String)> {
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(0)).and_then(|_| file.read_to_end(&mut bytes)).ok()?;
    let when = file
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or("?".into(), |d| voms_proxy::utc(d.as_secs() as i64));
    let (mut lines, mut at) = (Vec::new(), 0);
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        at += line.len() as u64;
        match serde_json::from_slice::<Value>(line) {
            Ok(value) if line.ends_with(b"\n") => lines.push((value, at)),
            _ => break,
        }
    }
    Some((lines, when))
}

/// The inputs the checkpoints among `lines` cover (the lines after the first, which ends at `end`).
fn covered(lines: impl Iterator<Item = (Value, u64)>, when: String, mut end: u64) -> Done {
    let (mut inputs, mut held, mut named) = (BTreeMap::new(), Vec::new(), 0);
    for (line, at) in lines {
        if line.get("checkpoint").is_some() {
            inputs.extend(held.drain(..));
            end = at;
        } else if let Some(input) = line["input"].as_u64() {
            held.push((input as usize, line));
            named += 1;
        }
    }
    Done { inputs, when, named, end }
}

/// The journal in `folder`, read against `press` (this press's first line).
pub fn read(folder: &Path, press: &Value) -> Found {
    found(unfinished(folder), press)
}

/// An unfinished press, read against `press` (this press's first line).
fn found(unfinished: Option<(Value, Done)>, press: &Value) -> Found {
    match unfinished {
        None => Found::Nothing,
        Some((was, done)) if was == *press => Found::Same(done),
        Some((was, _)) => Found::Other(differs(&was, press)),
    }
}

/// An unfinished press in `folder`, whatever press it is: its first line, and what its checkpoints cover.
pub fn unfinished(folder: &Path) -> Option<(Value, Done)> {
    unfinished_in(&File::open(folder.join(JOURNAL)).ok()?)
}

/// Why the press whose journal is read through `file` left its folder for nothing, if it did: it
/// was stopped once its file was in place (its last line names the file now at `target`), or it
/// finished every file and would make what is there (`current`, the Plan of a file current).
fn left_in(file: &File, target: &Path, current: Option<&Value>) -> Option<String> {
    let shown = target.display();
    let placed = last_line(file).and_then(|line| line.get("placed").cloned());
    if placed.is_some() && placed == identity(target) {
        return Some(format!("a press left it, stopped after it put {shown} in place"));
    }
    let (now, (was, done)) = (current?, unfinished_in(file)?);
    let all = was["inputs"].as_u64() == Some(done.inputs.len() as u64);
    let makes = all && ["recipe", "samples", "lock", "events", "toolchain"].iter().all(|key| was.get(key) == now.get(key));
    makes.then(|| format!("the press stopped there would make {shown} as it is"))
}

/// `left_in`, the journal in `folder` opened to be read.
pub fn left(folder: &Path, target: &Path, current: Option<&Value>) -> Option<String> {
    left_in(&File::open(folder.join(JOURNAL)).ok()?, target, current)
}

/// The last whole line of the journal read through `file`, if it is short (as a `placed` note is).
fn last_line(mut file: &File) -> Option<Value> {
    let len = file.metadata().ok()?.len();
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(len.saturating_sub(4096))).and_then(|_| file.read_to_end(&mut tail)).ok()?;
    let line = tail.strip_suffix(b"\n")?.rsplit(|b| *b == b'\n').next()?;
    serde_json::from_slice(line).ok()
}

/// The device, inode, size and modification time of the file at `path`, which a move keeps.
fn identity(path: &Path) -> Option<Value> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| json!([m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()]))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Remove the staging folder of `target` a press left for nothing, saying so (SPEC §7.5.7).
pub fn tidy(target: &Path, current: Option<&Value>) {
    let folder = folder(target);
    // Any other stays as it is, one with no journal untouched.
    if !folder.join(JOURNAL).is_file() || under_way(&folder) {
        return;
    }
    let Ok(staged) = Staged::lock(target) else {
        return;
    };
    let why = left_in(&staged.journal, target, current);
    staged.keep.set(why.is_none());
    drop(staged);
    if let Some(why) = why.filter(|_| !folder.exists()) {
        println!("Removed {}: {why}.", folder.display());
    }
}

/// `unfinished`, its journal read through `file`.
fn unfinished_in(file: &File) -> Option<(Value, Done)> {
    let (lines, when) = lines(file)?;
    let mut lines = lines.into_iter();
    let (first, end) = lines.next()?;
    let press = first.get("press")?.clone();
    Some((press, covered(lines, when, end)))
}

/// What differs between two presses' first lines, in words.
fn differs(was: &Value, now: &Value) -> String {
    let said = [
        ("recipe", "recipe.toml changed"),
        ("samples", "samples.toml changed"),
        ("lock", "samples.lock was resolved again"),
        ("events", "it chose other events (--events, --random or --seed)"),
        ("reading", "it read other columns"),
        ("on", "it built on another file, or filled another"),
        ("toolchain", "another build of rvr made it"),
    ];
    let changed: Vec<&str> = said
        .iter()
        .filter(|(key, _)| was.get(key) != now.get(key))
        .map(|(_, said)| *said)
        .collect();
    match changed.is_empty() {
        true => "it is another press's".into(),
        false => changed.join(", and "),
    }
}

/// The journal line of input `input`, finished as `taken` says, its values in the spool as `held`.
pub fn line(input: usize, taken: &Taken, held: Value) -> Value {
    json!({
        "input": input,
        "rows": taken.rows,
        "record": taken.record,
        "entries": taken.entries,
        "missing": taken.missing,
        "cutflow": taken.cutflow,
        "held": held,
    })
}

/// What a journal line says an input gave, or `None` when it is not a line `line` wrote.
pub fn taken(line: &Value) -> Option<Taken> {
    let numbers = line["cutflow"].as_array()?.iter().map(Value::as_u64);
    Some(Taken {
        rows: usize::try_from(line["rows"].as_u64()?).ok()?,
        record: line.get("record").filter(|r| r.is_object())?.clone(),
        entries: line["entries"].as_u64()?,
        missing: line["missing"].as_u64()?,
        cutflow: numbers.collect::<Option<_>>()?,
    })
}

/// The journal of a press under way, written at each checkpoint.
pub struct Journal {
    waiting: Mutex<Waiting>,
    /// The journal's file, held by one checkpoint at a time.
    file: Mutex<Arc<File>>,
    path: PathBuf,
}

/// The lines of the inputs finished since the last checkpoint, and how many inputs it holds in all.
struct Waiting {
    lines: Vec<String>,
    finished: usize,
}

/// A lock held, whether or not a thread that held it panicked.
fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

impl Journal {
    /// The journal in `file` (at `path`): a new one whose first line is `press`, or `done`'s, written on.
    fn on(mut file: Arc<File>, path: PathBuf, press: &Value, done: Option<&Done>) -> std::io::Result<Self> {
        match done {
            Some(done) => file.set_len(done.end)?,
            None => {
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                writeln!(file, "{}", json!({ "press": press }))?;
            }
        }
        file.seek(SeekFrom::End(0))?;
        let finished = done.map_or(0, |done| done.inputs.len());
        let waiting = Mutex::new(Waiting { lines: Vec::new(), finished });
        Ok(Journal { waiting, file: Mutex::new(file), path })
    }

    /// An input finished, as `line` says: made durable by the next checkpoint.
    pub fn finished(&self, line: &Value) {
        let mut waiting = held(&self.waiting);
        waiting.lines.push(line.to_string());
        waiting.finished += 1;
    }

    /// Make every input finished so far durable: how many there are.
    pub fn checkpoint(&self, spool: &Spool) -> Result<usize, String> {
        let mut file = held(&self.file);
        // The lines taken are of inputs whose values are in the spool already, so they are covered
        // by its sync; inputs finishing meanwhile wait for the next checkpoint.
        let (lines, finished) = {
            let mut waiting = held(&self.waiting);
            (std::mem::take(&mut waiting.lines), waiting.finished)
        };
        if lines.is_empty() {
            return Ok(finished);
        }
        let start = file.stream_position();
        let written = || -> std::io::Result<()> {
            spool.sync().map_err(|e| match e {
                rowver::Error::Io(e) => e,
                e => std::io::Error::other(e.to_string()),
            })?;
            let mut out = std::io::BufWriter::new(&**file);
            for line in &lines {
                writeln!(out, "{line}")?;
            }
            writeln!(out, "{}", json!({ "checkpoint": true }))?;
            out.flush()?;
            drop(out);
            file.sync_data()
        };
        match written() {
            Ok(()) => Ok(finished),
            Err(e) => {
                let why = rowver::plain_io(&self.path, &e);
                // Left as the last checkpoint left it, its lines wait for the next.
                if let Ok(start) = start {
                    let _ = file.set_len(start).and_then(|()| file.seek(SeekFrom::Start(start)).map(drop));
                }
                held(&self.waiting).lines.splice(0..0, lines);
                Err(why)
            }
        }
    }

    /// Note that `staged` is about to be moved into place (not made durable): where the note begins.
    pub fn placing(&self, staged: &Path) -> Option<u64> {
        let it = identity(staged)?;
        let mut file = held(&self.file);
        let at = file.stream_position().ok()?;
        // Begun, it is taken back if the move fails, written whole or not.
        let _ = writeln!(file, "{}", json!({ "placed": it }));
        Some(at)
    }

    /// Take back the note `placing` began `at`, the move not made (its inode may be another file's).
    pub fn unplaced(&self, at: u64) {
        let mut file = held(&self.file);
        let _ = file.set_len(at).and_then(|()| file.seek(SeekFrom::Start(at)).map(drop));
    }

    /// A checkpoint `every` until `ended` closes; one that fails is tried again at the next.
    pub fn keep_up(&self, spool: &Spool, every: Duration, ended: Receiver<()>) {
        while let Err(RecvTimeoutError::Timeout) = ended.recv_timeout(every) {
            let _ = self.checkpoint(spool);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC 7.5.7
    #[test]
    fn a_folder_is_known_as_left_only_when_its_journal_names_the_file_moved_in() {
        let dir = std::env::temp_dir().join(format!("rvr-placed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("study.rvr");
        let staged = Staged::lock(&target).unwrap();
        let (spool, journal) = stage(&target, &staged, &json!({"inputs": 1}), None).unwrap();
        let file = staged.folder.join("pressed.rvr");
        std::fs::write(&file, b"pressed").unwrap();
        journal.placing(&file);
        staged.keep.set(true);
        drop((journal, spool, staged));
        // Stopped before the move: not left, and tidied away by nothing.
        assert!(left(&folder(&target), &target, None).is_none());
        tidy(&target, None);
        assert!(folder(&target).is_dir());
        std::fs::rename(&file, &target).unwrap();
        assert!(left(&folder(&target), &target, None).is_some());
        tidy(&target, None);
        assert!(!folder(&target).exists());
        // A move that failed takes its note back: the file it named may be another's by then.
        std::fs::remove_file(&target).unwrap();
        let staged = Staged::lock(&target).unwrap();
        let (spool, journal) = stage(&target, &staged, &json!({"inputs": 1}), None).unwrap();
        std::fs::write(&file, b"pressed").unwrap();
        let at = journal.placing(&file).unwrap();
        journal.unplaced(at);
        std::fs::rename(&file, &target).unwrap();
        staged.keep.set(true);
        drop((journal, spool, staged));
        assert!(left(&folder(&target), &target, None).is_none());
        assert!(unfinished(&folder(&target)).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // SPEC 7.5.2, 7.5.3, 7.5.5
    #[test]
    fn only_the_lines_a_checkpoint_covers_are_picked_up() {
        let dir = std::env::temp_dir().join(format!("rvr-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("study.rvr");
        let press = json!({"recipe": "abc", "inputs": 3, "toolchain": crate::press::toolchain()});
        let taken = |rows| Taken {
            rows,
            record: json!({"tree": "t", "uuid": null, "entries": 10}),
            entries: 10,
            missing: 0,
            cutflow: vec![rows as u64],
        };
        let staged = Staged::lock(&target).unwrap();
        let (spool, journal) = stage(&target, &staged, &press, None).unwrap();
        journal.finished(&line(2, &taken(4), json!([])));
        assert_eq!(journal.checkpoint(&spool), Ok(1));
        journal.finished(&line(0, &taken(5), json!([])));
        // A press under way holds its folder: another is refused, and status sees it.
        assert!(Staged::lock(&target).is_err_and(|why| why.contains("is under way")));
        assert!(under_way(&staged.folder));
        staged.keep.set(true);
        drop((journal, spool, staged));
        assert!(!under_way(&folder(&target)));
        // A line cut short by a crash.
        let path = folder(&target).join(JOURNAL);
        write!(File::options().append(true).open(&path).unwrap(), "{{\"input\": 1, \"rows\"").unwrap();
        let Found::Same(done) = read(&folder(&target), &press) else { panic!("the same press") };
        assert_eq!(done.inputs.keys().collect::<Vec<_>>(), [&2]);
        assert_eq!(super::taken(&done.inputs[&2]).unwrap().rows, 4);
        let other = json!({"recipe": "abd", "inputs": 3, "toolchain": crate::press::toolchain()});
        let Found::Other(why) = read(&folder(&target), &other) else { panic!("another press") };
        assert_eq!(why, "recipe.toml changed");
        // Written on from its checkpoint, what followed it is gone; a line waits for a checkpoint
        // no longer than `every`, with no input finishing after it.
        let staged = Staged::lock(&target).unwrap();
        let picked = pick_up(&staged, &press, true).unwrap();
        let (spool, journal) = stage(&target, &staged, &press, Some(picked)).unwrap();
        journal.finished(&line(1, &taken(6), json!([])));
        let (ticking, ended) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| journal.keep_up(&spool, Duration::from_millis(10), ended));
            std::thread::sleep(Duration::from_millis(100));
            drop(ticking);
        });
        let Found::Same(done) = read(&staged.folder, &press) else { panic!("the same press") };
        assert_eq!(done.inputs.keys().collect::<Vec<_>>(), [&1, &2]);
        assert_eq!(unfinished(&staged.folder).map(|(press, done)| (press["inputs"].clone(), done.inputs.len())), Some((3.into(), 2)));
        // Dropped unkept, the folder goes.
        drop((journal, spool, staged));
        assert!(!folder(&target).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
