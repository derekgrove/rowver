use crate::{invalid, Result};

#[derive(Clone, Debug)]
pub(crate) struct NumericArray {
    pub kind: String,
    pub count: usize,
    pub bytes: Vec<u8>,
}

/// Return the exact byte width of a supported flat dtype.
pub(crate) fn width(kind: &str) -> Result<usize> {
    match kind {
        "f8" | "i8" | "u8" => Ok(8),
        "f4" | "i4" | "u4" => Ok(4),
        "i2" | "u2" => Ok(2),
        "i1" | "u1" | "b1" => Ok(1),
        _ => Err(invalid(format!("Unsupported numeric kind {kind}"))),
    }
}

impl NumericArray {
    /// Decode every integer kind exactly for metadata validation.
    pub fn integer(&self, index: usize) -> Result<i128> {
        if index >= self.count {
            return Err(invalid("Integer index is out of bounds"));
        }
        let w = width(&self.kind)?;
        let b = &self.bytes[index * w..(index + 1) * w];
        Ok(match self.kind.as_str() {
            "i8" => i64::from_le_bytes(b.try_into().unwrap()) as i128,
            "i4" => i32::from_le_bytes(b.try_into().unwrap()) as i128,
            "i2" => i16::from_le_bytes(b.try_into().unwrap()) as i128,
            "i1" => (b[0] as i8) as i128,
            "u8" => u64::from_le_bytes(b.try_into().unwrap()) as i128,
            "u4" => u32::from_le_bytes(b.try_into().unwrap()) as i128,
            "u2" => u16::from_le_bytes(b.try_into().unwrap()) as i128,
            "u1" | "b1" => b[0] as i128,
            _ => return Err(invalid("Floating arrays have no exact integer values")),
        })
    }

    /// Read a numerical value without changing the stored dtype.
    pub fn value(&self, index: usize) -> Result<f64> {
        if index >= self.count {
            return Err(invalid(format!(
                "Value index {index} exceeds {} values",
                self.count
            )));
        }
        let w = width(&self.kind)?;
        let b = &self.bytes[index * w..(index + 1) * w];
        Ok(match self.kind.as_str() {
            "f8" => f64::from_le_bytes(b.try_into().unwrap()),
            "f4" => f32::from_le_bytes(b.try_into().unwrap()) as f64,
            "i8" => i64::from_le_bytes(b.try_into().unwrap()) as f64,
            "i4" => i32::from_le_bytes(b.try_into().unwrap()) as f64,
            "i2" => i16::from_le_bytes(b.try_into().unwrap()) as f64,
            "i1" => (b[0] as i8) as f64,
            "u8" => u64::from_le_bytes(b.try_into().unwrap()) as f64,
            "u4" => u32::from_le_bytes(b.try_into().unwrap()) as f64,
            "u2" => u16::from_le_bytes(b.try_into().unwrap()) as f64,
            "u1" | "b1" => b[0] as f64,
            _ => unreachable!(),
        })
    }

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

impl From<crate::NumericBuffer> for NumericArray {
    /// Take a caller's buffer without copying its bytes.
    fn from(buffer: crate::NumericBuffer) -> Self {
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
