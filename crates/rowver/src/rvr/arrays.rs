use super::cancellation::Cancellation;
use super::{checksum, count, exactly, hex, MEND};
use crate::numeric::{width, NumericArray};
use crate::{invalid, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;

/// Every array record of `data`, in file order: `sample`, `entry`, then each column's (a jagged
/// one's offsets, then content), each with where it is and whether it has `by_sample`.
pub(super) fn records(data: &Value) -> Vec<(&Value, String, bool)> {
    let mut out = vec![(&data["sample"], "data.sample".to_owned(), false), (&data["entry"], "data.entry".to_owned(), true)];
    for (index, column) in data["cols"].as_array().into_iter().flatten().enumerate() {
        let place = match column["name"].as_str() {
            Some(name) => format!("data.cols[{name:?}]"),
            None => format!("data.cols[{index}]"),
        };
        match column["kind"] == "jagged" {
            true => out.extend([(&column["offsets"], format!("{place}.offsets"), true), (&column["content"], format!("{place}.content"), true)]),
            false => out.push((column, place, true)),
        }
    }
    out
}

/// Check the arrays' records lie end to end from `at` 0, each of `size` 1 or more (docs/FORMAT.md
/// §7), and `sample`'s and `entry`'s fields; how many bytes the arrays take.
pub(super) fn layout(data: &Value) -> Result<u64> {
    exactly(&data["sample"], "data.sample", &["at", "size", "sha256"], &[])?;
    exactly(&data["entry"], "data.entry", &["at", "size", "sha256", "by_sample"], &[])?;
    let mut end = 0_u64;
    for (record, place, _) in records(data) {
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

/// Check the arrays, which `layout` found to end at `end`, fill the `length` bytes after the
/// header exactly, unless the file is empty.
pub(super) fn fill(end: u64, length: usize) -> Result<()> {
    if length != 0 && end != length as u64 {
        return Err(invalid(format!("the arrays' records end at byte {end} after the header, and the file holds {length} bytes there: the arrays fill the rest of the file exactly (docs/FORMAT.md §1); a file cut short or with bytes added is damaged: copy it again, or press its study again")));
    }
    Ok(())
}

pub(super) struct Arrays<'a, R> {
    reader: R,
    length: usize,
    at: usize,
    hashes: Sha256,
    pub(super) cancellation: Cancellation<'a>,
}

impl<'a, R: Read> Arrays<'a, R> {
    /// Decode either a byte slice or an open file through the same array checks.
    pub(super) fn new(reader: R, length: usize, cancellation: Cancellation<'a>) -> Self {
        Self {
            reader,
            length,
            at: 0,
            hashes: Sha256::new(),
            cancellation,
        }
    }

    /// Read one array of `values` values of `kind`, laid out as `layout` checked: its frame, its
    /// length and its `sha256`.
    pub(super) fn read(&mut self, record: &Value, kind: &str, values: usize, place: &str) -> Result<NumericArray> {
        self.cancellation.check()?;
        let w = width(kind)?;
        let expected = values
            .checked_mul(w)
            .ok_or_else(|| invalid(format!("{place}: its values take more bytes than this computer holds")))?;
        let size = count(record, place, "size")?;
        if self.at.checked_add(size).is_none_or(|end| end > self.length) {
            return Err(invalid(format!("{place}: its array lies past the end of the file: the file was cut short; copy it again")));
        }
        let hash = checksum(record, place)?;
        let mut packed = vec![0; size];
        self.reader.read_exact(&mut packed)?;
        self.cancellation.check()?;
        let bytes = crate::codec::unpack(packed, w, expected, place)?;
        self.cancellation.check()?;
        let mut actual = Sha256::new();
        for chunk in bytes.chunks(65536) {
            self.cancellation.check()?;
            actual.update(chunk);
        }
        if hex(&actual.finalize()) != hash {
            return Err(invalid(format!("{place}: its array's bytes are not the sha256 its record states (docs/FORMAT.md §8): the file is damaged; copy it again, or press its study again")));
        }
        if kind == "b1" {
            if let Some(index) = bytes.iter().position(|byte| *byte > 1) {
                return Err(invalid(format!("{place}: value {index} of a b1 array is {}, and a b1 value is 0 or 1 (docs/FORMAT.md §4): {MEND}", bytes[index])));
            }
        }
        self.hashes.update(hash.as_bytes());
        self.at += size;
        Ok(NumericArray { kind: kind.to_owned(), count: values, bytes })
    }

    /// Require every array read, and nothing after them.
    pub(super) fn finish(mut self, data: &Value) -> Result<()> {
        self.cancellation.check()?;
        if self.at != self.length || self.reader.read(&mut [0])? != 0 {
            return Err(invalid("bytes follow the last array, and nothing does (docs/FORMAT.md §1): the file is damaged; copy it again"));
        }
        if hex(&self.hashes.finalize()) != checksum(data, "data")? {
            return Err(invalid(format!("data.sha256 is not the SHA-256 of the arrays' sha256s joined in file order (docs/FORMAT.md §8): {MEND}")));
        }
        Ok(())
    }
}
