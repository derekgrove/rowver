//! Read chosen columns of an `.rvr` file, leaving every other array compressed on disk.
//!
//! Documentation: `docs/FORMAT.md` ("Arrays", "Checksums").
//! Every fact of the header is checked when the file is opened (`checked_header`), and each
//! array against its `sha256` when it is read. Per-sample array checksums, column facts and
//! the event choice need every array; `validate_file` checks those.

use super::{count, text};
use crate::change::Handle;
use crate::chunks::{Rows, Table};
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// An open `.rvr` file whose header has been read and whose arrays are read on request.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    header: Value,
    /// The files that hold the arrays, in order, each read through the handle opened.
    parts: Vec<Part>,
    /// Whether the file is empty: its arrays let go, as it was when opened.
    empty: bool,
    /// The file's chunks, made when first wanted, or why they cannot be.
    chunks: std::sync::OnceLock<std::result::Result<Arc<Vec<Rows>>, String>>,
    /// Each array's place among the arrays, by where its record lies in the header.
    places: HashMap<usize, usize>,
    /// The chunk tables a read of some chunks of an array checked, kept for the next, by place.
    tables: Mutex<HashMap<usize, Arc<Table>>>,
    /// The derived columns checked against their records.
    checked: Mutex<HashSet<String>>,
    /// The tail it was read from, when it was read under a file's lock (`Locked::inspect`).
    pub(crate) seen: Option<crate::change::Seen>,
}

/// An array read whole: its record, kind, place, and how many values each chunk holds.
pub(crate) type Whole<'a> = (&'a Value, &'a str, String, std::borrow::Cow<'a, [usize]>);

/// What is handed each chunk of arrays read whole: the array, the chunk, its values, and with an
/// array's first chunk the bytes its chunks decode to.
pub(crate) type Chunked<'f> = dyn FnMut(usize, usize, Vec<u8>, Option<usize>) -> Result<()> + 'f;

/// The fewest bytes a chunk takes in the arrays: its `sample` array's table entry and four frames.
const LEAST: u64 = 64 + 4 * crate::chunks::FRAME as u64;

/// One file of a Reader's arrays: where they begin, how many bytes they take, and how it is read.
#[derive(Debug)]
struct Part {
    from: Reading,
    start: u64,
    length: u64,
}

/// How a Part is read.
#[derive(Debug)]
enum Reading {
    /// Through the handle opened, whatever is renamed over the file since; and its length and
    /// time of change as it was opened, to tell it changed in place since.
    Held(Handle, Stamp),
    /// Through the handle of a file being written, its header still to come.
    Laid(Handle),
}

impl Part {
    /// The arrays of `file` from `start`, `length` bytes of them, read through its handle, the
    /// file `seen` as it was when it was opened.
    fn new(file: Handle, seen: Stamp, start: u64, length: u64) -> Self {
        Self { from: Reading::Held(file, seen), start, length }
    }

    /// Whether the file was changed in place since it was opened (emptied, cut short, written over).
    fn changed(&self) -> bool {
        match &self.from {
            Reading::Held(file, seen) => stamp(file).ok().as_ref() != Some(seen),
            Reading::Laid(_) => false,
        }
    }
}

/// A file's length and time of change.
pub(crate) type Stamp = (u64, Option<std::time::SystemTime>);

/// A file's length and time of change, as its handle sees them.
pub(crate) fn stamp(file: &File) -> Result<Stamp> {
    let held = file.metadata()?;
    Ok((held.len(), held.modified().ok()))
}

/// What a read of the file at `path`, changed in place since it was opened, fails with.
pub(crate) fn changed(path: &Path) -> crate::Error {
    crate::tagged(path, std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the file changed while it was read")).into()
}

/// A debug build's note of an array about to be read, `size` bytes of the file at `path` from
/// `at`, in the file `RVR_READ_LOG` names when it is set: so a test sees what a change reads.
#[cfg(debug_assertions)]
fn logged(path: &Path, at: u64, size: u64) {
    use std::io::Write;
    if let Some(log) = std::env::var_os("RVR_READ_LOG") {
        let line = format!("{}\t{at}\t{size}\n", std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).display());
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(log).and_then(|mut f| f.write_all(line.as_bytes()));
    }
}

/// One array's compressed bytes, read from where they lie in an open file; a failure names the file.
#[derive(Clone)]
pub(crate) struct Span {
    file: Arc<File>,
    path: PathBuf,
    at: u64,
    end: u64,
}

