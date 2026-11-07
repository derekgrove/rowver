use crate::{invalid, Result};
use rvr_recipe::Kind;

/// Bytes per value of a kind: 8 for `f8`, 1 for `b1`; refused for a word that is no kind.
pub fn width(kind: &str) -> Result<usize> {
    Kind::named(kind).map(Kind::width).map_err(|_| invalid(format!("{kind:?} is not a kind of the format (f8, f4, i8, i4, i2, i1, u8, u4, u2, u1, b1): press its study again")))
}

/// The first byte of `b1` values that is neither 0 nor 1; none for another kind.
pub(crate) fn untrue(kind: &str, values: &[u8]) -> Option<u8> {
    (kind == "b1").then(|| values.iter().find(|byte| **byte > 1).copied()).flatten()
}

/// Whether a kind holds whole numbers (every kind but `f4` and `f8`).
pub(crate) fn whole_kind(kind: &str) -> bool {
    !Kind::named(kind).is_ok_and(Kind::float)
}

/// The width of a float kind and the bits of its one quiet NaN (docs/FORMAT.md §4); `None` for a whole kind.
pub(crate) fn quiet_nan(kind: &str) -> Option<(usize, u64)> {
    match kind {
        "f8" => Some((8, rvr_recipe::QUIET_NAN_F8)),
        "f4" => Some((4, u64::from(rvr_recipe::QUIET_NAN_F4))),
        _ => None,
    }
}

/// Whether the bits of a value of a kind are a NaN other than the one quiet NaN.
pub(crate) fn other_nan(kind: &str, bits: u64) -> bool {
    let nan = match kind {
        "f8" => f64::from_bits(bits).is_nan(),
        _ => f32::from_bits(bits as u32).is_nan(),
    };
    nan && quiet_nan(kind).is_some_and(|(_, one)| bits != one)
}

/// Each value of a kind's little-endian `bytes` written to `out` as exact text, `, ` between them, another NaN than the quiet one with its bits.
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
            "f8" | "f4" if other_nan(kind, u64::from_le_bytes(v)) => write!(out, "NaN (bits {:#0w$x})", u64::from_le_bytes(v), w = 2 + 2 * w),
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
