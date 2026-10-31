//! Read chosen columns of an `.rvr` file, leaving every other array compressed on disk.
//!
//! Documentation: `docs/FORMAT.md` ("Arrays", "Checksums").
//! Every fact of the header is checked when the file is opened (`checked_header`), and each
//! array against its `sha256` when it is read. Per-sample array checksums, column facts and
//! the event choice need every array; `validate_file` checks those.

use super::{checked_header, checksum, count, digest, header_end, object, required, string};
use crate::numeric::width;
use crate::{invalid, Result};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// An open `.rvr` file whose header has been read and whose arrays are read on request.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    header: Value,
    /// The files that hold the arrays, in order: each with where its bytes begin, and how many.
    parts: Vec<(PathBuf, u64, u64)>,
}

/// One column as the file holds it: little-endian values, and for a list column each row's bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    /// The kind of the values: `f4`, `i8`, `b1`.
    pub kind: String,
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

    /// Open a file to look inside it, even one whose header fails its checks: the reader, and
    /// what checking the header found.
    pub fn inspect(path: impl AsRef<Path>) -> Result<(Self, Result<()>)> {
        let path = path.as_ref().to_path_buf();
        let mut file = std::fs::File::open(&path)?;
        let size = file.metadata()?.len();
        let mut prefix = [0; 12];
        file.read_exact(&mut prefix)
            .map_err(|_| invalid(super::NOT_RVR))?;
        let start = header_end(&prefix, usize::try_from(size).unwrap_or(usize::MAX))?;
        let mut text = vec![0; start - 12];
        file.read_exact(&mut text)?;
        let header: Value = serde_json::from_slice(&text)?;
        let checked = checked_header(&text).map(drop);
        let parts = vec![(path.clone(), start as u64, size - start as u64)];
        Ok((Self { path, header, parts }, checked))
    }

    /// Read the arrays laid in `files` under `header`, checked as `open` checks it, before they are written.
    pub(crate) fn laid(dir: &Path, files: &[PathBuf], header: Value) -> Result<Self> {
        checked_header(&serde_json::to_vec(&header)?)?;
        let mut parts = Vec::new();
        for file in files {
            parts.push((file.clone(), 0, std::fs::metadata(file)?.len()));
        }
        Ok(Self { path: dir.to_path_buf(), header, parts })
    }

    /// Whether the file is empty: its header alone, its arrays let go by `write::empty`.
    pub fn empty(&self) -> bool {
        self.parts.iter().all(|(_, _, length)| *length == 0)
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
        count(self.data(), "rows")
    }

    /// The record of a row column, when the file itself holds one of that name.
    pub fn record(&self, name: &str) -> Option<&Value> {
        let columns = self.data()["cols"].as_array()?;
        columns.iter().find(|column| column["name"] == name)
    }

    /// The record of a sample column, when the file holds one of that name.
    pub fn sample_record(&self, name: &str) -> Option<&Value> {
        let columns = self.data()["sample_cols"].as_array()?;
        columns.iter().find(|column| column["name"] == name)
    }

    /// Where one array's compressed bytes lie: the file that holds them, their offset, their size.
    pub(crate) fn span(&self, record: &Value, label: &str) -> Result<(&Path, u64, u64)> {
        if self.empty() {
            return Err(invalid(super::EMPTY));
        }
        let (mut at, size) = (count(record, "at")? as u64, count(record, "size")? as u64);
        for (path, start, length) in &self.parts {
            if at.checked_add(size).is_some_and(|end| end <= *length) {
                return Ok((path, start + at, size));
            }
            // An array lies whole in one part: one past this part's end is in a later one.
            match at.checked_sub(*length) {
                Some(rest) => at = rest,
                None => break,
            }
        }
        Err(invalid(format!("{label}: compressed span exceeds the file")))
    }

    /// Decompress one array and refuse it unless its bytes have the checksum its record states.
    fn array(&self, record: &Value, kind: &str, values: usize, label: &str) -> Result<Vec<u8>> {
        let w = width(kind)?;
        let expected = values
            .checked_mul(w)
            .ok_or_else(|| invalid(format!("{label}: byte count overflows")))?;
        let (path, at, size) = self.span(record, label)?;
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(at))?;
        let mut packed = vec![0; size as usize];
        file.read_exact(&mut packed)?;
        let bytes = crate::codec::unpack(packed, w, expected, label)?;
        if digest(&bytes) != checksum(record)? {
            return Err(invalid(format!(
                "{label} does not match its sha256; validate the file"
            )));
        }
        Ok(bytes)
    }

    /// Each row's index into `data.samples`.
    pub fn sample(&self) -> Result<Vec<i32>> {
        let bytes = self.array(
            required(self.data(), "sample")?,
            "i4",
            self.rows()?,
            "sample",
        )?;
        let (values, _) = bytes.as_chunks::<4>();
        Ok(values
            .iter()
            .map(|value| i32::from_le_bytes(*value))
            .collect())
    }

    /// Each row's entry number in its source tree.
    pub fn entry(&self) -> Result<Vec<i64>> {
        let bytes = self.array(required(self.data(), "entry")?, "i8", self.rows()?, "entry")?;
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
        let rows = self.rows()?;
        let kind = string(record, "kind")?;
        if kind != "jagged" {
            return Ok(Some(Held {
                kind: kind.to_owned(),
                bytes: self.array(record, kind, rows, name)?,
                offsets: None,
            }));
        }
        let content = object(record, "content")?;
        let (kind, values) = (string(content, "kind")?, count(content, "count")?);
        let label = format!("{name} offsets");
        let steps = self.array(object(record, "offsets")?, "i8", rows + 1, &label)?;
        let (steps, _) = steps.as_chunks::<8>();
        let mut offsets = Vec::with_capacity(steps.len());
        for step in steps {
            let offset = usize::try_from(i64::from_le_bytes(*step)).ok();
            let ordered = offset.filter(|offset| {
                *offset <= values && offsets.last().is_none_or(|before| before <= offset)
            });
            offsets.push(ordered.ok_or_else(|| {
                invalid(format!("{name}: jagged offsets decrease or exceed content"))
            })?);
        }
        if offsets.first() != Some(&0) || offsets.last() != Some(&values) {
            return Err(invalid(format!(
                "{name}: offsets must start at zero and end at content count"
            )));
        }
        Ok(Some(Held {
            kind: kind.to_owned(),
            bytes: self.array(content, kind, values, &format!("{name} content"))?,
            offsets: Some(offsets),
        }))
    }
}
