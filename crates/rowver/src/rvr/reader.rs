//! Read chosen columns of an `.rvr` file, leaving every other array compressed on disk.
//!
//! Documentation: `docs/FORMAT.md` ("Arrays", "Checksums").
//! Every fact of the header is checked when the file is opened (`checked_header`), and each
//! array against its `sha256` when it is read. Per-sample array checksums, column facts and
//! the event choice need every array; `validate_file` checks those.

use super::{count, text};
use crate::{invalid, Result};
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An open `.rvr` file whose header has been read and whose arrays are read on request.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    header: Value,
    /// The files that hold the arrays, in order, each read through the handle opened.
    parts: Vec<Part>,
}

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
    /// time of change then, to tell it changed in place since.
    Held(Arc<File>, (u64, Option<std::time::SystemTime>)),
    /// Opened while one of its arrays is read: a file a press laid, of its own.
    Laid(PathBuf),
}

impl Part {
    /// The arrays of `file` from `start`, `length` bytes of them, read through its handle.
    fn new(file: File, start: u64, length: u64) -> Result<Self> {
        let seen = stamp(&file)?;
        Ok(Self { from: Reading::Held(Arc::new(file), seen), start, length })
    }

    /// Whether the file was changed in place since it was opened (emptied, cut short, written over).
    fn changed(&self) -> bool {
        match &self.from {
            Reading::Held(file, seen) => stamp(file).ok().as_ref() != Some(seen),
            Reading::Laid(_) => false,
        }
    }
}

/// A file's length and time of change, as its handle sees them.
fn stamp(file: &File) -> Result<(u64, Option<std::time::SystemTime>)> {
    let held = file.metadata()?;
    Ok((held.len(), held.modified().ok()))
}

/// What a read of the file at `path`, changed in place since it was opened, fails with.
fn changed(path: &Path) -> crate::Error {
    crate::tagged(path, std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the file changed while it was read")).into()
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

impl Reader {
    /// Open a file and check every fact its header holds, without reading event arrays.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let (reader, checked) = Self::inspect(path)?;
        checked.map(|()| reader)
    }

    /// Open a file to look inside it, even one whose header fails its checks: the reader, and what checking the header found.
    pub fn inspect(path: impl AsRef<Path>) -> Result<(Self, Result<()>)> {
        let path = path.as_ref().to_path_buf();
        let (file, size, text) = super::head(&path)?;
        let start = 12 + text.len();
        let length = size - start;
        let size = size as u64;
        // Parsed once: strictly, or, when that fails, as it is, to look inside all the same.
        let (header, checked) = match crate::json::strict(&text) {
            Ok(header) => {
                let checked = super::checks(&header).and_then(|end| super::arrays::fill(end, length));
                (header, checked)
            }
            Err(e) => (serde_json::from_slice(&text)?, Err(e)),
        };
        let parts = vec![Part::new(file, start as u64, size - start as u64)?];
        Ok((Self { path, header, parts }, checked))
    }

    /// The file opened at `path` whose `header`, already checked, is followed by `length` bytes of arrays from `start`.
    pub(super) fn of(path: &Path, file: File, header: Value, start: u64, length: u64) -> Result<Self> {
        Ok(Self { path: path.to_path_buf(), header, parts: vec![Part::new(file, start, length)?] })
    }

    /// Read the arrays laid in `files` under `header`, checked as `open` checks it, before they are written.
    pub(crate) fn laid(dir: &Path, files: &[PathBuf], header: Value) -> Result<Self> {
        let (_, end) = super::checked(&serde_json::to_vec(&header)?)?;
        let mut parts = Vec::new();
        for path in files {
            let length = std::fs::metadata(path).map_err(|e| crate::about(path, e.into()))?.len();
            parts.push(Part { from: Reading::Laid(path.clone()), start: 0, length });
        }
        super::arrays::fill(end, parts.iter().map(|part| part.length as usize).sum())?;
        Ok(Self { path: dir.to_path_buf(), header, parts })
    }

    /// Whether the file is empty: its header alone, its arrays let go by `write::empty`.
    pub fn empty(&self) -> bool {
        self.parts.iter().all(|part| part.length == 0)
    }

    /// The header's text as the file opened holds it, read through the handle it was opened with.
    pub fn header_text(&self) -> Result<Vec<u8>> {
        let part = &self.parts[0];
        let Reading::Held(file, _) = &part.from else {
            return Err(invalid("the file is being written: its header has no text yet"));
        };
        let mut text = vec![0; part.start.saturating_sub(12) as usize];
        let mut span = Span { file: file.clone(), path: self.path.clone(), at: 12, end: part.start };
        span.read_exact(&mut text).map_err(|e| if part.changed() { changed(&self.path) } else { e.into() })?;
        Ok(text)
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
                let (file, path) = match &part.from {
                    Reading::Held(file, _) => (file.clone(), self.path.clone()),
                    Reading::Laid(path) => (Arc::new(File::open(path).map_err(|e| crate::about(path, e.into()))?), path.clone()),
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

    /// One array's compressed bytes, as they lie in the file opened, and how many.
    pub(crate) fn span(&self, record: &Value, label: &str) -> Result<(Span, u64)> {
        let (_, span) = self.part(record, label)?;
        let size = span.end - span.at;
        Ok((span, size))
    }

    /// Decompress one array, checked against its record's sha256; a file changed in place since it was opened is said to be.
    fn array(&self, record: &Value, kind: &str, values: usize, label: &str) -> Result<Vec<u8>> {
        let (part, mut span) = self.part(record, label)?;
        let mut packed = vec![0; (span.end - span.at) as usize];
        let read = span.read_exact(&mut packed).map_err(crate::Error::from).and_then(|()| super::arrays::unpacked(packed, record, kind, values, label));
        read.map_err(|e| if part.changed() { changed(&self.path) } else { e })
    }

    /// Each row's index into `data.samples`.
    pub fn sample(&self) -> Result<Vec<i32>> {
        let bytes = self.array(&self.data()["sample"], "i4", self.rows()?, "data.sample")?;
        let (values, _) = bytes.as_chunks::<4>();
        Ok(values
            .iter()
            .map(|value| i32::from_le_bytes(*value))
            .collect())
    }

    /// Each row's entry number in its source tree.
    pub fn entry(&self) -> Result<Vec<i64>> {
        let bytes = self.array(&self.data()["entry"], "i8", self.rows()?, "data.entry")?;
        let (values, _) = bytes.as_chunks::<8>();
        Ok(values
            .iter()
            .map(|value| i64::from_le_bytes(*value))
            .collect())
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
        let rows = self.rows()?;
        let place = format!("data.cols[{name:?}]");
        let kind = text(record, &place, "kind")?;
        if kind != "jagged" {
            return Ok(Held { kind: kind.to_owned(), bytes: self.array(record, kind, rows, &place)?, offsets: None });
        }
        let content = &record["content"];
        let (kind, values) = (text(content, &place, "kind")?, count(content, &place, "count")?);
        let label = format!("{place}.offsets");
        let offsets = super::jagged_offsets(&self.array(&record["offsets"], "i8", rows + 1, &label)?, values, &label)?;
        Ok(Held { kind: kind.to_owned(), bytes: self.array(content, kind, values, &format!("{place}.content"))?, offsets: Some(offsets) })
    }
}
