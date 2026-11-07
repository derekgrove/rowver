//! Changes made in a file itself under its lock, journaled beside it or written anew (docs/SPEC.md §8.12).
// Columns added after its arrays, or a new header: a change cut short is put back from the journal,
// and a file that cannot be changed in place is written anew and moved over it.

use crate::write::{read_at, write_at, Source};
use crate::{invalid, Error, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The first 8 bytes of a journal.
const MAGIC: &[u8; 8] = b"\x89RVJ\r\n\x1a\n";

/// How many bytes before a file's tail, at most, its journal names it by.
const NAMED: u64 = 65_536;

/// What a file too long to address is told.
pub(crate) const LONG: &str = "the file is longer than this computer can address: read it on a 64-bit computer";

/// Why a file that may not be written, refused with `kind` when opened to be written, is not
/// changed in place.
fn unwritable(kind: ErrorKind) -> Option<&'static str> {
    match kind {
        ErrorKind::PermissionDenied => Some("you may not write it"),
        ErrorKind::ReadOnlyFilesystem => Some("its filesystem is read-only"),
        _ => None,
    }
}

/// Why the file at `path` met `kind` when opened to be written, and what lets it be written.
pub fn unwritable_said(kind: ErrorKind, path: &Path) -> (&'static str, String) {
    match kind {
        ErrorKind::ReadOnlyFilesystem => ("lies on a read-only filesystem", "mount it to be written".to_owned()),
        _ => ("is read-only", format!("chmod u+w {}", path.display())),
    }
}

/// The journal of a change to the file at `path`: its name with `-journal` added.
pub fn journal(path: &Path) -> PathBuf {
    crate::write::beside(path, "-journal")
}

/// The file `path` names: a symbolic link resolved, so that the file it names is the one changed.
pub fn resolved(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(held) if held.file_type().is_symlink() => std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// What became of a change to a file that was cut short (docs/SPEC.md §8.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restored {
    /// There was none.
    Nothing,
    /// Its journal checked out, and the file was put back as it was.
    PutBack,
    /// As `PutBack`, without the file's lock: its filesystem gives none.
    PutBackWithoutLock,
    /// Its journal did not check out: it was cut short before the file was touched, and was let go.
    LetGo,
    /// The change had finished, all but letting its journal go: the journal was let go, the file untouched.
    Finished,
    /// The file was not there: its journal, of no file, was let go.
    NoFile,
}

/// Put back the file at `path` as it was before a change to it cut short: what became of it.
pub fn restore(path: impl AsRef<Path>) -> Result<Restored> {
    let path = &resolved(path.as_ref());
    if !journal(path).exists() {
        return Ok(Restored::Nothing);
    }
    match std::fs::metadata(path) {
        // Left by a change to a file since removed: a file made there later is not the one it is of.
        Err(e) if e.kind() == ErrorKind::NotFound => let_go_journal(path).map(|()| Restored::NoFile),
        _ => Locked::take(path).map(|locked| locked.restored),
    }
}

/// The file at `path` opened to be read and written; in a debug build, refused as on a read-only
/// disk when `RVR_READ_ONLY_DISK` names it, so that a test meets one.
fn opened_to_write(path: &Path) -> std::io::Result<File> {
    #[cfg(debug_assertions)]
    if let Some(named) = std::env::var_os("RVR_READ_ONLY_DISK") {
        if std::fs::canonicalize(&named).is_ok_and(|named| std::fs::canonicalize(path).is_ok_and(|path| path == named)) {
            return Err(ErrorKind::ReadOnlyFilesystem.into());
        }
    }
    File::options().read(true).write(true).open(path)
}

/// Remove the journal of the file at `path`, a file no longer there: one another command let go
/// first is gone all the same.
fn let_go_journal(path: &Path) -> Result<()> {
    match std::fs::remove_file(journal(path)) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(crate::about(&journal(path), e.into())),
        _ => {
            synced(path);
            Ok(())
        }
    }
}

/// `restore`, its answer let go: what a writer does first with a file it reads.
pub(crate) fn restored(path: &Path) -> Result<()> {
    restore(path).map(drop)
}

/// Why a change of the file at `path` may not be written, and why it would be written anew: one open and a moment's lock.
pub fn probe(path: impl AsRef<Path>) -> (Option<ErrorKind>, Option<String>) {
    let path = path.as_ref();
    if unheld_at(path).is_err() {
        return (None, None);
    }
    let file = match opened_to_write(path) {
        Err(e) => return (unwritable(e.kind()).map(|_| e.kind()), unwritable(e.kind()).map(str::to_owned)),
        Ok(file) => file,
    };
    // A file this program holds locked is not asked about.
    let Ok(file) = unheld(file, path) else {
        return (None, None);
    };
    match file.try_lock_shared() {
        Err(std::fs::TryLockError::Error(e)) => (None, Some(format!("its filesystem gives no lock: {}", crate::described(&e)))),
        _ => (None, None),
    }
}

/// A file locked by its one writer, which no other changes while it is held (docs/SPEC.md §8.12).
// Read and written through this one descriptor, shared, and no other opened while it is held:
// where flock is POSIX locks (Linux NFS), closing any descriptor of a file lets the lock go.
#[derive(Debug)]
pub struct Locked {
    path: PathBuf,
    file: Handle,
    /// Why the filesystem gave no lock, when it gave none: a change is then written anew.
    lockless: Option<String>,
    /// What opening it to be written met, when it may not be: a change of it is then written anew.
    unwritable: Option<ErrorKind>,
    restored: Restored,
    /// Whether a change wrote it anew and moved that over it since it was locked: then this is
    /// the lock of a file no longer there.
    moved: AtomicBool,
    /// How many changes it has been through since it was locked, finished or put back.
    changes: std::sync::atomic::AtomicU64,
    /// What it is held by among the files this program holds locked (0: not among them).
    token: u64,
}

/// A locked file's tail as `Locked::inspect` read it: its length, where the tail starts, the
/// tail, and the lock's token and changes then, so that a change begins from it while it holds.
pub(crate) type Seen = (u64, u64, Vec<u8>, (u64, u64));

impl Locked {
    /// Lock the file `path` names, refused while another writer holds it, a change cut short put back.
    pub fn take(path: impl AsRef<Path>) -> Result<Self> {
        let path = resolved(path.as_ref());
        Self::taken(&path, |write| if write { opened_to_write(&path) } else { File::open(&path) }, File::try_lock)
    }

