//! Read chosen columns of an `.rvr` file, leaving every other array compressed on disk.
//!
//! Ported from: RowVer 13.0.1 `rowver/derive.py` `Columns`, which decompresses a column and
//! checks it against its `sha256` only when it is asked for.
//! Documentation: `FORMAT.md` (repository root) ("Arrays", "Checksums")
//! Differences: the header's own checksums (`data.sha256`, `made.sha256`, `derived.sha256`)
//! and the sample-column values are checked when the file is opened. Per-sample array
//! checksums and column facts are not checked here; `validate_file` checks those.

use super::{checksum, count, digest, header_end, list, object, required, string, MAGIC};
use crate::numeric::width;
use crate::write::{data_checksum, made_checksum};
use crate::{invalid, Result};
use flate2::{Decompress, FlushDecompress, Status};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// An open `.rvr` file whose header has been read and whose arrays are read on request.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    header: Value,
    /// Where the arrays begin, and how many bytes they fill.
    start: u64,
    length: u64,
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

/// Undo the byte shuffle: the file holds every value's first byte, then every second byte.
fn unshuffle(shuffled: Vec<u8>, w: usize) -> Vec<u8> {
    let values = shuffled.len() / w;
    if w == 1 || values == 0 {
        return shuffled;
    }
    let mut bytes = vec![0; shuffled.len()];
    for (lane, source) in shuffled.chunks_exact(values).enumerate() {
        for (value, byte) in bytes.chunks_exact_mut(w).zip(source) {
            value[lane] = *byte;
        }
    }
    bytes
}

/// The one checksum lineage records for a column: its array's, or for a list column the
/// SHA-256 of its offsets' checksum followed by its content's.
pub fn column_checksum(record: &Value) -> Result<String> {
    if string(record, "kind")? != "jagged" {
        return Ok(checksum(record)?.to_owned());
    }
    let offsets = checksum(object(record, "offsets")?)?;
    let content = checksum(object(record, "content")?)?;
    Ok(digest(format!("{offsets}{content}").as_bytes()))
}

impl Reader {
    /// Check the header's checksums and sample columns without reading event arrays.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = std::fs::File::open(&path)?;
        let size = file.metadata()?.len();
        let mut prefix = [0; 12];
        file.read_exact(&mut prefix)
            .map_err(|_| invalid("Input is not a RowVer v13 .rvr container"))?;
        if &prefix[..8] != MAGIC {
            return Err(invalid("Input is not a RowVer v13 .rvr container"));
        }
        let start = header_end(&prefix, usize::try_from(size).unwrap_or(usize::MAX))?;
        let mut text = vec![0; start - 12];
        file.read_exact(&mut text)?;
        let header: Value = serde_json::from_slice(&text)?;
        if header.get("rowver").and_then(Value::as_u64) != Some(13) {
            return Err(invalid("This reader accepts only RowVer version 13"));
        }
        let (data, made) = (object(&header, "data")?, object(&header, "made")?);
        let sound = checksum(data)? == data_checksum(data)?
            && checksum(made)? == made_checksum(made)?
            && match header.get("derived") {
                Some(lineage) => checksum(lineage)? == made_checksum(lineage)?,
                None => true,
            };
        if !sound {
            return Err(invalid(format!(
                "{}: the header does not match its own checksums",
                path.display()
            )));
        }
        list(data, "cols")?;
        super::samples::columns(&header, data, made)?;
        Ok(Self {
            path,
            header,
            start: start as u64,
            length: size - start as u64,
        })
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

    /// Decompress one array and refuse it unless its bytes have the checksum its record states.
    fn array(&self, record: &Value, kind: &str, values: usize, label: &str) -> Result<Vec<u8>> {
        let w = width(kind)?;
        let expected = values
            .checked_mul(w)
            .ok_or_else(|| invalid(format!("{label}: byte count overflows")))?;
        let (at, size) = (count(record, "at")? as u64, count(record, "size")? as u64);
        if at.checked_add(size).is_none_or(|end| end > self.length) {
            return Err(invalid(format!(
                "{label}: compressed span exceeds the file"
            )));
        }
        let mut file = std::fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.start + at))?;
        // Deflate expands at most 1032 to 1, so a longer declared array is refused before any of
        // it is allocated: a damaged header cannot ask for more memory than its span could fill.
        if expected > (size as usize).saturating_mul(1032).saturating_add(64) {
            return Err(invalid(format!(
                "{label}: declares more bytes than its compressed span can hold"
            )));
        }
        let mut packed = vec![0; size as usize];
        file.read_exact(&mut packed)?;
        // One byte of room past the declared length, so a stream that holds more is caught.
        let mut shuffled = Vec::with_capacity(expected + 1);
        let status = Decompress::new(true)
            .decompress_vec(&packed, &mut shuffled, FlushDecompress::Finish)
            .map_err(|error| invalid(format!("{label}: invalid zlib stream: {error}")))?;
        if status != Status::StreamEnd && shuffled.len() < expected {
            return Err(invalid(format!(
                "{label}: invalid zlib stream: it ends early"
            )));
        }
        if status != Status::StreamEnd || shuffled.len() != expected {
            return Err(invalid(format!(
                "{label}: decoded length differs from the declared array"
            )));
        }
        let bytes = unshuffle(shuffled, w);
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

    /// Read one row column the file itself holds, or `None` when it holds none of that name.
    pub fn column(&self, name: &str) -> Result<Option<Held>> {
        let Some(record) = self.record(name) else {
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