impl Read for Span {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let room = buffer.len().min(usize::try_from(self.end.saturating_sub(self.at)).unwrap_or(usize::MAX));
        #[cfg(unix)]
        let read = std::os::unix::fs::FileExt::read_at(&*self.file, &mut buffer[..room], self.at);
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(&*self.file, &mut buffer[..room], self.at);
        let read = read.map_err(|e| crate::tagged(&self.path, e))?;
        // Bytes its record lays out that are no longer there: the file was cut short since.
        if read == 0 && room > 0 {
            return Err(crate::tagged(&self.path, std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the file changed while it was read")));
        }
        self.at += read as u64;
        Ok(read)
    }
}

/// A chunk read: its SHA-256, its frames as the file holds them, and its values.
pub(crate) type ChunkRead = ([u8; 32], crate::chunks::Frames, Vec<u8>);

/// One array of an open file, read a chunk at a time on any thread.
#[derive(Clone)]
pub(crate) struct ArrayChunks<'r> {
    path: &'r Path,
    part: &'r Part,
    span: Span,
    table: Arc<Table>,
    label: String,
}

impl ArrayChunks<'_> {
    /// Chunk `k`, of `n` values: its SHA-256, its frames as the file holds them, and its values
    /// decoded and checked against its SHA-256.
    pub(crate) fn read(&self, k: usize, n: usize) -> Result<ChunkRead> {
        let read = || -> Result<_> {
            let (from, to) = self.table.span(k);
            let mut frames = vec![0; (to - from) as usize];
            Span { at: self.span.at + from, end: self.span.at + to, ..self.span.clone() }.read_exact(&mut frames)?;
            let values = crate::chunks::decode(&frames, &self.table, k, n, &self.label)?;
            Ok((self.table.sum(k), crate::chunks::Frames::Together(frames, self.table.sizes(k).to_vec()), values))
        };
        read().map_err(|e| if self.part.changed() { changed(self.path) } else { e })
    }
}

/// One column as the file holds it: little-endian values, and for a list column each row's bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    /// The kind of the values: `f4`, `i8`, `b1`.
    pub kind: String,
    /// The values, little-endian.
    pub bytes: Vec<u8>,
    /// For a list column, row `r` holds values `offsets[r]..offsets[r + 1]`.
    pub offsets: Option<Vec<usize>>,
}

impl Held {
    /// Rows `from..to` as they lie in it: their bytes, and a list column's offsets counted from 0;
    /// refused unless they lie in it.
    pub(super) fn part(&self, from: usize, to: usize) -> Result<(&[u8], Option<Vec<usize>>)> {
        let w = crate::numeric::width(&self.kind)?;
        let rows = self.offsets.as_ref().map_or(self.bytes.len() / w, |o| o.len().saturating_sub(1));
        let outside = || invalid(format!("rows {from} to {to} of a column of {rows} rows are asked for: ask for rows from 0 to {rows}, the first no later than the last"));
        if from > to || to > rows {
            return Err(outside());
        }
        let (a, b, offsets) = match &self.offsets {
            None => (from, to, None),
            Some(o) => (o[from], o[to], Some(o[from..=to].iter().map(|at| at.saturating_sub(o[from])).collect())),
        };
        Ok((self.bytes.get(a * w..b * w).ok_or_else(outside)?, offsets))
    }

    /// Its rows `from..to`: their values, and a list column's offsets counted from 0.
    pub fn rows(&self, from: usize, to: usize) -> Result<Held> {
        let (bytes, offsets) = self.part(from, to)?;
        Ok(Held { kind: self.kind.clone(), bytes: bytes.to_vec(), offsets })
    }
}

/// A list column's offsets put together a chunk at a time: where they end, each chunk's members,
/// and, when kept, every offset.
pub(super) struct Stitched {
    pub end: usize,
    pub members: Vec<usize>,
    pub kept: Option<Vec<usize>>,
}

impl Stitched {
    /// None yet, every offset kept when `keep`.
    pub fn new(keep: bool) -> Self {
        Stitched { end: 0, members: Vec::new(), kept: keep.then(|| vec![0]) }
    }

    /// One chunk's offsets, counted on from where those before it end.
    pub fn add(&mut self, values: &[u8], label: &str) -> Result<()> {
        let (chunk, members) = crate::chunks::offsets(values, label)?;
        let past = || invalid(format!("{label} add up past what an offset holds (2^63 − 1) (docs/FORMAT.md §4): the file is damaged; copy it again, or press its study again"));
        let end = self.end.checked_add(members).filter(|end| *end <= i64::MAX as usize).ok_or_else(past)?;
        if let Some(kept) = &mut self.kept {
            kept.extend(chunk[1..].iter().map(|at| self.end + at));
        }
        self.end = end;
        self.members.push(members);
        Ok(())
    }
}