    /// `take` of the file at `path`, opened by `open` (to be written, or only read), locked by `lock`.
    fn taken(path: &Path, open: impl Fn(bool) -> std::io::Result<File>, lock: impl Fn(&File) -> std::result::Result<(), std::fs::TryLockError>) -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        let taken = Self::tried(path, token, open, lock);
        if taken.is_err() {
            let_go(token);
        }
        taken
    }

    /// `taken`, the file claimed for the lock `token` among those this program holds.
    fn tried(path: &Path, token: u64, open: impl Fn(bool) -> std::io::Result<File>, lock: impl Fn(&File) -> std::result::Result<(), std::fs::TryLockError>) -> Result<Self> {
        // A lock held for a moment (rvr status asking), or a file replaced as it was opened (a new
        // one moved over it), is asked again; the last try says why it gave up.
        let mut busy = false;
        for _ in 0..5 {
            // Claimed before it is opened, and again as it is opened: where flock is POSIX locks
            // (Linux NFS), the system gives one file's lock to every thread of a process that asks.
            if let Ok(there) = std::fs::metadata(path) {
                claim(token, ids(&there), path)?;
            }
            let opened = open(true).map(|file| (file, None)).or_else(|e| match unwritable(e.kind()) {
                Some(_) => open(false).map(|file| (file, Some(e.kind()))),
                None => Err(e),
            });
            let (file, unwritable) = opened.map_err(|e| crate::about(path, e.into()))?;
            let file = claimed(token, file, path)?;
            let lockless = match lock(&file) {
                Ok(()) => None,
                Err(std::fs::TryLockError::WouldBlock) => {
                    busy = true;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                // Where an exclusive lock is a write lock (Linux NFS), a file only read is refused
                // one: it is changed anew, without a lock, for it may not be written.
                Err(std::fs::TryLockError::Error(_)) if unwritable.is_some() => None,
                Err(std::fs::TryLockError::Error(e)) => Some(crate::described(&e)),
            };
            busy = false;
            if identity(&file).ok() == std::fs::metadata(path).ok().map(|m| ids(&m)) {
                let mut locked = Self {
                    path: path.to_path_buf(),
                    file: Handle::new(file),
                    lockless,
                    unwritable,
                    restored: Restored::Nothing,
                    moved: false.into(),
                    changes: 0.into(),
                    token,
                };
                locked.restored = locked.put_back()?;
                return Ok(locked);
            }
        }
        Err(invalid(match busy {
            true => format!("{} is being changed by another program (it holds the file's lock): wait for it to finish, then try again", path.display()),
            false => format!("{} was replaced each time it was opened, so another program is writing it: wait for it to finish, then try again", path.display()),
        }))
    }

    /// Why a change of the file is written anew and moved over it, when it cannot be made in place.
    pub fn anew(&self) -> Option<String> {
        match (&self.lockless, self.unwritable) {
            (Some(why), _) => Some(format!("its filesystem gives no lock: {why}")),
            (None, kind) => kind.and_then(unwritable).map(str::to_owned),
        }
    }

    /// Put the file back from the journal beside it, when there is one: under its lock, or, on a
    /// filesystem that gives none, without it. Refused for a file it may not write.
    fn put_back(&self) -> Result<Restored> {
        let shown = self.path.display();
        let can = || {
            let Some(kind) = self.unwritable else {
                return Ok(());
            };
            let (is, fix) = unwritable_said(kind, &self.path);
            Err(invalid(format!("{shown} has the journal of a change to it cut short, and {is}, so it cannot be put back: {fix}, then run any rvr command on it.")))
        };
        let restored = put_back(&self.file, &self.path, can, false)?;
        Ok(match (restored, &self.lockless) {
            (Restored::PutBack, Some(_)) => Restored::PutBackWithoutLock,
            (restored, _) => restored,
        })
    }

    /// What opening the file to be written met, when it may not be written (`ReadOnlyFilesystem` on a read-only disk).
    pub fn unwritable(&self) -> Option<ErrorKind> {
        self.unwritable
    }

    /// The file read through the descriptor that holds its lock, as `Reader::inspect` reads one.
    pub fn inspect(&self) -> Result<(crate::rvr::Reader, Result<()>)> {
        let (size, start, tail) = self.tail()?;
        let (mut reader, checked) = crate::rvr::Reader::locked(&self.path, self.file.clone(), &tail[..tail.len() - 12], start)?;
        reader.seen = Some((size, start, tail, (self.token, self.changes.load(Ordering::Relaxed))));
        Ok((reader, checked))
    }

    /// `inspect`, refused when the header fails its checks, as `Reader::open` refuses it.
    pub fn reader(&self) -> Result<crate::rvr::Reader> {
        let (reader, checked) = self.inspect()?;
        checked.map(|()| reader)
    }

    /// The file's header text, unchecked, read through the descriptor that holds the lock.
    pub fn header_text(&self) -> Result<Vec<u8>> {
        self.tail().map(|(_, _, mut tail)| {
            tail.truncate(tail.len().saturating_sub(12));
            tail
        })
    }

    /// The file's first bytes (up to 8), read through the descriptor that holds the lock.
    pub fn head(&self) -> Result<Vec<u8>> {
        let mut head = vec![0; self.file.metadata()?.len().min(8) as usize];
        read_at(&self.file, &mut head, 0)?;
        Ok(head)
    }

    /// Refused when another program moved a file over the one locked since it was locked.
    pub fn there(&self) -> Result<()> {
        unreplaced(&self.file, &self.path)
    }

    /// The path of the file locked.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What became of a change to it that was cut short, as it was locked.
    pub fn restored(&self) -> Restored {
        self.restored
    }

    /// Move the file at `staged` over the file locked, unless another program replaced it since.
    pub fn replace(self, staged: &Path) -> Result<()> {
        moved_over(&self.file, staged, &self.path)
    }

    /// The file's length, where its tail (its header, the header's length and the magic) begins,
    /// and the tail, its frame checked.
    pub(crate) fn tail(&self) -> Result<(u64, u64, Vec<u8>)> {
        if self.moved.load(Ordering::Relaxed) {
            return Err(invalid(format!("{} was written anew and moved over since it was locked: lock it again", self.path.display())));
        }
        let size = usize::try_from(self.file.metadata()?.len()).map_err(|_| invalid(LONG))?;
        let start = crate::rvr::frame(&self.file, size)?;
        let mut tail = vec![0; size - start];
        read_at(&self.file, &mut tail, start as u64)?;
        Ok((size as u64, start as u64, tail))
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        // Its descriptor closed, or left to its readers, before another lock of this program can take the file.
        self.file.close();
        let_go(self.token);
    }
}

/// A lock this program holds: its token, the file's device and inode, the thread that took it, and
/// the descriptors of the file its readers let go, closed when it lets go.
type Held = (u64, (u64, u64), std::thread::ThreadId, Vec<File>);

/// The files this program holds locked (see `Locked`).
fn held() -> std::sync::MutexGuard<'static, Vec<Held>> {
    static HELD: std::sync::Mutex<Vec<Held>> = std::sync::Mutex::new(Vec::new());
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Take the lock `token` out of the files this program holds, closing the descriptors it kept.
fn let_go(token: u64) {
    held().retain(|(held, ..)| *held != token);
}

/// Close `file`, a descriptor its readers let go; one of a file this program holds locked is kept by
/// that lock until it lets go, for where flock is POSIX locks (Linux NFS) closing it lets the lock go.
// Closed under the registry's lock: a lock of the file taken later is claimed after it is closed.
pub(crate) fn closed(file: File) {
    let Ok(id) = identity(&file) else {
        return;
    };
    let mut held = held();
    match held.iter_mut().find(|(_, at, ..)| *at == id) {
        Some((.., kept)) => kept.push(file),
        None => drop(file),
    }
}

/// A descriptor of a file, shared by the readers of it, closed as `closed` closes it once the last lets it go.
#[derive(Debug)]
pub(crate) struct Handle(Option<Arc<File>>);

impl Handle {
    /// `file`, to be shared.
    pub(crate) fn new(file: File) -> Self {
        Self(Some(Arc::new(file)))
    }

    /// Let this share of the descriptor go, closing it when it was the last.
    fn close(&mut self) {
        if let Some(file) = self.0.take().and_then(Arc::into_inner) {
            closed(file);
        }
    }
}

impl From<Arc<File>> for Handle {
    fn from(file: Arc<File>) -> Self {
        Self(Some(file))
    }
}

impl std::ops::Deref for Handle {
    type Target = Arc<File>;
    fn deref(&self) -> &Arc<File> {
        self.0.as_ref().expect("a handle is let go only as it is dropped")
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.close();
    }
}

