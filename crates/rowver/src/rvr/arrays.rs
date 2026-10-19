use super::cancellation::Cancellation;
use super::{checksum, count, hex};
use crate::numeric::{width, NumericArray};
use crate::{invalid, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;

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

    /// Expand one array, validate its span, and undo byte shuffling.
    pub(super) fn read(
        &mut self,
        record: &Value,
        kind: &str,
        nvalues: usize,
        label: &str,
    ) -> Result<NumericArray> {
        self.cancellation.check()?;
        let w = width(kind)?;
        let expected = nvalues
            .checked_mul(w)
            .ok_or_else(|| invalid(format!("{label}: byte count overflows")))?;
        let at = count(record, "at")?;
        let size = count(record, "size")?;
        if at != self.at || size == 0 {
            return Err(invalid(format!(
                "{label}: array spans must fill the file in order without gaps"
            )));
        }
        let end = at
            .checked_add(size)
            .filter(|end| *end <= self.length)
            .ok_or_else(|| invalid(format!("{label}: compressed span exceeds the file")))?;
        let hash = checksum(record)?;
        let mut packed = vec![0; size];
        self.reader.read_exact(&mut packed)?;
        self.cancellation.check()?;
        let bytes = crate::codec::unpack(packed, w, expected, label)?;
        self.cancellation.check()?;
        let mut actual_hash = Sha256::new();
        for chunk in bytes.chunks(65536) {
            self.cancellation.check()?;
            actual_hash.update(chunk);
        }
        if hex(&actual_hash.finalize()) != hash {
            return Err(invalid(format!("{label}: array sha256 mismatch")));
        }
        if kind == "b1" {
            for (index, byte) in bytes.iter().enumerate() {
                self.cancellation.at(index)?;
                if *byte > 1 {
                    return Err(invalid(format!("{label}: boolean bytes must be 0 or 1")));
                }
            }
        }
        let array = NumericArray {
            kind: kind.to_owned(),
            count: nvalues,
            bytes,
        };
        self.hashes.update(hash.as_bytes());
        self.at = end;
        Ok(array)
    }

    /// Require exact physical coverage and the complete ordered array digest.
    pub(super) fn finish(mut self, data: &Value) -> Result<()> {
        self.cancellation.check()?;
        if self.at != self.length || self.reader.read(&mut [0])? != 0 {
            return Err(invalid("Arrays do not fill the file: trailing data"));
        }
        if hex(&self.hashes.finalize()) != checksum(data)? {
            return Err(invalid(
                "data.sha256 does not match the array digest sequence",
            ));
        }
        Ok(())
    }
}
