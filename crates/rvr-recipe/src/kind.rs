//! The number kinds of `.rvr` columns and how two of them combine.
//!
//! Ported from: NumPy 2.5.1 type promotion (`numpy.result_type`) for the eleven kinds an
//! `.rvr` array may have, with Python numbers treated as weak (NEP 50).
//! Documentation: <https://numpy.org/doc/stable/reference/arrays.promotion.html>

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

impl Kind {
    /// Read the `.rvr` spelling of a kind.
    pub fn named(name: &str) -> Result<Self> {
        Ok(match name {
            "b1" => B1,
            "i1" => I1,
            "i2" => I2,
            "i4" => I4,
            "i8" => I8,
            "u1" => U1,
            "u2" => U2,
            "u4" => U4,
            "u8" => U8,
            "f4" => F4,
            "f8" => F8,
            other => return Err(problem(format!("{other} is not a number kind"))),
        })
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
    pub fn unsigned(self) -> bool {
        matches!(self, B1 | U1 | U2 | U4 | U8)
    }

    /// Bring a whole number computed in 64 bits back into this kind, wrapping as NumPy does.
    pub fn wrap(self, value: i64) -> i64 {
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
    pub fn floating(self) -> Kind {
        match self {
            F4 | F8 => self,
            B1 | I1 | U1 => F4,
            I2 | U2 => F4,
            _ => F8,
        }
    }
}