/// Claim the file `id` (at `path`) for the lock `token` (0: only look), refused while another lock
/// of this program holds it: by another thread, until it lets go; by this one, a bug (a debug build stops).
fn claim(token: u64, id: (u64, u64), path: &Path) -> Result<()> {
    if cfg!(not(unix)) {
        return Ok(());
    }
    let mut held = held();
    let here = std::thread::current().id();
    if let Some(thread) = held.iter().find(|(held, at, ..)| *held != token && *at == id).map(|(_, _, thread, _)| *thread) {
        drop(held);
        let shown = path.display();
        if thread != here {
            return Err(invalid(format!("{shown} is being changed by this program already (another of its threads holds its lock, or is waiting for it): wait for that change to finish")));
        }
        let bug = format!("a bug in rowver: {shown} was opened again while this program holds it locked; please report it");
        #[cfg(debug_assertions)]
        panic!("{bug}");
        #[cfg(not(debug_assertions))]
        return Err(invalid(bug));
    }
    if token != 0 {
        match held.iter_mut().find(|(held, ..)| *held == token) {
            Some(entry) => (entry.1, entry.2) = (id, here),
            None => held.push((token, id, here, Vec::new())),
        }
    }
    Ok(())
}

/// `file`, just opened at `path`, claimed for the lock `token` (0: only looked at); one this
/// program holds locked is refused, its descriptor kept by that lock until it lets go (`closed`).
fn claimed(token: u64, file: File, path: &Path) -> Result<File> {
    let id = identity(&file).map_err(|e| crate::about(path, e.into()))?;
    match claim(token, id, path) {
        Ok(()) => Ok(file),
        Err(e) => {
            closed(file);
            Err(e)
        }
    }
}

/// Refuse the file at `path` when this program holds it locked, before a descriptor of it is opened.
pub(crate) fn unheld_at(path: &Path) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(there) => claim(0, ids(&there), path),
        Err(_) => Ok(()),
    }
}

/// `file`, just opened at `path` to be read, refused when this program holds it locked: one
/// replaced as it was opened, `unheld_at` having let it by.
pub(crate) fn unheld(file: File, path: &Path) -> Result<File> {
    claimed(0, file, path)
}

/// Refuse `path` when it no longer names `file`, the file opened there: another program moved a file over it.
fn unreplaced(file: &File, path: &Path) -> Result<()> {
    match std::fs::metadata(path).ok().map(|m| ids(&m)) == Some(identity(file).map_err(|e| crate::about(path, e.into()))?) {
        true => Ok(()),
        false => Err(invalid(format!("{} was replaced by another program while rvr held it: run the command again", path.display()))),
    }
}

/// Move the file at `staged` over `path`, with the permissions of `old`, the file there: refused
/// when `path` names another file now (one moved over it, where a filesystem gives no lock or by a
/// program that takes none), which is left as it is.
fn moved_over(old: &File, staged: &Path, path: &Path) -> Result<()> {
    let failed = |e: std::io::Error| crate::about(path, e.into());
    unreplaced(old, path)?;
    crate::write::moved_over(staged, path, Some(old.metadata().map_err(failed)?.permissions())).map_err(failed)?;
    synced(path);
    Ok(())
}

/// The JSON object `text` holds, checked by the header's rules to be the tool key `key` (docs/FORMAT.md §1, §11).
pub fn tool_data(key: &str, text: &[u8]) -> Result<Value> {
    crate::rvr::tool_key(key)?;
    let value = crate::json::strict_tool(text, key)?;
    if !value.is_object() {
        return Err(invalid(format!("the tool key {key} must be one JSON object (docs/FORMAT.md §11): write its data as one object, {{ … }}")));
    }
    Ok(value)
}

/// Make `value` (as `tool_data` gives it) the tool key `key` of the file `locked`, the new header checked (docs/SPEC.md §8.11).
pub fn set_tool(locked: &Locked, key: &str, value: Value) -> Result<()> {
    let (change, header) = Change::setting(locked, key, value)?;
    change.finish(&header)
}

/// Refuse a header every reader would not open, or longer than the format holds, before it is
/// written over the file at `path`: a fault under `given`, the tool key it was given, said as one to
/// change, any other as the file's own.
fn opens(header: &Value, path: &Path, advice: &str, given: Option<&str>) -> Result<()> {
    let text = serde_json::to_vec(header)?;
    crate::write::length(text.len(), advice).map_err(|e| crate::about(path, e))?;
    let under = |e: &Error| match e {
        Error::Tool { key, .. } => Some(key.clone()),
        Error::Breaks { under, .. } => under.clone(),
        _ => None,
    };
    match crate::rvr::checked_header(&text) {
        Err(e) if given.is_some() && under(&e).as_deref() == given => Err(crate::about(path, crate::rvr::to_write(e))),
        checked => checked.map(drop).map_err(|e| crate::about(path, e)),
    }
}

/// The text `header` is written as, and its length, refused unless every reader opens it, with `advice` when it is too long.
fn checked_text(header: &Value, advice: &str) -> Result<(Vec<u8>, u32)> {
    let text = serde_json::to_vec(header)?;
    let length = crate::write::length(text.len(), advice)?;
    crate::rvr::checked_to_write(&text)?;
    Ok((text, length))
}

/// The tail `header` is written as (its text, length and magic), refused unless every reader opens
/// it, with `advice` when it is too long.
fn opening(header: &Value, advice: &str) -> Result<Vec<u8>> {
    let (mut bytes, length) = checked_text(header, advice)?;
    bytes.reserve_exact(12);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(crate::rvr::MAGIC);
    Ok(bytes)
}

/// Whether the header `text` opens once the tool keys `keys` are taken out, whatever they hold.
pub fn opens_without(text: &[u8], keys: &[&str]) -> bool {
    without(text, keys).is_ok_and(|(rest, ..)| crate::rvr::checked_header(&rest).is_ok())
}

/// Whether the header `text`, which does not open, would with its tool keys taken out: only they keep it from opening (docs/FORMAT.md §11).
pub fn shut_by_tools(text: &[u8]) -> bool {
    let names: Vec<String> = crate::rvr::tool_texts(text).map(|keys| keys.into_iter().map(|(name, _)| name).collect()).unwrap_or_default();
    !names.is_empty() && crate::rvr::checked_header(text).is_err() && opens_without(text, &names.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Drop the tool keys `keys` of the file `locked` in one change: each key's copies, as bytes.
pub fn drop_tool(locked: &Locked, keys: &[&str]) -> Result<Vec<Vec<Vec<u8>>>> {
    let change = Change::dropping(locked, keys)?;
    let dropped = change.dropped().into_iter().map(|copies| copies.into_iter().map(<[u8]>::to_vec).collect()).collect();
    let header = change.header().clone();
    change.finish(&header)?;
    Ok(dropped)
}

/// Where each tool key's objects lie in a header: every copy of one held more than once.
type Cuts = Vec<Vec<std::ops::Range<usize>>>;

/// A header's text with tool keys taken out, where their objects lay, and the first one's place.
type Without = (Vec<u8>, Cuts, usize);

/// The header `text` with every top-level member named one of `keys` blanked out, its newlines kept
/// so that what is left lies at its lines and columns still; where each key's values lie, and the
/// first key's place among the members; or the first key it does not hold (none: no object).
// Its bytes are not parsed: a key holding what no parser reads (a lone surrogate, depth past
// serde_json's 128) is dropped all the same.
fn without<'k>(text: &[u8], keys: &[&'k str]) -> std::result::Result<Without, Option<&'k str>> {
    let members = crate::json::members(text).ok_or(None)?;
    let names: Vec<String> = members.iter().map(|m| crate::json::name(&text[m.name.clone()])).collect();
    let held = |key: &str| members.iter().zip(&names).filter(|(_, name)| *name == key).map(|(m, _)| m.value.clone()).collect::<Vec<_>>();
    let cuts: Cuts = keys.iter().map(|&key| Some(held(key)).filter(|cut| !cut.is_empty()).ok_or(Some(key))).collect::<std::result::Result<_, _>>()?;
    let at = keys.first().and_then(|key| names.iter().position(|name| name == key)).unwrap_or_default();
    let named: Vec<bool> = names.iter().map(|name| keys.contains(&name.as_str())).collect();
    let mut out = text.to_vec();
    for (m, _) in members.iter().zip(&named).filter(|(_, named)| **named) {
        out[m.name.start..m.value.end].iter_mut().filter(|b| **b != b'\n').for_each(|b| *b = b' ');
    }
    // Every comma goes, and one comes back after each member kept but the last.
    members.iter().filter_map(|m| m.comma).for_each(|at| out[at] = b' ');
    let kept: Vec<&crate::json::Member> = members.iter().zip(&named).filter(|(_, named)| !**named).map(|(m, _)| m).collect();
    kept.iter().take(kept.len().saturating_sub(1)).filter_map(|m| m.comma).for_each(|at| out[at] = b',');
    Ok((out, cuts, at))
}

