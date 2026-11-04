//! The number kinds of `.rvr` columns and how two of them combine: `numpy.result_type` (NumPy
//! 2.5.1) over the eleven kinds, the table in the README's "Kinds" section.

use crate::{problem, Result};

/// The kind of every number in a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    B1,
    I1,
    I2,
    I4,
    I8,
    U1,
    U2,
    U4,
    U8,
    F4,
    F8,
}

use Kind::*;

/// Every kind, in the order declared.
const ALL: [Kind; 11] = [B1, I1, I2, I4, I8, U1, U2, U4, U8, F4, F8];

impl Kind {
    /// Read the `.rvr` spelling of a kind.
    pub fn named(name: &str) -> Result<Self> {
        match ALL.into_iter().find(|kind| kind.name() == name) {
            Some(kind) => Ok(kind),
            None => Err(problem(format!("{name:?} is not a number kind: the kinds are {}.{}", ALL.map(Kind::name).join(", "), crate::meant(name, ALL.into_iter().map(Kind::name))))),
        }
    }

    /// The `.rvr` spelling of a kind.
    pub fn name(self) -> &'static str {
        match self {
            B1 => "b1",
            I1 => "i1",
            I2 => "i2",
            I4 => "i4",
            I8 => "i8",
            U1 => "u1",
            U2 => "u2",
            U4 => "u4",
            U8 => "u8",
            F4 => "f4",
            F8 => "f8",
        }
    }

    /// Whether the kind is a float.
    pub fn float(self) -> bool {
        matches!(self, F4 | F8)
    }

    /// The bytes one number takes.
    pub fn width(self) -> usize {
        match self {
            B1 | I1 | U1 => 1,
            I2 | U2 => 2,
            I4 | U4 | F4 => 4,
            I8 | U8 | F8 => 8,
        }
    }

    /// Whether a whole-number kind has no sign.
    pub(crate) fn unsigned(self) -> bool {
        matches!(self, B1 | U1 | U2 | U4 | U8)
    }

    /// The least and greatest value a whole-number kind holds.
    pub(crate) fn bounds(self) -> (i128, i128) {
        match self {
            B1 => (0, 1),
            I1 => (i8::MIN.into(), i8::MAX.into()),
            I2 => (i16::MIN.into(), i16::MAX.into()),
            I4 => (i32::MIN.into(), i32::MAX.into()),
            U1 => (0, u8::MAX.into()),
            U2 => (0, u16::MAX.into()),
            U4 => (0, u32::MAX.into()),
            U8 => (0, u64::MAX.into()),
            _ => (i64::MIN.into(), i64::MAX.into()),
        }
    }

    /// A whole number held in 64 bits as this kind holds it (a `u8` by its bits), at full width.
    pub(crate) fn widen(self, value: i64) -> i128 {
        match self {
            U8 => value as u64 as i128,
            _ => value as i128,
        }
    }

    /// Bring a whole number computed in 64 bits back into this kind, wrapping as NumPy does.
    pub(crate) fn wrap(self, value: i64) -> i64 {
        match self {
            B1 => (value != 0) as i64,
            I1 => value as i8 as i64,
            I2 => value as i16 as i64,
            I4 => value as i32 as i64,
            U1 => value as u8 as i64,
            U2 => value as u16 as i64,
            U4 => value as u32 as i64,
            _ => value,
        }
    }

    /// Bring a float computed in 64 bits back into this kind.
    pub fn round(self, value: f64) -> f64 {
        if self == F4 {
            value as f32 as f64
        } else {
            value
        }
    }

    /// The kind two columns combine in.
    pub fn with(self, other: Kind) -> Kind {
        if self == other {
            return self;
        }
        match (self, other) {
            (B1, kind) | (kind, B1) => kind,
            (F8, _) | (_, F8) => F8,
            (F4, kind) | (kind, F4) => {
                if kind.width() <= 2 {
                    F4
                } else {
                    F8
                }
            }
            (left, right) if left.unsigned() == right.unsigned() => {
                if left.width() >= right.width() {
                    left
                } else {
                    right
                }
            }
            (left, right) => {
                let (signed, unsigned) = if left.unsigned() {
                    (right, left)
                } else {
                    (left, right)
                };
                match (unsigned.width(), signed.width()) {
                    (8, _) => F8,
                    (plain, wide) if wide > plain => signed,
                    (1, _) => I2,
                    (2, _) => I4,
                    _ => I8,
                }
            }
        }
    }

    /// The float kind a whole-number kind takes in a function that returns floats.
    pub(crate) fn floating(self) -> Kind {
        match self {
            F4 | F8 => self,
            B1 | I1 | U1 | I2 | U2 => F4,
            _ => F8,
        }
    }
}
