use crate::write::NumericBuffer;
use crate::{invalid, Result};
use rvr_recipe::Kind;

/// Bytes per value of a kind: 8 for `f8`, 1 for `b1`; refused for a word that is no kind.
pub fn width(kind: &str) -> Result<usize> {
    Kind::named(kind).map(Kind::width).map_err(|_| invalid(format!("{kind:?} is not a kind of the format (f8, f4, i8, i4, i2, i1, u8, u4, u2, u1, b1): press its study again")))
}

/// Whether a kind holds whole numbers (every kind but `f4` and `f8`).
pub(crate) fn whole_kind(kind: &str) -> bool {
    !Kind::named(kind).is_ok_and(Kind::float)
}

/// Each value of a kind's little-endian `bytes` written to `out` as exact text, `, ` between them.
pub fn push_shown(kind: &str, bytes: &[u8], out: &mut String) -> Result<()> {
    use std::fmt::Write;
    let w = width(kind)?;
    for (at, b) in bytes.chunks_exact(w).enumerate() {
        if at > 0 {
            out.push_str(", ");
        }
        let mut v = [0; 8];
        v[..w].copy_from_slice(b);
        let four = [v[0], v[1], v[2], v[3]];
        let _ = match kind {
            "f8" => write!(out, "{}", f64::from_le_bytes(v)),
            "f4" => write!(out, "{}", f32::from_le_bytes(four)),
            "i8" => write!(out, "{}", i64::from_le_bytes(v)),
            "i4" => write!(out, "{}", i32::from_le_bytes(four)),
            "i2" => write!(out, "{}", i16::from_le_bytes([v[0], v[1]])),
            "i1" => write!(out, "{}", v[0] as i8),
            "u8" => write!(out, "{}", u64::from_le_bytes(v)),
            "u4" => write!(out, "{}", u32::from_le_bytes(four)),
            "u2" => write!(out, "{}", u16::from_le_bytes([v[0], v[1]])),
            "u1" => write!(out, "{}", v[0]),
            _ => write!(out, "{}", v[0] != 0),
        };
    }
    Ok(())
}

impl NumericBuffer {
    /// Read signed integer identity and offset arrays exactly.
    pub fn signed(&self, index: usize) -> Result<i64> {
        if index >= self.count {
            return Err(invalid(format!("a bug in rowver: value {index} of an array of {} values was asked for; please report it", self.count)));
        }
        match self.kind.as_str() {
            "i4" => Ok(
                i32::from_le_bytes(self.bytes[index * 4..index * 4 + 4].try_into().unwrap()) as i64,
            ),
            "i8" => Ok(i64::from_le_bytes(
                self.bytes[index * 8..index * 8 + 8].try_into().unwrap(),
            )),
            _ => Err(invalid(format!("a bug in rowver: an array of {} was read as positions, which are i4 or i8; please report it", self.kind))),
        }
    }
}

/// Call `each` on every value of `bytes` in order, as a float.
pub(crate) fn each_value(kind: &str, bytes: &[u8], mut each: impl FnMut(f64)) -> Result<()> {
    macro_rules! scan {
        ($t:ty) => {
            for b in bytes.chunks_exact(std::mem::size_of::<$t>()) {
                each(<$t>::from_le_bytes(b.try_into().unwrap()) as f64)
            }
        };
    }
    match kind {
        "f8" => scan!(f64),
        "f4" => scan!(f32),
        "i8" => scan!(i64),
        "i4" => scan!(i32),
        "i2" => scan!(i16),
        "i1" => scan!(i8),
        "u8" => scan!(u64),
        "u4" => scan!(u32),
        "u2" => scan!(u16),
        "u1" | "b1" => scan!(u8),
        kind => return Err(invalid(format!("{kind:?} is not a kind of the format (f8, f4, i8, i4, i2, i1, u8, u4, u2, u1, b1): give values one of those"))),
    }
    Ok(())
}

/// Call `each` on every value of `bytes` in order, exactly, as a whole number.
pub(crate) fn each_integer(kind: &str, bytes: &[u8], mut each: impl FnMut(i128)) -> Result<()> {
    macro_rules! scan {
        ($t:ty) => {
            for b in bytes.chunks_exact(std::mem::size_of::<$t>()) {
                each(<$t>::from_le_bytes(b.try_into().unwrap()) as i128)
            }
        };
    }
    match kind {
        "i8" => scan!(i64),
        "i4" => scan!(i32),
        "i2" => scan!(i16),
        "i1" => scan!(i8),
        "u8" => scan!(u64),
        "u4" => scan!(u32),
        "u2" => scan!(u16),
        "u1" | "b1" => scan!(u8),
        kind => return Err(invalid(format!("a bug in rowver: an array of {kind} was read as whole numbers; please report it"))),
    }
    Ok(())
}