/// A change of a locked file, journaled in place or written anew, undone when dropped unfinished.
// In place, the journal is written before the first byte changes, and the file put back from it
// when dropped unfinished; written anew, the new file (`FILE.anew-PID-N`) is moved over it when finished.
pub struct Change<'a> {
    locked: &'a Locked,
    header: Value,
    /// The file's length, where its old tail (the header on) began, and that tail.
    size: u64,
    start: u64,
    tail: Vec<u8>,
    /// Where the objects of the tool keys it drops lie in the old header (none for another change).
    cut: Cuts,
    /// The end of the arrays, counted from byte 8 (docs/FORMAT.md §7): where new ones go.
    end: u64,
    /// Whether it is empty: its records name bytes it does not hold.
    empty: bool,
    /// The new file it is written anew in, and its path, once begun.
    staged: Option<(PathBuf, Arc<File>)>,
    prepared: bool,
    finished: bool,
}

impl<'a> Change<'a> {
    /// Begin a change of the file `locked`: its header read and checked, nothing written.
    pub fn begin(locked: &'a Locked) -> Result<Self> {
        Self::begun(locked, |text| Ok((crate::json::strict(text)?, Vec::new())))
    }

    /// `begin`, the tool keys `keys` taken out unparsed, whatever they hold, and the header left checked to open (docs/SPEC.md §8.11).
    pub fn dropping(locked: &'a Locked, keys: &[&str]) -> Result<Self> {
        // A key misnamed (a slip of the format's own, or no tool key's name) is dropped too: it too keeps a file from opening.
        keys.iter().try_for_each(|key| crate::rvr::not_own(key))?;
        if keys.is_empty() {
            return Err(invalid("no tool key was named to drop: name one or more"));
        }
        let shown = locked.path.display();
        let change = Self::begun(locked, |text| match without(text, keys) {
            Ok((rest, cuts, _)) => Ok((crate::json::strict(&rest)?, cuts)),
            Err(Some(missing)) => Err(invalid(format!("{shown} has no tool key {missing}: rvr tool {} lists its tool keys", crate::shell(&shown.to_string())))),
            // A header that is not one object is refused as it is.
            Err(None) => Err(crate::rvr::checked_header(text).err().unwrap_or_else(|| invalid(format!("{shown}'s header is not one JSON object")))),
        })?;
        opens(&change.header, &locked.path, crate::write::FEWER, None)?;
        Ok(change)
    }

    /// `begin` without the tool key `key`, and the header with `value` that key, where it lay, checked to open (docs/SPEC.md §8.11).
    pub fn setting(locked: &'a Locked, key: &str, value: Value) -> Result<(Self, Value)> {
        // The header's checks refuse every name a tool key cannot have but the format's own keys.
        crate::rvr::not_own(key)?;
        // Taken out unparsed, so that a key breaking docs/FORMAT.md §1 is replaced too, and every copy of one held twice.
        let mut place = None;
        let change = Self::begun(locked, |text| match without(text, &[key]) {
            Ok((rest, cuts, at)) => {
                place = Some(at);
                Ok((crate::json::strict(&rest)?, cuts))
            }
            Err(_) => Ok((crate::json::strict(text)?, Vec::new())),
        })?;
        let mut header = change.header.clone();
        match (place, header.as_object_mut()) {
            (Some(at), Some(map)) => {
                map.shift_insert(at, key.to_owned(), value);
            }
            _ => header[key] = value,
        }
        opens(&header, &locked.path, "give the tool key less to hold", Some(key))?;
        Ok((change, header))
    }

    /// `begin` from what `reader` (of `locked.inspect`) read, or read again if the file changed since.
    pub fn of(locked: &'a Locked, reader: &crate::rvr::Reader) -> Result<Self> {
        match &reader.seen {
            Some((size, start, tail, at)) if *at == (locked.token, locked.changes.load(Ordering::Relaxed)) && !locked.moved.load(Ordering::Relaxed) => {
                stop(1);
                Self::from(locked, (*size, *start, tail.clone()), reader.header().clone(), Vec::new())
            }
            _ => Self::begin(locked),
        }
    }

    /// `begin`, the header read from its text by `read`: its value, and where the objects of the tool keys it drops lie.
    fn begun(locked: &'a Locked, read: impl FnOnce(&[u8]) -> Result<(Value, Cuts)>) -> Result<Self> {
        let path = &locked.path;
        stop(1);
        let (size, start, tail) = locked.tail().map_err(|e| crate::about(path, e))?;
        let (header, cut) = read(&tail[..tail.len() - 12]).map_err(|e| crate::about(path, e))?;
        Self::from(locked, (size, start, tail), header, cut)
    }

    /// The change of the file `locked` from its tail (its length, where the tail starts, and the
    /// tail) and the header it holds, parsed; `cut`, the objects of the tool keys it drops.
    fn from(locked: &'a Locked, (size, start, tail): (u64, u64, Vec<u8>), header: Value, cut: Cuts) -> Result<Self> {
        let end = crate::rvr::placed(&header, start as usize - 8).map_err(|e| crate::about(&locked.path, e))?;
        Ok(Self { locked, header, size, start, tail, cut, end: start - 8, empty: start == 8 && end > 0, staged: None, prepared: false, finished: false })
    }

    /// The file's header as it was when the change began, without the tool key it drops or sets.
    pub fn header(&self) -> &Value {
        &self.header
    }

    /// The header's text as the file held it when the change began.
    pub fn text(&self) -> &[u8] {
        &self.tail[..self.tail.len() - 12]
    }

    /// The bytes of each tool key the change takes out, every copy, as the header held them.
    pub fn dropped(&self) -> Vec<Vec<&[u8]>> {
        self.cut.iter().map(|copies| copies.iter().map(|cut| &self.text()[cut.clone()]).collect()).collect()
    }

    /// The file written: the file itself, or the new one it is written anew in.
    fn file(&self) -> &Arc<File> {
        match &self.staged {
            Some((_, file)) => file,
            None => &self.locked.file,
        }
    }

    /// Pack `columns` of the file's rows on `workers` threads after its arrays: their records.
    pub fn pack(&mut self, columns: Vec<(String, Source)>, workers: usize) -> Result<Vec<Value>> {
        let path = &self.locked.path;
        if self.empty {
            return Err(invalid(format!("{} is empty: columns are added only to a file that holds its arrays (rvr refill gives them back)", path.display())));
        }
        // No columns, no arrays: the change writes only a header, and its journal says so.
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        // Nor do a file of no rows' columns take a byte: its change too writes only a header, and
        // is journaled as it finishes, with its new tail. Written anew, it is begun now.
        let rowless = self.header["data"]["rows"].as_u64() == Some(0);
        if !rowless || self.locked.anew().is_some() {
            self.prepare(u64::MAX, &[])?;
        }
        // A failed read or write of the file written is told against it: the new one when written anew.
        let written = self.staged.as_ref().map_or_else(|| path.clone(), |(at, _)| at.clone());
        // Unjournaled, the file is not handed over at all: what its columns would take is only counted.
        let into = self.prepared.then(|| self.file().clone());
        let (records, end) = crate::write::added(into, self.end, &self.header["data"], columns, workers, &written).map_err(|e| crate::about(path, e))?;
        if end != self.end && !self.prepared {
            return Err(invalid(format!("{}: a bug in rowver: columns of no rows would take bytes, and the change is not journaled to write them, so the file is as it was; please report it", path.display())));
        }
        self.end = end;
        Ok(records)
    }

