use crate::{invalid, Result};

#[derive(Clone, Debug)]
pub(crate) struct NumericArray {
    pub kind: String,
    pub count: usize,
    pub bytes: Vec<u8>,
}

/// Bytes per value of a kind: 8 for `f8`, 1 for `b1`; refused for a word that is no kind.
pub fn width(kind: &str) -> Result<usize> {
    match kind {
        "f8" | "i8" | "u8" => Ok(8),
        "f4" | "i4" | "u4" => Ok(4),
        "i2" | "u2" => Ok(2),
        "i1" | "u1" | "b1" => Ok(1),
        _ => Err(invalid(format!("{kind:?} is not a kind of the format (f8, f4, i8, i4, i2, i1, u8, u4, u2, u1, b1): press its study again"))),
    }
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

impl NumericArray {
    /// Read signed integer identity and offset arrays exactly.
    pub fn signed(&self, index: usize) -> Result<i64> {
        if index >= self.count {
            return Err(invalid("Identity index is out of bounds"));
        }
        match self.kind.as_str() {
            "i4" => Ok(
                i32::from_le_bytes(self.bytes[index * 4..index * 4 + 4].try_into().unwrap()) as i64,
            ),
            "i8" => Ok(i64::from_le_bytes(
                self.bytes[index * 8..index * 8 + 8].try_into().unwrap(),
            )),
            _ => Err(invalid("Identity arrays must be signed integers")),
        }
    }
}

impl From<crate::write::NumericBuffer> for NumericArray {
    /// Take a caller's buffer without copying its bytes.
    fn from(buffer: crate::write::NumericBuffer) -> Self {
        Self {
            kind: buffer.kind,
            count: buffer.count,
            bytes: buffer.bytes,
        }
    }
}

/// Call `each` on every value of `bytes` in order, as `NumericArray::value` reads it.
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
        kind => return Err(invalid(format!("Unsupported numeric kind {kind}"))),
    }
    Ok(())
}

/// Call `each` on every value of `bytes` in order, exactly, as `NumericArray::integer` reads it.
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
        _ => return Err(invalid("Floating arrays have no exact integer values")),
    }
    Ok(())
}