/// Refuse a list column at `place` whose offsets end at `end`, not at its content's count.
pub(super) fn ends(end: usize, content: &Value, place: &str) -> Result<()> {
    let values = count(content, place, "count")?;
    match end == values {
        true => Ok(()),
        false => Err(invalid(format!("{place}.offsets end at {end}, and content.count is {values} (docs/FORMAT.md §4): the file is damaged; copy it again, or press its study again"))),
    }
}

impl Reader {
    /// Open a file and check every fact its header holds, without reading event arrays.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let (reader, checked) = Self::inspect(path)?;
        checked.map(|()| reader)
    }

    /// Open a file to look inside it, even one whose header fails its checks: the reader, and what checking the header found.
    pub fn inspect(path: impl AsRef<Path>) -> Result<(Self, Result<()>)> {
        let path = path.as_ref();
        let (file, size, text, seen) = super::head(path)?;
        Self::looked(path, file, seen, &text, size - 20 - text.len())
    }

    /// The file at `path` read through `file`, the descriptor that holds its lock, its header
    /// `text` from `start`: as `inspect` reads it.
    pub(crate) fn locked(path: &Path, file: Arc<File>, text: &[u8], start: u64) -> Result<(Self, Result<()>)> {
        let seen = stamp(&file)?;
        Self::looked(path, file.into(), seen, text, start.saturating_sub(8) as usize)
    }

    /// The file at `path`, opened as `file` (`seen`), its header `text` after `length` bytes of
    /// arrays: the reader, and what checking the header found.
    fn looked(path: &Path, file: Handle, seen: Stamp, text: &[u8], length: usize) -> Result<(Self, Result<()>)> {
        // Parsed once: strictly, or, when that fails, as it is, to look inside all the same.
        let (header, checked) = match crate::json::strict(text) {
            Ok(header) => {
                let checked = super::checks(&header).and_then(|end| super::arrays::fill(end, length));
                (header, checked)
            }
            Err(e) => (serde_json::from_slice(text)?, Err(e)),
        };
        let parts = vec![Part::new(file, seen, 8, length as u64)];
        Ok((Self::made(path.to_path_buf(), header, parts), checked))
    }

    /// The file opened at `path` (`seen` as it was opened) whose `header`, already checked, is followed by `length` bytes of arrays from `start`.
    pub(super) fn of(path: &Path, file: Handle, seen: Stamp, header: Value, start: u64, length: u64) -> Self {
        Self::made(path.to_path_buf(), header, vec![Part::new(file, seen, start, length)])
    }

    /// Read the arrays of the file at `path` that is being written, through `file`, its `end` bytes
    /// from byte 8, under `header`, checked as `open` checks it, before the header is written.
    pub(crate) fn laid(path: &Path, file: Arc<File>, written: u64, header: Value) -> Result<Self> {
        let (_, end) = super::checked(&serde_json::to_vec(&header)?).map_err(|e| match e {
            crate::Error::Breaks { fact, under: Some(key), .. } if !super::TOP.contains(&key.as_str()) => invalid(format!("the header rowver was about to write breaks docs/FORMAT.md §1: {fact}, in the tool key {key} it was given: change that tool key's data to follow §1")),
            crate::Error::Breaks { fact, .. } => invalid(format!("the header rowver was about to write breaks docs/FORMAT.md §1: {fact}, in what it was given to write: change that to follow §1")),
            e => e,
        })?;
        super::arrays::fill(end, usize::try_from(written).map_err(|_| invalid(crate::change::LONG))?)?;
        let parts = vec![Part { from: Reading::Laid(file.into()), start: 8, length: written }];
        Ok(Self::made(path.to_path_buf(), header, parts))
    }

    /// A reader of `header`'s arrays in `parts`.
    fn made(path: PathBuf, header: Value, parts: Vec<Part>) -> Self {
        let empty = parts.iter().all(|part| part.length == 0) && super::arrays::end(&header["data"]).is_none_or(|end| end > 0);
        let mut reader = Self { path, header, parts, empty, chunks: Default::default(), places: HashMap::new(), tables: Default::default(), checked: Default::default(), seen: None };
        // The records lie in the header's own allocations, which stay where they are as it moves.
        reader.places = super::arrays::records(&reader.header["data"]).iter().enumerate().map(|(at, (record, _, _))| (*record as *const Value as usize, at)).collect();
        reader
    }

    /// Whether the file is empty: its header alone, its arrays let go by `write::empty` (a file of
    /// no rows, whose arrays hold no bytes, is whole).
    pub fn empty(&self) -> bool {
        self.empty
    }

    /// The header's text as the file opened holds it, read through the handle it was opened with.
    pub fn header_text(&self) -> Result<Vec<u8>> {
        let part = &self.parts[0];
        let Reading::Held(file, (size, _)) = &part.from else {
            return Err(invalid("the file is being written: its header has no text yet"));
        };
        // The header lies after the arrays and before its length and the magic (docs/FORMAT.md §1).
        let start = part.start + part.length;
        let mut text = vec![0; size.saturating_sub(12).saturating_sub(start) as usize];
        let mut span = Span { file: Arc::clone(file), path: self.path.clone(), at: start, end: start + text.len() as u64 };
        let read = span.read_exact(&mut text);
        // A change in place since it was opened leaves other bytes there, which read without error.
        match part.changed() {
            true => Err(changed(&self.path)),
            false => read.map(|()| text).map_err(Into::into),
        }
    }

    /// Where the file is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The whole header.
    pub fn header(&self) -> &Value {
        &self.header
    }

    /// The header's `data` section.
    fn data(&self) -> &Value {
        &self.header["data"]
    }

    /// How many rows the file has.
    pub fn rows(&self) -> Result<usize> {
        count(self.data(), "data", "rows")
    }

    /// The record of a row column, when the file itself holds one of that name.
    pub fn record(&self, name: &str) -> Option<&Value> {
        let columns = self.data()["cols"].as_array()?;
        columns.iter().find(|column| column["name"] == name)
    }

    /// Where one array's compressed bytes lie: the part that holds them, and they from there.
    fn part(&self, record: &Value, label: &str) -> Result<(&Part, Span)> {
        if self.empty() {
            return Err(invalid(super::EMPTY));
        }
        let (mut at, size) = (count(record, label, "at")? as u64, count(record, label, "size")? as u64);
        for part in &self.parts {
            if at.checked_add(size).is_some_and(|end| end <= part.length) {
                let at = part.start + at;
                #[cfg(debug_assertions)]
                logged(&self.path, at, size);
                let (file, path) = match &part.from {
                    Reading::Held(file, _) | Reading::Laid(file) => (Arc::clone(file), self.path.clone()),
                };
                return Ok((part, Span { file, path, at, end: at + size }));
            }
            // An array lies whole in one part: one past this part's end is in a later one.
            match at.checked_sub(part.length) {
                Some(rest) => at = rest,
                None => break,
            }
        }
        Err(invalid(format!("{label}: its array lies past the end of the file: the file is damaged; copy it again")))
    }

    /// The file's chunks: each one's sample, first row and end row; refused for an empty file, and
    /// when its header's rows make more chunks than its arrays could hold, before any is made.
    pub(crate) fn chunks(&self) -> Result<Arc<Vec<Rows>>> {
        if self.empty {
            return Err(invalid(super::EMPTY));
        }
        let made = self.chunks.get_or_init(|| {
            let length: u64 = self.parts.iter().map(|part| part.length).sum();
            match crate::chunks::count(self.data()) {
                Some(n) if n <= length / LEAST => crate::chunks::of(self.data()).map(Arc::new).map_err(|e| e.to_string()),
                Some(n) => Err(format!("its header's rows make {n} chunks, more than its {length} bytes of arrays could hold (docs/FORMAT.md §7): the file is damaged; copy it again, or press its study again")),
                None => Err("data.samples' rows add up past what a count holds (2^64): the file is damaged; copy it again, or press its study again".into()),
            }
        });
        made.clone().map_err(invalid)
    }

    /// The rows the file's arrays hold: those its chunks cover (`data.rows` for an empty file).
    fn held_rows(&self) -> Result<usize> {
        match self.empty {
            true => self.rows(),
            false => Ok(self.chunks()?.last().map_or(0, |chunk| chunk.2)),
        }
    }

    /// Refuse rows `from..to` of `what` unless they lie in the file, the first no later than the last.
    pub(super) fn within(&self, what: &str, from: usize, to: usize) -> Result<()> {
        let rows = self.held_rows()?;
        match from <= to && to <= rows {
            true => Ok(()),
            false => Err(outside(what, from, to, rows)),
        }
    }

    /// Rows `from..to` of `what`, asked for as signed numbers: refused unless they lie in the file.
    pub fn asked(&self, what: &str, from: i64, to: i64) -> Result<(usize, usize)> {
        let rows = self.held_rows()?;
        match (usize::try_from(from), usize::try_from(to)) {
            (Ok(first), Ok(end)) if first <= end && end <= rows => Ok((first, end)),
            _ => Err(outside(what, from, to, rows)),
        }
    }

    /// Whether derived column `name` was checked against its record; with `now`, that it is.
    pub(super) fn checked(&self, name: &str, now: bool) -> bool {
        let mut checked = self.checked.lock().unwrap_or_else(|e| e.into_inner());
        if now {
            checked.insert(name.to_owned());
        }
        checked.contains(name)
    }

    /// One array's chunk table, read and checked against its record (its size, sha256 and
    /// by_sample), or as an earlier read of some of its chunks kept it; kept when `keep`.
    fn table(&self, record: &Value, kind: &str, label: &str, span: &Span, keep: bool) -> Result<Arc<Table>> {
        // By the array's place among the arrays, as `at` is not one place: a laid file's arrays
        // each begin a part of their own, and a file of no rows has every array at 0.
        let place = self.places.get(&(record as *const Value as usize)).copied();
        if let Some(held) = place.and_then(|place| self.tables.lock().unwrap_or_else(|e| e.into_inner()).get(&place).cloned()) {
            return Ok(held);
        }
        let rows = self.chunks()?;
        let samples = self.data()["samples"].as_array().map_or(0, Vec::len);
        let w = crate::numeric::width(kind)?;
        let size = span.end - span.at;
        let table_bytes = (rows.len() as u64).saturating_mul(crate::chunks::entry(w) as u64);
        let start = size.checked_sub(table_bytes).ok_or_else(|| invalid(format!("{label}: its {size} bytes cannot hold its chunk table of {table_bytes} (docs/FORMAT.md §7): the file is damaged; copy it again, or press its study again")))?;
        let mut held = vec![0; table_bytes as usize];
        Span { at: span.at + start, ..span.clone() }.read_exact(&mut held)?;
        let table = crate::chunks::table(&held, size, w, rows.len(), label)?;
        crate::chunks::agree(record, &table, &rows, samples, label)?;
        let table = Arc::new(table);
        if let Some(place) = place.filter(|_| keep) {
            self.tables.lock().unwrap_or_else(|e| e.into_inner()).insert(place, table.clone());
        }
        Ok(table)
    }

    /// One array of `kind`, its chunk table read and checked against its record, to read a chunk
    /// at a time on any thread.
    pub(crate) fn array_chunks(&self, record: &Value, kind: &str, label: &str) -> Result<ArrayChunks<'_>> {
        let (part, span) = self.part(record, label)?;
        let table = self.table(record, kind, label, &span, false).map_err(|e| if part.changed() { changed(&self.path) } else { e })?;
        Ok(ArrayChunks { path: &self.path, part, span, table, label: label.into() })
    }

    /// Each chunk of one array of `kind` (only those `wanted`, when given), in order, `n(k)` values
    /// in chunk `k`: decoded in one pool of threads a few ahead of `each`, checked, and handed to
    /// it, its table checked first (and kept for the next read of some of its chunks); a file
    /// changed in place since it was opened is said to be.
    pub(crate) fn each_chunk(&self, record: &Value, kind: &str, label: &str, n: &dyn Fn(usize) -> usize, wanted: Option<std::ops::Range<usize>>, each: &mut dyn FnMut(usize, Vec<u8>) -> Result<()>) -> Result<()> {
        let (part, span) = self.part(record, label)?;
        let read = || -> Result<()> {
            let table = self.table(record, kind, label, &span, wanted.is_some())?;
            let wanted = wanted.unwrap_or(0..table.len());
            let first = wanted.start.min(table.len());
            let counts: Vec<usize> = (first..wanted.end.min(table.len())).map(n).collect();
            crate::parallel::ordered(counts.len(), crate::parallel::cores(), |at| {
                let k = first + at;
                let (from, to) = table.span(k);
                let mut frames = vec![0; (to - from) as usize];
                Span { at: span.at + from, end: span.at + to, ..span.clone() }.read_exact(&mut frames)?;
                let decoded = crate::chunks::decode(&frames, &table, k, counts[at], label)?;
                Ok((k, decoded))
            }, |(k, values)| each(k, values))
        };
        read().map_err(|e| if part.changed() { changed(&self.path) } else { e })
    }

    /// Every chunk of every one of `arrays`, decoded in one pool of threads a few ahead of `each`,
    /// which is handed them in order, each checked; each array's table read with its first chunk
    /// and let go after its last.
    pub(crate) fn each_chunks(&self, arrays: &[Whole], each: &mut Chunked) -> Result<()> {
        let chunks = self.chunks()?.len();
        let placed = arrays.iter().map(|(record, _, label, _)| self.part(record, label)).collect::<Result<Vec<_>>>()?;
        // A file of no rows has no chunks to read a table with: each is checked all the same.
        for ((record, kind, label, _), (part, span)) in arrays.iter().zip(&placed).filter(|_| chunks == 0) {
            self.table(record, kind, label, span, false).map_err(|e| if part.changed() { changed(&self.path) } else { e })?;
        }
        let tables: Vec<Mutex<Option<Arc<Table>>>> = arrays.iter().map(|_| Mutex::new(None)).collect();
        let work = |at: usize| {
            let (a, k) = (at / chunks, at % chunks);
            let ((record, kind, label, n), (part, span)) = (&arrays[a], &placed[a]);
            let read = || -> Result<(usize, usize, Vec<u8>, Option<usize>)> {
                let table = {
                    let mut held = tables[a].lock().unwrap_or_else(|e| e.into_inner());
                    match held.as_ref() {
                        Some(table) => table.clone(),
                        None => held.insert(self.table(record, kind, label, span, false)?).clone(),
                    }
                };
                let (from, to) = table.span(k);
                let mut frames = vec![0; (to - from) as usize];
                Span { at: span.at + from, end: span.at + to, ..span.clone() }.read_exact(&mut frames)?;
                let count = |k: usize| n.get(k).copied().unwrap_or(0);
                let decoded = crate::chunks::decode(&frames, &table, k, count(k), label)?;
                Ok((a, k, decoded, (k == 0).then(|| crate::chunks::decoded(&table, &count))))
            };
            read().map_err(|e| if part.changed() { changed(&self.path) } else { e })
        };
        crate::parallel::ordered(arrays.len() * chunks, crate::parallel::cores(), work, |(a, k, values, bytes)| {
            // Every chunk of the array is read: none needs its table again.
            if k + 1 == chunks {
                *tables[a].lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            each(a, k, values, bytes)
        })
    }

    /// Stored columns by name, every chunk of every one decoded in one pool of threads; `None` for a name it stores no column of.
    pub fn stored_columns(&self, names: &[String]) -> Result<Vec<Option<Held>>> {
        let places: Vec<String> = names.iter().map(|name| format!("data.cols[{name:?}]")).collect();
        let records: Vec<Option<&Value>> = names.iter().map(|name| self.record(name)).collect();
        if records.iter().all(Option::is_none) {
            return Ok(vec![None; names.len()]);
        }
        let rows = self.chunks()?;
        let (per, more) = (Self::rows_of(&rows), rows.iter().map(|c| c.2 - c.1 + 1).collect::<Vec<_>>());
        // The flat columns' arrays and the list columns' offsets, then the list columns' content.
        let (mut first, mut at): (Vec<Whole>, Vec<usize>) = (Vec::new(), Vec::new());
        for (index, (record, place)) in records.iter().zip(&places).enumerate().filter_map(|(i, (r, p))| Some((i, (r.as_ref()?, p)))) {
            let jagged = record["kind"] == "jagged";
            first.push(match jagged {
                true => (&record["offsets"], "i8", format!("{place}.offsets"), (&more[..]).into()),
                false => (*record, text(record, place, "kind")?, place.clone(), (&per[..]).into()),
            });
            at.push(index);
        }
        let mut held: Vec<Option<Held>> = vec![None; names.len()];
        let mut stitched: Vec<Option<Stitched>> = first.iter().map(|(_, kind, label, _)| (label.ends_with(".offsets") && *kind == "i8").then(|| Stitched::new(true))).collect();
        let mut bytes: Vec<Vec<u8>> = first.iter().map(|_| Vec::new()).collect();
        self.each_chunks(&first, &mut |a, _, values, whole| {
            match &mut stitched[a] {
                Some(stitched) => stitched.add(&values, &first[a].2)?,
                None => {
                    bytes[a].reserve(whole.unwrap_or(0));
                    bytes[a].extend_from_slice(&values);
                }
            }
            Ok(())
        })?;
        let mut contents: Vec<Whole> = Vec::new();
        let mut content_at = Vec::new();
        for (a, ((index, values), stitched)) in at.iter().zip(bytes).zip(stitched).enumerate() {
            let (record, place) = (records[*index].unwrap_or(&Value::Null), &places[*index]);
            let Some(stitched) = stitched else {
                held[*index] = Some(Held { kind: first[a].1.to_owned(), bytes: values, offsets: None });
                continue;
            };
            let content = &record["content"];
            ends(stitched.end, content, place)?;
            let kind = text(content, place, "kind")?;
            contents.push((content, kind, format!("{place}.content"), stitched.members.into()));
            held[*index] = Some(Held { kind: kind.to_owned(), bytes: Vec::new(), offsets: stitched.kept });
            content_at.push(*index);
        }
        self.each_chunks(&contents, &mut |a, _, values, whole| {
            let column = held[content_at[a]].as_mut().map(|column| &mut column.bytes);
            if let Some(column) = column {
                column.reserve(whole.unwrap_or(0));
                column.extend_from_slice(&values);
            }
            Ok(())
        })?;
        Ok(held)
    }

    /// One array of `kind`, whole: its chunks' values, in order.
    pub(super) fn array(&self, record: &Value, kind: &str, label: &str, n: &[usize]) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.each_chunks(&[(record, kind, label.to_owned(), n.into())], &mut |_, _, values, whole| {
            bytes.reserve(whole.unwrap_or(0));
            bytes.extend_from_slice(&values);
            Ok(())
        })?;
        Ok(bytes)
    }

    /// Each chunk's rows.
    pub(super) fn rows_of(rows: &[Rows]) -> Vec<usize> {
        rows.iter().map(|chunk| chunk.2 - chunk.1).collect()
    }

    /// The chunks holding rows `from..to`, and the first row of the first of them.
    fn wanted(rows: &[Rows], from: usize, to: usize) -> (std::ops::Range<usize>, usize) {
        let wanted = rows.partition_point(|c| c.2 <= from)..rows.partition_point(|c| c.1 < to);
        let first = rows.get(wanted.start).map_or(from, |c| c.1);
        (wanted, first)
    }

    /// Each chunk's first and end row, in order (docs/FORMAT.md §7).
    pub fn chunk_spans(&self) -> Result<Vec<(usize, usize)>> {
        Ok(self.chunks()?.iter().map(|(_, first, end)| (*first, *end)).collect())
    }

    /// Values `from..to` of the `sample` or `entry` array (`kind`), only the chunks holding them read.
    fn array_slice(&self, which: &str, kind: &str, from: usize, to: usize) -> Result<Vec<u8>> {
        self.within(which, from, to)?;
        let rows = self.chunks()?;
        let (wanted, first) = Self::wanted(&rows, from, to);
        let mut bytes = Vec::new();
        self.each_chunk(&self.data()[which], kind, &format!("data.{which}"), &|k| rows[k].2 - rows[k].1, Some(wanted), &mut |_, values| {
            bytes.extend(values);
            Ok(())
        })?;
        Ok(Held { kind: kind.into(), bytes, offsets: None }.rows(from - first, to - first)?.bytes)
    }

    /// Rows `from..to`'s indices into `data.samples`.
    pub fn sample_slice(&self, from: usize, to: usize) -> Result<Vec<i32>> {
        let bytes = self.array_slice("sample", "i4", from, to)?;
        Ok(bytes.as_chunks::<4>().0.iter().map(|v| i32::from_le_bytes(*v)).collect())
    }

    /// Rows `from..to`'s entry numbers in their source trees.
    pub fn entry_slice(&self, from: usize, to: usize) -> Result<Vec<i64>> {
        let bytes = self.array_slice("entry", "i8", from, to)?;
        Ok(bytes.as_chunks::<8>().0.iter().map(|v| i64::from_le_bytes(*v)).collect())
    }

    /// Each row's index into `data.samples`.
    pub fn sample(&self) -> Result<Vec<i32>> {
        let bytes = self.array(&self.data()["sample"], "i4", "data.sample", &Self::rows_of(&self.chunks()?))?;
        Ok(bytes.as_chunks::<4>().0.iter().map(|v| i32::from_le_bytes(*v)).collect())
    }

    /// Each row's entry number in its source tree.
    pub fn entry(&self) -> Result<Vec<i64>> {
        let bytes = self.array(&self.data()["entry"], "i8", "data.entry", &Self::rows_of(&self.chunks()?))?;
        Ok(bytes.as_chunks::<8>().0.iter().map(|v| i64::from_le_bytes(*v)).collect())
    }

    /// Read one row column, stored or derived (worked out and checked), or `None` when there is none.
    pub fn column(&self, name: &str) -> Result<Option<Held>> {
        let Some(record) = self.record(name) else {
            if super::derived::listed(&self.header).iter().any(|(n, _)| n == name) {
                return super::derived::columns(self, &[name.to_owned()]).map(|mut held| held.pop());
            }
            return Ok(None);
        };
        self.stored(name, record).map(Some)
    }

    /// The stored column `name`, read by its record.
    pub(super) fn stored(&self, name: &str, record: &Value) -> Result<Held> {
        let rows = self.chunks()?;
        let place = format!("data.cols[{name:?}]");
        let kind = text(record, &place, "kind")?;
        if kind != "jagged" {
            return Ok(Held { kind: kind.to_owned(), bytes: self.array(record, kind, &place, &Self::rows_of(&rows))?, offsets: None });
        }
        let content = &record["content"];
        let kind = text(content, &place, "kind")?;
        let stitched = self.offsets_of(&record["offsets"], &rows, &format!("{place}.offsets"), None)?;
        ends(stitched.end, content, &place)?;
        let bytes = self.array(content, kind, &format!("{place}.content"), &stitched.members)?;
        Ok(Held { kind: kind.to_owned(), bytes, offsets: stitched.kept })
    }

    /// Rows `from..to` of a row column, stored or derived, or `None` when there is none: only the chunks holding them read and checked.
    pub fn slice(&self, name: &str, from: usize, to: usize) -> Result<Option<Held>> {
        match self.record(name) {
            Some(record) => self.stored_rows(name, record, from, to).map(Some),
            None if super::derived::listed(&self.header).iter().any(|(n, _)| n == name) => Ok(super::derived::slice(self, &[name.to_owned()], from, to)?.pop()),
            None => Ok(None),
        }
    }

    /// Rows `from..to` of the stored column `name`, by its record.
    pub(super) fn stored_rows(&self, name: &str, record: &Value, from: usize, to: usize) -> Result<Held> {
        self.within(name, from, to)?;
        let rows = self.chunks()?;
        let place = format!("data.cols[{name:?}]");
        let kind = text(record, &place, "kind")?;
        let (wanted, first) = Self::wanted(&rows, from, to);
        let mut bytes = Vec::new();
        let mut gather = |_: usize, values: Vec<u8>| {
            bytes.extend(values);
            Ok(())
        };
        if kind != "jagged" {
            self.each_chunk(record, kind, &place, &|k| rows[k].2 - rows[k].1, Some(wanted), &mut gather)?;
            return Held { kind: kind.to_owned(), bytes, offsets: None }.rows(from - first, to - first);
        }
        let content = &record["content"];
        let kind = text(content, &place, "kind")?;
        let stitched = self.offsets_of(&record["offsets"], &rows, &format!("{place}.offsets"), Some(wanted.clone()))?;
        let start = wanted.start;
        self.each_chunk(content, kind, &format!("{place}.content"), &|k| stitched.members.get(k - start).copied().unwrap_or(0), Some(wanted), &mut gather)?;
        Held { kind: kind.to_owned(), bytes, offsets: stitched.kept }.rows(from - first, to - first)
    }

    /// A list column's offsets from its chunks (only those `wanted`, when given, counted from the
    /// first of them; `[0]` for none): where they end, each chunk's members, and every offset.
    pub(super) fn offsets_of(&self, record: &Value, rows: &[Rows], label: &str, wanted: Option<std::ops::Range<usize>>) -> Result<Stitched> {
        let mut stitched = Stitched::new(true);
        self.each_chunk(record, "i8", label, &|k| rows[k].2 - rows[k].1 + 1, wanted, &mut |_, values| stitched.add(&values, label))?;
        Ok(stitched)
    }
}

/// The refusal of rows `from..to` of `what` that do not lie in a file of `rows` rows.
fn outside(what: &str, from: impl std::fmt::Display, to: impl std::fmt::Display, rows: usize) -> crate::Error {
    invalid(format!("rows {from} to {to} of {what} are asked for, and the file has {rows}: ask for rows from 0 to {rows}, the first no later than the last"))
}