    /// The records of `derived` under `header`, from the arrays packed so far (one the file has is kept).
    pub fn derived(&self, header: &Value, derived: &[(String, String)]) -> Result<Vec<Value>> {
        let before = self.header["derived"]["columns"].as_array().map_or(&[][..], Vec::as_slice);
        let file = crate::rvr::Reader::laid(&self.locked.path, self.file().clone(), self.end, header.clone())?;
        crate::rvr::derived::records(&file, derived, before)
    }

    /// Finish the change with `header` after the arrays, synced, the journal let go or the file moved over.
    pub fn finish(mut self, header: &Value) -> Result<()> {
        let path = self.locked.path.clone();
        // A header every reader opens, or none: refused, the file put back.
        let shown = path.display();
        let bytes = opening(header, crate::write::FEWER).map_err(|e| invalid(format!("{shown} is as it was: the header the change would write would not open ({e}), so it was not written")))?;
        // Its records lay out exactly the arrays written (an empty file's, what its records did), or
        // the change is a bug: refused, the file put back.
        let bug = |why: String| invalid(format!("{}: a bug in rowver: the header it was about to write lays out other arrays than the file holds ({why}), so the file is put back as it was; please report it", path.display()));
        let (end, spans) = crate::rvr::spans(header).map_err(|e| bug(e.to_string()))?;
        match self.empty {
            true if spans != crate::rvr::spans(&self.header).map_err(|e| bug(e.to_string()))?.1 => return Err(bug("an empty file's records lay out the arrays they did".into())),
            false if end != self.end => return Err(bug(format!("its arrays end at byte {end} after byte 8, and those written at {}", self.end))),
            _ => {}
        }
        self.prepare(8 + self.end + bytes.len() as u64, &bytes)?;
        let (file, at) = (self.file(), 8 + self.end);
        let written = write_at(file, &bytes, at).and_then(|()| file.set_len(at + bytes.len() as u64)).and_then(|()| file.sync_all());
        written.map_err(|e| crate::about(&path, e.into()))?;
        stop(3);
        match &self.staged {
            Some((at, _)) => {
                moved_over(&self.locked.file, at, &path)?;
                self.locked.moved.store(true, Ordering::Relaxed);
            }
            None => {
                // A journal that cannot go would put the change back as the file is next opened
                // (one that adds arrays is never known finished): put back now, and said.
                let journaled = journal(&path);
                // One gone already (a delete NFS retried) is removed.
                std::fs::remove_file(&journaled).or_else(|e| if e.kind() == ErrorKind::NotFound { Ok(()) } else { Err(e) }).map_err(|e| invalid(format!("{} is as it was: the change was made, but its journal {} could not be removed ({}), so the change is put back: let files be removed from its folder, then try again", path.display(), journaled.display(), crate::described(&e))))?;
                synced(&path);
            }
        }
        self.finished = true;
        Ok(())
    }

    /// Before the first byte is written: the journal written beside the file (its length `after`
    /// the change and its `new` tail when it writes only a header, or 2^64 - 1 when it adds arrays),
    /// or, when it cannot be changed in place, the new file begun with its magic and arrays, copied
    /// byte for byte.
    fn prepare(&mut self, after: u64, new: &[u8]) -> Result<()> {
        if self.prepared {
            return Ok(());
        }
        match self.locked.anew() {
            None => self.journal(after, new)?,
            Some(_) => {
                let path = &self.locked.path;
                let (at, mut out) = staged(self.locked, ".anew")?;
                if let Err(e) = crate::write::copied(&self.locked.file, 0, self.start, path, &mut out, &at) {
                    let _ = std::fs::remove_file(&at);
                    return Err(e);
                }
                self.staged = Some((at, Arc::new(out)));
            }
        }
        self.prepared = true;
        Ok(())
    }

    /// Write the journal beside the file, before its first byte changes, with its length `after`
    /// the change and its `new` tail; one that cannot be written whole goes.
    fn journal(&mut self, after: u64, new: &[u8]) -> Result<()> {
        let mut kept = MAGIC.to_vec();
        for number in [self.size, self.start, after] {
            kept.extend(number.to_le_bytes());
        }
        kept.extend(named(&self.locked.file, self.start).map_err(|e| crate::about(&self.locked.path, e.into()))?);
        kept.extend(&self.tail);
        kept.extend(new);
        let sum = Sha256::digest(&kept);
        kept.extend(sum);
        let at = journal(&self.locked.path);
        // A file in the way at the journal's place is said, not the folder.
        let there = at.symlink_metadata().is_ok();
        let made = private(File::options().write(true).create(true).truncate(true), self.locked.file.metadata().ok()).open(&at);
        let ours = made.is_ok();
        let written = made.and_then(|mut out| std::io::Write::write_all(&mut out, &kept).and_then(|()| out.sync_all()));
        if let Err(e) = written {
            if ours {
                let _ = std::fs::remove_file(&at);
            }
            return Err(match e.kind() {
                ErrorKind::PermissionDenied if !there => invalid(shut(&self.locked.path, "the journal of the change")),
                _ => crate::about(&at, e.into()),
            });
        }
        synced(&self.locked.path);
        stop(2);
        Ok(())
    }
}

/// `staging` beside the file `locked`, its mode from the file locked, refused as one that cannot be changed where nothing may be written beside it.
pub(crate) fn staged(locked: &Locked, suffix: &str) -> Result<(PathBuf, File)> {
    let path = locked.path();
    staging(path, suffix, locked.file.metadata().ok()).map_err(|e| match e.kind() {
        ErrorKind::ReadOnlyFilesystem => invalid(format!("{} cannot be changed: its filesystem is read-only, so neither it nor a new file beside it can be written: copy it to a disk you may write, and change the copy", path.display())),
        ErrorKind::PermissionDenied => invalid(shut(path, "the new file that replaces it")),
        _ => crate::about(path, e.into()),
    })
}

/// Why the file at `path` cannot be changed when its folder may not be written, where `what` goes first.
fn shut(path: &Path, what: &str) -> String {
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    format!("{} cannot be changed: its folder may not be written, and {what} is written there first: give yourself the right to write it (chmod u+w {}, if it is yours), or copy the file to a folder you may write, and change the copy", path.display(), folder.display())
}

impl Drop for Change<'_> {
    fn drop(&mut self) {
        self.locked.changes.fetch_add(1, Ordering::Relaxed);
        if self.prepared && !self.finished {
            let _ = match &self.staged {
                Some((at, _)) => std::fs::remove_file(at).map_err(crate::Error::from),
                None => put_back(&self.locked.file, &self.locked.path, || Ok(()), true).map(drop),
            };
        }
    }
}

/// `options` making a file no wider open than the file `like` (only its owner's without one): a staged file or a journal holds a copy of it.
fn private(options: &mut std::fs::OpenOptions, like: Option<std::fs::Metadata>) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(options, like.map_or(0, |m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o666) | 0o600);
    #[cfg(not(unix))]
    let _ = like;
    options
}

