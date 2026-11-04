use super::{checksum, count, digest, exactly, MEND};
use crate::numeric::width;
use crate::write::NumericBuffer;
use crate::{invalid, Result};
use serde_json::Value;
use std::io::Read;

/// Where an array record is in the header, written out only when it is said.
pub(crate) enum Place<'a> {
    Named(&'static str),
    /// A column, by name or index, and its `offsets` or `content` for a jagged one.
    Column(Option<&'a str>, usize, Option<&'static str>),
}

impl std::fmt::Display for Place<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Place::Named(name) => f.write_str(name),
            Place::Column(name, index, part) => {
                match name {
                    Some(name) => write!(f, "data.cols[{name:?}]")?,
                    None => write!(f, "data.cols[{index}]")?,
                }
                part.map_or(Ok(()), |part| write!(f, ".{part}"))
            }
        }
    }
}

/// Every array record of `data` in file order (a jagged column's offsets, then content), with its place.
pub(crate) fn records(data: &Value) -> Vec<(&Value, Place<'_>)> {
    let mut out = vec![(&data["sample"], Place::Named("data.sample")), (&data["entry"], Place::Named("data.entry"))];
    for (index, column) in data["cols"].as_array().into_iter().flatten().enumerate() {
        let name = column["name"].as_str();
        match column["kind"] == "jagged" {
            true => out.extend([(&column["offsets"], Place::Column(name, index, Some("offsets"))), (&column["content"], Place::Column(name, index, Some("content")))]),
            false => out.push((column, Place::Column(name, index, None))),
        }
    }
    out
}

/// Check the arrays lie end to end from 0, each of `size` 1 or more (docs/FORMAT.md §7), and `sample`'s and `entry`'s fields; where they end.
pub(super) fn layout(data: &Value) -> Result<u64> {
    exactly(&data["sample"], "data.sample", &["at", "size", "sha256"], &[])?;
    exactly(&data["entry"], "data.entry", &["at", "size", "sha256", "by_sample"], &[])?;
    let mut end = 0_u64;
    for (record, place) in records(data) {
        let place = place.to_string();
        checksum(record, &place)?;
        let (at, size) = (count(record, &place, "at")? as u64, count(record, &place, "size")? as u64);
        if at != end {
            return Err(invalid(format!("{place}.at is {at}, and the arrays lie end to end in file order, so it is {end} (docs/FORMAT.md §7): {MEND}")));
        }
        if size == 0 {
            return Err(invalid(format!("{place}.size is 0, and every array is a compressed frame of a few bytes or more (docs/FORMAT.md §7): {MEND}")));
        }
        end = end.checked_add(size).ok_or_else(|| invalid(format!("{place}: the arrays end past 2^64 bytes: {MEND}")))?;
    }
    Ok(end)
}

/// Check the arrays, which `layout` found to end at `end`, fill the `length` bytes after the header, unless the file is empty.
pub(super) fn fill(end: u64, length: usize) -> Result<()> {
    if length != 0 && end != length as u64 {
        return Err(invalid(format!("the arrays' records end at byte {end} after the header, and the file holds {length} bytes there: the arrays fill the rest of the file exactly (docs/FORMAT.md §1); a file cut short or with bytes added is damaged: copy it again, or press its study again")));
    }
    Ok(())
}

/// An array's `packed` bytes decompressed to `values` values of `kind`, refused unless they have the sha256 its record states.
pub(super) fn unpacked(packed: Vec<u8>, record: &Value, kind: &str, values: usize, place: &str) -> Result<Vec<u8>> {
    let w = width(kind)?;
    let expected = values.checked_mul(w).ok_or_else(|| invalid(format!("{place}: its values take more bytes than this computer holds")))?;
    let bytes = crate::codec::unpack(packed, w, expected, place)?;
    match digest(&bytes) == checksum(record, place)? {
        true => Ok(bytes),
        false => Err(invalid(format!("{place}: its array's bytes are not the sha256 its record states (docs/FORMAT.md §8): the file is damaged; copy it again, or press its study again"))),
    }
}

/// The arrays of an open file, read in file order, each checked as it is read.
pub(super) struct Arrays<R> {
    reader: R,
    length: usize,
    at: usize,
}

impl<R: Read> Arrays<R> {
    /// The `length` bytes of arrays `reader` holds from where it is.
    pub(super) fn new(reader: R, length: usize) -> Self {
        Self { reader, length, at: 0 }
    }

    /// Read one array of `values` values of `kind`, laid out as `layout` checked: its frame, its length and its `sha256`.
    pub(super) fn read(&mut self, record: &Value, kind: &str, values: usize, place: &str) -> Result<NumericBuffer> {
        let size = count(record, place, "size")?;
        if self.at.checked_add(size).is_none_or(|end| end > self.length) {
            return Err(invalid(format!("{place}: its array lies past the end of the file: the file was cut short; copy it again")));
        }
        let mut packed = vec![0; size];
        self.reader.read_exact(&mut packed)?;
        let bytes = unpacked(packed, record, kind, values, place)?;
        if kind == "b1" {
            if let Some(index) = bytes.iter().position(|byte| *byte > 1) {
                return Err(invalid(format!("{place}: value {index} of a b1 array is {}, and a b1 value is 0 or 1 (docs/FORMAT.md §4): {MEND}", bytes[index])));
            }
        }
        self.at += size;
        Ok(NumericBuffer { kind: kind.to_owned(), count: values, bytes })
    }

    /// Require every array read, and nothing after them.
    pub(super) fn finish(mut self) -> Result<()> {
        match self.at == self.length && self.reader.read(&mut [0])? == 0 {
            true => Ok(()),
            false => Err(invalid("bytes follow the last array, and nothing does (docs/FORMAT.md §1): the file is damaged; copy it again")),
        }
    }
}
