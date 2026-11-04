//! What a press has finished, in `<target>.partial/` beside the spool, so that one stopped carries
//! on (SPEC §7.5): a line for the press, then each input finished, trusted only up to a checkpoint.

use crate::take::Taken;
use rowver::write::Spool;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// How often finished inputs are made durable (a checkpoint costs two syncs of ~4 ms: too much per file).
pub const EVERY: Duration = Duration::from_secs(2);

/// The journal's name in the folder, and the spool's.
const JOURNAL: &str = "journal.jsonl";
const SPOOL: &str = "spool";

/// The folder a press stages `target` in: its name with `.partial` added.
pub fn folder(target: &Path) -> PathBuf {
    crate::files::beside(target, ".partial")
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
pub struct Staged {
    /// The folder.
    pub folder: PathBuf,
    journal: File,
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
            match std::fs::create_dir(&folder) {
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
                return Ok(Staged { folder, journal, keep: Cell::new(keep) });
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

/// What an earlier press of this one (`press`) left in `staged`, or `None`, saying why it starts over.
pub fn pick_up(staged: &Staged, press: &Value) -> Option<Picked> {
    let folder = &staged.folder;
    let over = |why: &str| println!("Starting over: the press that stopped in {} {why}.", folder.display());
    match read(folder, press) {
        Found::Same(done) if !done.inputs.is_empty() => {
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
        _ => None,
    }
}

/// The spool and journal of a press (`press`) of `target` staged in `staged`: `picked`'s, or new ones.
pub fn stage(target: &Path, staged: &Staged, press: &Value, picked: Option<Picked>) -> Result<(Spool, Journal), String> {
    let folder = &staged.folder;
    let failed = |e: &dyn std::fmt::Display| {
        format!("{} could not be staged in {}: {e}. Make it writable, or free disk space there.", target.display(), folder.display())
    };
    let file = staged.journal.try_clone().map_err(|e| failed(&e))?;
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
fn lines(folder: &Path) -> Option<(Vec<(Value, u64)>, String)> {
    let path = folder.join(JOURNAL);
    let bytes = std::fs::read(&path).ok()?;
    let when = std::fs::metadata(&path)
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
    let (mut inputs, mut held) = (BTreeMap::new(), Vec::new());
    for (line, at) in lines {
        if line.get("checkpoint").is_some() {
            inputs.extend(held.drain(..));
            end = at;
        } else if let Some(input) = line["input"].as_u64() {
            held.push((input as usize, line));
        }
    }
    Done { inputs, when, end }
}

/// The journal in `folder`, read against `press` (this press's first line).
pub fn read(folder: &Path, press: &Value) -> Found {
    match unfinished(folder) {
        None => Found::Nothing,
        Some((was, done)) if was == *press => Found::Same(done),
        Some((was, _)) => Found::Other(differs(&was, press)),
    }
}

/// An unfinished press in `folder`, whatever press it is: its first line, and what its checkpoints cover.
pub fn unfinished(folder: &Path) -> Option<(Value, Done)> {
    let (lines, when) = lines(folder)?;
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
    file: Mutex<File>,
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
    fn on(mut file: File, path: PathBuf, press: &Value, done: Option<&Done>) -> std::io::Result<Self> {
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
        let start = (&*file).stream_position();
        let written = || -> std::io::Result<()> {
            spool.sync().map_err(|e| match e {
                rowver::Error::Io(e) => e,
                e => std::io::Error::other(e.to_string()),
            })?;
            let mut out = std::io::BufWriter::new(&*file);
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
        let picked = pick_up(&staged, &press).unwrap();
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