/// How many files this program has staged: the N of `FILE.anew-PID-N` and `FILE.empty-PID-N`.
static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A new file beside `path` that no other writer shares, `FILE{suffix}-PID-N`, and its path; a name
/// taken (left by a crash of a program of this process ID) is passed over, never written over.
pub(crate) fn staging(path: &Path, suffix: &str, like: Option<std::fs::Metadata>) -> std::io::Result<(PathBuf, File)> {
    loop {
        let at = crate::write::beside(path, &format!("{suffix}-{}-{}", std::process::id(), STAGED.fetch_add(1, Ordering::Relaxed)));
        match private(File::options().read(true).write(true).create_new(true), like.clone()).open(&at) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(crate::tagged(&at, e)),
            Ok(file) => return Ok((at, file)),
        }
    }
}

/// Put the file back from the journal beside it, when there is one and `can` lets it be written:
/// its old tail written back and the file cut to its old length. A journal that does not check out
/// was cut short before the file was touched, and is let go; one of a change that finished is let
/// go once the file is synced, the file untouched; one of another file is refused. A change of
/// this program's that did not finish (`unfinished`) is put back even when its new tail is
/// written, as it may not be kept. What became of it.
fn put_back(file: &File, path: &Path, can: impl Fn() -> Result<()>, unfinished: bool) -> Result<Restored> {
    let journaled = journal(path);
    let kept = match std::fs::read(&journaled) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Restored::Nothing),
        held => held.map_err(|e| crate::about(&journaled, e.into()))?,
    };
    let restored = match parsed(&kept) {
        Some(journaled) => {
            let restored = match settled(file, path, &journaled, &can)? {
                Restored::Finished if unfinished => can().map(|()| Restored::PutBack)?,
                restored => restored,
            };
            if restored == Restored::Finished {
                file.sync_all().map_err(|e| crate::about(path, e.into()))?;
            }
            if restored == Restored::PutBack {
                let (_, size, start, _, tail, _) = journaled;
                let put = write_at(file, tail, start).and_then(|()| file.set_len(size)).and_then(|()| file.sync_all());
                put.map_err(|e| crate::about(path, e.into()))?;
            }
            restored
        }
        None => Restored::LetGo,
    };
    std::fs::remove_file(&journaled).map_err(|e| crate::about(&journaled, e.into()))?;
    synced(path);
    Ok(restored)
}

/// Whether the file at `path` (`file`) is the one `journaled` is of, a change to it cut short, to
/// be put back once `can` lets it be written (`PutBack`), or finished (`Finished`); else refused.
fn settled(file: &File, path: &Path, (name, size, start, after, tail, new): &Kept, can: impl Fn() -> Result<()>) -> Result<Restored> {
    let (shown, beside) = (path.display(), journal(path));
    let refused = || invalid(format!("{} is not of this {shown} ({shown} was replaced since a change to it was cut short): remove {} if {shown} is the file you want", beside.display(), beside.display()));
    let length = file.metadata()?.len();
    let Some(new) = new else {
        // A change that adds arrays is named by the bytes before its tail (its length after, 2^64 - 1, bounds nothing).
        if length < *start || length > (*size).max(*after) || named(file, *start)?.as_slice() != *name {
            return Err(refused());
        }
        can()?;
        return Ok(Restored::PutBack);
    };
    // A change that writes only a header leaves the file between its two lengths, each byte of its
    // tail the old tail's, the new one's, or 0 past the shorter length (a page a crash did not keep,
    // as the change or its putting back grew the file).
    if length < (*size).min(*after) || length > (*size).max(*after) || named(file, *start)?.as_slice() != *name {
        return Err(refused());
    }
    let mut now = vec![0; (length - start) as usize];
    read_at(file, &mut now, *start).map_err(|e| crate::about(path, e.into()))?;
    if now == *new {
        return Ok(Restored::Finished);
    }
    let torn = now.iter().enumerate().all(|(at, byte)| tail.get(at) == Some(byte) || new.get(at) == Some(byte) || (*byte == 0 && start + at as u64 >= (*size).min(*after)));
    if !torn {
        return Err(refused());
    }
    can()?;
    Ok(Restored::PutBack)
}

/// A journal read: the SHA-256 of the bytes before its file's tail, the file's old length, where
/// its tail began, its length after the change (2^64 - 1 when it adds arrays), the old tail, and
/// the new one when it writes only a header.
type Kept<'a> = (&'a [u8], u64, u64, u64, &'a [u8], Option<&'a [u8]>);

/// What a journal holds, when it checks out.
fn parsed(kept: &[u8]) -> Option<Kept<'_>> {
    let n = kept.len().checked_sub(32).filter(|n| *n >= 64)?;
    let (body, sum) = kept.split_at(n);
    if &body[..8] != MAGIC || Sha256::digest(body).as_slice() != sum {
        return None;
    }
    let number = |at: usize| u64::from_le_bytes(body[at..at + 8].try_into().unwrap_or_default());
    let (size, start, after, name, tails) = (number(8), number(16), number(24), &body[32..64], &body[64..]);
    let old = usize::try_from(size.checked_sub(start)?).ok()?;
    let new = match after {
        u64::MAX => 0,
        after => usize::try_from(after.checked_sub(start)?).ok()?,
    };
    if tails.len() != old.checked_add(new)? {
        return None;
    }
    let (tail, new) = tails.split_at(old);
    Some((name, size, start, after, tail, (after != u64::MAX).then_some(new)))
}

/// The SHA-256 of the up to 65,536 bytes of `file` before `start`, its tail's first byte: bytes a
/// change never touches, so they name the file across reboots, remounts and copies.
fn named(file: &File, start: u64) -> std::io::Result<[u8; 32]> {
    let from = start.saturating_sub(NAMED);
    let mut bytes = vec![0; (start - from) as usize];
    read_at(file, &mut bytes, from)?;
    Ok(Sha256::digest(&bytes).into())
}

/// The device and inode of an open file: which file it is.
fn identity(file: &File) -> std::io::Result<(u64, u64)> {
    file.metadata().map(|m| ids(&m))
}

/// The device and inode a file's metadata names (none where the system has no inodes).
pub(crate) fn ids(held: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    return (std::os::unix::fs::MetadataExt::dev(held), std::os::unix::fs::MetadataExt::ino(held));
    #[cfg(not(unix))]
    {
        let _ = held;
        (0, 0)
    }
}

/// Make the entries of the folder `path` is in durable, as far as the system lets it.
pub(crate) fn synced(path: &Path) {
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let _ = File::open(folder).and_then(|f| f.sync_all());
}

/// In a debug build, stop the process after step `step` of a change when `RVR_STOP_CHANGE` asks,
/// leaving what a change cut short there leaves (docs/SPEC.md §8.12).
fn stop(step: u8) {
    #[cfg(debug_assertions)]
    if std::env::var("RVR_STOP_CHANGE").ok().and_then(|n| n.parse().ok()) == Some(step) {
        eprintln!("Stopped after step {step} of the change, as RVR_STOP_CHANGE asks.");
        std::process::exit(99);
    }
    let _ = step;
}

#[cfg(test)]
mod tests {
    use super::{Change, Locked, Restored};

    // SPEC 8.12
    #[test]
    #[cfg(unix)]
    fn a_reader_let_go_while_this_program_holds_its_file_locked_keeps_its_descriptor_open_until_the_lock_lets_go() {
        use std::os::fd::AsRawFd;
        let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/rvr/valid/fixture.rvr")).unwrap();
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-kept.rvr", std::process::id()));
        std::fs::write(&path, &fixture).unwrap();
        let id = super::ids(&std::fs::metadata(&path).unwrap());
        // The inode the descriptor `fd` names now, as the system says (none once it is closed).
        let names = |fd: i32| std::fs::metadata(format!("/dev/fd/{fd}")).ok().map(|m| std::os::unix::fs::MetadataExt::ino(&m));
        // A reader opened first (a rowver.File in Python), then the file locked, then the reader let go by another thread.
        let reader = crate::rvr::Reader::open(&path).unwrap();
        let locked = Locked::take(&path).unwrap();
        std::thread::spawn(move || drop(reader)).join().unwrap();
        let fd = {
            let held = super::held();
            let (.., kept) = held.iter().find(|(token, ..)| *token == locked.token).unwrap();
            assert_eq!(kept.len(), 1, "the reader's descriptor, kept by the lock");
            kept[0].as_raw_fd()
        };
        assert_eq!(names(fd), Some(id.1), "open while the lock is held");
        drop(locked);
        assert_ne!(names(fd), Some(id.1), "closed once the lock let go");
        assert!(super::held().iter().all(|(_, at, ..)| *at != id));
        // A reader let go while no lock holds the file closes its descriptor at once.
        drop(crate::rvr::Reader::open(&path).unwrap());
        assert!(super::held().iter().all(|(_, at, ..)| *at != id));
        std::fs::remove_file(&path).unwrap();
    }

    /// The fixture written at a path of its own, and the lock of a filesystem that gives none.
    fn lockless(name: &str) -> (std::path::PathBuf, Locked) {
        let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/rvr/valid/fixture.rvr")).unwrap();
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-{name}.rvr", std::process::id()));
        std::fs::write(&path, &fixture).unwrap();
        let file = std::fs::File::options().read(true).write(true).open(&path).unwrap();
        (path.clone(), Locked { path, file: super::Handle::new(file), lockless: Some("Operation not supported".into()), unwritable: None, restored: Restored::Nothing, moved: false.into(), changes: 0.into(), token: 0 })
    }

    /// Another lock of the file at `path` on a filesystem that gives none, as another program's.
    fn lockless_at(path: &std::path::Path) -> Locked {
        let file = std::fs::File::options().read(true).write(true).open(path).unwrap();
        Locked { path: path.to_path_buf(), file: super::Handle::new(file), lockless: Some("Operation not supported".into()), unwritable: None, restored: Restored::Nothing, moved: false.into(), changes: 0.into(), token: 0 }
    }

    /// The files staged beside the file at `path`: `FILE.anew-…` and `FILE.empty-…`.
    fn staged_beside(path: &std::path::Path) -> Vec<String> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut staged: Vec<String> = std::fs::read_dir(path.parent().unwrap()).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&format!("{name}.anew-")) || n.starts_with(&format!("{name}.empty-"))).collect();
        staged.sort();
        staged
    }

    // SPEC 8.12
    #[test]
    fn two_writers_without_locks_stage_apart_and_the_second_to_move_over_is_refused() {
        let (path, first) = lockless("two-writers");
        let second = lockless_at(&path);
        let (mut one, mut two) = (Change::begin(&first).unwrap(), Change::begin(&second).unwrap());
        // Each begun at once, its magic and arrays copied into a new file of its own beside the file.
        one.prepare(u64::MAX, &[]).unwrap();
        two.prepare(u64::MAX, &[]).unwrap();
        assert_eq!(staged_beside(&path).len(), 2, "{:?}", staged_beside(&path));
        let with = |change: &Change, key: &str| {
            let mut header = change.header().clone();
            header[key] = serde_json::json!({});
            header
        };
        let (first_header, second_header) = (with(&one, "one"), with(&two, "two"));
        one.finish(&first_header).unwrap();
        let said = two.finish(&second_header).unwrap_err().to_string();
        assert!(said.contains(&format!("{} was replaced by another program while rvr held it: run the command again", path.display())), "{said}");
        crate::rvr::validate_file(&path).unwrap();
        let header = crate::rvr::Reader::open(&path).unwrap().header().clone();
        assert_eq!((header.get("one").is_some(), header.get("two").is_some()), (true, false), "the first change in it, not the second");
        assert_eq!(staged_beside(&path), Vec::<String>::new());
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    fn a_staged_file_left_by_a_crash_is_left_as_it_is_and_never_blocks_a_change() {
        let (path, locked) = lockless("left-by-a-crash");
        // Left by a crash of a program whose process ID this one has now, under the next names it would take.
        let next = super::STAGED.load(std::sync::atomic::Ordering::Relaxed);
        let left: Vec<std::path::PathBuf> = [".anew", ".empty"].iter().flat_map(|suffix| (next..next + 3).map(move |n| (suffix, n))).map(|(suffix, n)| crate::write::beside(&path, &format!("{suffix}-{}-{n}", std::process::id()))).collect();
        for at in &left {
            std::fs::write(at, b"left by a crash").unwrap();
            std::fs::set_permissions(at, std::os::unix::fs::PermissionsExt::from_mode(0o444)).unwrap();
        }
        super::set_tool(&locked, "k", serde_json::json!({})).unwrap();
        drop(locked);
        crate::write::empty(lockless_at(&path)).unwrap();
        assert!(crate::rvr::Reader::open(&path).unwrap().empty());
        assert!(left.iter().all(|at| std::fs::read(at).unwrap() == b"left by a crash"));
        assert_eq!(staged_beside(&path).len(), left.len());
        for at in left.iter().chain([&path]) {
            std::fs::remove_file(at).unwrap();
        }
    }

    // SPEC 8.12
    #[test]
    #[cfg(unix)]
    fn descriptors_refused_as_a_file_another_thread_holds_was_opened_are_kept_by_its_lock_and_closed_as_it_lets_go() {
        let (path, _) = lockless("race-kept");
        let open = || std::fs::read_dir("/dev/fd").unwrap().count();
        let before = open();
        // A thousand refusals in the race (the path looked at, then the file another thread holds
        // opened there), a hundred for each lock taken, so that few descriptors are open at once.
        for _ in 0..10 {
            let (taken, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
            let holder = {
                let path = path.clone();
                std::thread::spawn(move || {
                    let locked = Locked::take(&path).unwrap();
                    taken.0.send(locked.token).unwrap();
                    release.1.recv().unwrap();
                })
            };
            let token = taken.1.recv().unwrap();
            for _ in 0..100 {
                let file = std::fs::File::open(&path).unwrap();
                assert!(super::unheld(file, &path).unwrap_err().to_string().contains("another of its threads holds its lock"));
            }
            let kept = super::held().iter().find(|(held, ..)| *held == token).map(|(.., kept)| kept.len());
            assert_eq!(kept, Some(100), "each kept by the lock, open while it is held");
            release.0.send(()).unwrap();
            holder.join().unwrap();
        }
        // Other tests open and close files as this runs: a leak would be a thousand more.
        let after = open();
        assert!(after < before + 50, "{before} descriptors open before, {after} after");
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    fn a_journal_of_no_file_that_another_command_let_go_first_counts_as_let_go() {
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-let-go.rvr", std::process::id()));
        super::let_go_journal(&path).unwrap();
        // One that cannot be removed (a folder there) is told, naming it.
        std::fs::create_dir(super::journal(&path)).unwrap();
        let said = super::let_go_journal(&path).unwrap_err().to_string();
        assert!(said.contains(&super::journal(&path).display().to_string()), "{said}");
        std::fs::remove_dir(super::journal(&path)).unwrap();
    }

    // SPEC 8.12
    #[test]
    fn a_file_on_a_filesystem_without_locks_is_changed_anew() {
        let (path, locked) = lockless("lockless");
        let was = super::ids(&std::fs::metadata(&path).unwrap());
        assert_eq!(locked.anew().as_deref(), Some("its filesystem gives no lock: Operation not supported"));
        super::set_tool(&locked, "k", serde_json::json!({})).unwrap();
        assert_ne!(super::ids(&std::fs::metadata(&path).unwrap()), was, "a new file, moved over it");
        let header = crate::rvr::Reader::open(&path).unwrap().header().clone();
        assert_eq!(header["k"], serde_json::json!({}));
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    fn a_file_on_a_read_only_filesystem_is_locked_to_be_read_and_changed_anew() {
        let (path, _) = lockless("read-only-disk");
        let before = std::fs::read(&path).unwrap();
        // CVMFS, or a read-only mount: opened to be written, the file is refused as on a read-only disk.
        let open = |write: bool| match write {
            true => Err(std::io::Error::from(std::io::ErrorKind::ReadOnlyFilesystem)),
            false => std::fs::File::open(&path),
        };
        let locked = Locked::taken(&path, open, std::fs::File::try_lock).unwrap();
        assert_eq!((locked.unwritable(), locked.anew().as_deref()), (Some(std::io::ErrorKind::ReadOnlyFilesystem), Some("its filesystem is read-only")));
        assert_eq!(locked.reader().unwrap().header()["rowver"], 15);
        drop(locked);
        // A change to it cut short: told how it is put back.
        let held = Locked::take(&path).unwrap();
        let mut change = Change::begin(&held).unwrap();
        change.journal(u64::MAX, &[]).unwrap();
        std::mem::forget(change);
        drop(held);
        let said = Locked::taken(&path, open, std::fs::File::try_lock).unwrap_err().to_string();
        let shown = path.display();
        assert_eq!(said, format!("{shown} has the journal of a change to it cut short, and lies on a read-only filesystem, so it cannot be put back: mount it to be written, then run any rvr command on it."));
        assert!(Locked::take(&path).is_ok());
        assert_eq!((std::fs::read(&path).unwrap(), super::journal(&path).exists()), (before, false));
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    #[cfg(unix)]
    fn a_file_opened_only_to_be_read_whose_lock_fails_is_changed_anew_for_it_may_not_be_written() {
        let (path, _) = lockless("read-only-lock");
        // Linux NFS: an exclusive lock is an fcntl write lock, refused on a descriptor only read.
        let refused = |_: &std::fs::File| Err(std::fs::TryLockError::Error(std::io::Error::from_raw_os_error(9)));
        let open = |write: bool| match write {
            true => Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            false => std::fs::File::open(&path),
        };
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o444)).unwrap();
        assert_eq!(super::probe(&path), (Some(std::io::ErrorKind::PermissionDenied), Some("you may not write it".into())), "as rvr status tells it, holding no lock");
        let locked = Locked::taken(&path, open, refused).unwrap();
        assert_eq!((locked.unwritable(), locked.anew().as_deref()), (Some(std::io::ErrorKind::PermissionDenied), Some("you may not write it")));
        let was = super::ids(&std::fs::metadata(&path).unwrap());
        super::set_tool(&locked, "k", serde_json::json!({})).unwrap();
        assert_ne!(super::ids(&std::fs::metadata(&path).unwrap()), was, "a new file, moved over it");
        // Opened to be written, a lock refused so is the filesystem's: it gives none.
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let locked = Locked::taken(&path, |write| std::fs::File::options().read(true).write(write).open(&path), refused).unwrap();
        assert_eq!(locked.anew().as_deref(), Some("its filesystem gives no lock: Bad file descriptor"));
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    fn a_file_held_once_then_replaced_each_time_it_was_opened_is_told_replaced() {
        let (path, _) = lockless("replaced-each-time");
        let (other, _) = lockless("replaced-each-time-other");
        let tries = std::cell::Cell::new(0);
        let held_once = |file: &std::fs::File| match tries.replace(tries.get() + 1) {
            0 => Err(std::fs::TryLockError::WouldBlock),
            _ => file.try_lock(),
        };
        // Each time it is opened, the file at its path is another.
        let open = |_: bool| std::fs::File::open(&other);
        let said = Locked::taken(&path, open, held_once).unwrap_err().to_string();
        assert_eq!(said, format!("{} was replaced each time it was opened, so another program is writing it: wait for it to finish, then try again", path.display()));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&other).unwrap();
    }

    // SPEC 8.12
    #[test]
    #[cfg(unix)]
    fn a_lock_let_go_takes_only_its_own_entry_among_the_files_held() {
        let (path, other) = lockless("own-entry");
        let id = super::identity(&other.file).unwrap();
        let count = || super::held().iter().filter(|held| held.1 == id).count();
        let locked = Locked::take(&path).unwrap();
        assert_eq!(count(), 1);
        // Another lock of the same file, never among those held (as a test's), let go.
        drop(other);
        assert_eq!(count(), 1, "the file locked is held still");
        drop(locked);
        assert_eq!(count(), 0);
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    fn a_change_cut_short_on_a_filesystem_without_locks_is_put_back_without_one() {
        let (path, _) = lockless("put-back");
        let before = std::fs::read(&path).unwrap();
        // A change cut short where the file could be locked: journaled, then its tail written over.
        let held = Locked::take(&path).unwrap();
        let mut change = Change::begin(&held).unwrap();
        change.journal(u64::MAX, &[]).unwrap();
        std::mem::forget(change);
        drop(held);
        std::fs::OpenOptions::new().append(true).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, b"cut short")).unwrap();
        let file = std::fs::File::options().read(true).write(true).open(&path).unwrap();
        let locked = Locked { path: path.clone(), file: super::Handle::new(file), lockless: Some("Operation not supported".into()), unwritable: None, restored: Restored::Nothing, moved: false.into(), changes: 0.into(), token: 0 };
        assert_eq!(locked.put_back().unwrap(), Restored::PutBackWithoutLock);
        assert_eq!((std::fs::read(&path).unwrap(), super::journal(&path).exists()), (before, false));
        std::fs::remove_file(&path).unwrap();
    }
    // SPEC 8.12
    #[test]
    fn a_change_that_wrote_its_new_header_but_did_not_finish_is_put_back() {
        let (path, _) = lockless("unfinished");
        let before = std::fs::read(&path).unwrap();
        let held = Locked::take(&path).unwrap();
        let mut change = Change::begin(&held).unwrap();
        let mut header = change.header().clone();
        header["k"] = serde_json::json!({});
        let (bytes, at) = (super::opening(&header, crate::write::FEWER).unwrap(), 8 + change.end);
        change.prepare(at + bytes.len() as u64, &bytes).unwrap();
        // The new header written whole, as finish writes it, and the sync that would keep it failed.
        super::write_at(change.file(), &bytes, at).and_then(|()| change.file().set_len(at + bytes.len() as u64)).unwrap();
        drop(change);
        assert_eq!((std::fs::read(&path).unwrap(), super::journal(&path).exists()), (before, false));
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12
    #[test]
    #[cfg(target_os = "macos")]
    fn a_change_whose_journal_cannot_go_is_put_back_and_said() {
        let folder = std::env::temp_dir().join(format!("rowver-unit-{}-appended", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("f.rvr");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/rvr/valid/fixture.rvr"), &path).unwrap();
        // A folder entries may be added to but not taken from.
        let flag = |on: &str| assert!(std::process::Command::new("chflags").arg(on).arg(&folder).status().unwrap().success());
        let before = std::fs::read(&path).unwrap();
        flag("uappnd");
        // Nothing that may panic until the flag is cleared, or the folder could not be removed.
        let made = Locked::take(&path).and_then(|locked| super::set_tool(&locked, "k", serde_json::json!({})));
        let (left, now) = (super::journal(&path).exists(), std::fs::read(&path));
        flag("nouappnd");
        let said = made.unwrap_err().to_string();
        assert!(said.contains("is as it was: the change was made, but its journal") && said.contains("so the change is put back"), "{said}");
        assert!(left, "the journal could not go");
        assert_eq!(now.unwrap(), before);
        assert_eq!(Locked::take(&path).unwrap().restored, Restored::PutBack);
        assert!(!super::journal(&path).exists());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
