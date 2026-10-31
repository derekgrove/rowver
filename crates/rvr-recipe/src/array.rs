//! Columns of numbers, one per event or a list per event, and the arithmetic between them.
//!
//! Ported from: NumPy 2.5.1 elementwise arithmetic, comparison, and logical operators, and
//! awkward 2.12.0 broadcasting of one value per event over that event's list.
//! Documentation: <https://numpy.org/doc/stable/reference/ufuncs.html>;
//! <https://awkward-array.org/doc/main/user-guide/how-to-math-broadcasting.html>
//! Method: floats are held in 64 bits and rounded to their kind after every operation, which
//! gives exactly the 32-bit result for `+ - * /` and square root. Whole numbers are held in
//! 64 bits and wrapped to their kind.

use crate::kind::Kind;
use crate::{problem, Result};
use std::sync::Arc;

/// The numbers of one column, all of one kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    pub kind: Kind,
    pub store: Store,
}

/// How numbers are held while they are worked on. The numbers are shared, so naming a
/// column again, or passing it into a function, copies nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Store {
    /// Whole numbers and truth values; a `u8` is held by its bits.
    Whole(Arc<Vec<i64>>),
    Float(Arc<Vec<f64>>),
}

/// A value written in an expression or taken from a sample's metadata.
#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Whole(i64),
    Float(f64),
    Truth(bool),
    Text(String),
    /// A sample field the sample does not have, by its key.
    Missing(String),
}

/// What a sample field the sample does not have is refused with, where it is used as `what`.
pub(crate) fn lacking(key: &str, what: &str) -> crate::Error {
    let why = match crate::ADDED.contains(&key) {
        true => format!("{key} is a count rvr press records of each sample, which no expression can read"),
        false => format!("this sample has no {key}: give it one in samples.toml, or give every sample one under [defaults]"),
    };
    problem(format!("sample.{key} is used as {what}, but {why}"))
}

/// What an expression works on.
#[derive(Clone, Debug, PartialEq)]
pub enum Array {
    Constant(Constant),
    /// One number per event.
    Event(Values),
    /// A list of numbers per event; list `e` is `values[offsets[e]..offsets[e + 1]]`.
    Object {
        offsets: Arc<Vec<usize>>,
        values: Values,
    },
}

/// The arithmetic and comparison operators of an expression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Power,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

impl Values {
    /// Hold whole numbers of a kind.
    pub fn whole(kind: Kind, values: Vec<i64>) -> Self {
        Self {
            kind,
            store: Store::Whole(Arc::new(values)),
        }
    }

    /// Hold floats of a kind.
    pub fn float(kind: Kind, values: Vec<f64>) -> Self {
        Self {
            kind,
            store: Store::Float(Arc::new(values)),
        }
    }

    /// Read a column from the little-endian bytes of its kind.
    pub fn from_bytes(kind: Kind, bytes: &[u8]) -> Self {
        /// Read every value of one fixed-width type and widen it.
        macro_rules! read {
            ($type:ty, $wide:ty) => {{
                let (parts, _) = bytes.as_chunks::<{ size_of::<$type>() }>();
                let widen =
                    |part: &[u8; size_of::<$type>()]| <$type>::from_le_bytes(*part) as $wide;
                parts.iter().map(widen).collect()
            }};
        }
        match kind {
            Kind::F4 => Self::float(kind, read!(f32, f64)),
            Kind::F8 => Self::float(kind, read!(f64, f64)),
            Kind::B1 => Self::whole(kind, bytes.iter().map(|byte| (*byte != 0) as i64).collect()),
            Kind::I1 => Self::whole(kind, read!(i8, i64)),
            Kind::I2 => Self::whole(kind, read!(i16, i64)),
            Kind::I4 => Self::whole(kind, read!(i32, i64)),
            Kind::I8 => Self::whole(kind, read!(i64, i64)),
            Kind::U1 => Self::whole(kind, read!(u8, i64)),
            Kind::U2 => Self::whole(kind, read!(u16, i64)),
            Kind::U4 => Self::whole(kind, read!(u32, i64)),
            Kind::U8 => Self::whole(kind, read!(u64, i64)),
        }
    }

    /// Write a column as the little-endian bytes of its kind.
    pub fn to_bytes(&self) -> Vec<u8> {
        let w = self.kind.width();
        let mut bytes = Vec::with_capacity(self.len() * w);
        match &self.store {
            Store::Float(values) if w == 4 => {
                for value in values.iter() {
                    bytes.extend_from_slice(&(*value as f32).to_le_bytes());
                }
            }
            Store::Float(values) => {
                for value in values.iter() {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            Store::Whole(values) => {
                for value in values.iter() {
                    bytes.extend_from_slice(&value.to_le_bytes()[..w]);
                }
            }
        }
        bytes
    }

    /// The number of values.
    pub fn len(&self) -> usize {
        match &self.store {
            Store::Whole(values) => values.len(),
            Store::Float(values) => values.len(),
        }
    }

    /// Whether there are no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One value as a float, reading a `u8` by its magnitude.
    pub fn number(&self, index: usize) -> f64 {
        match &self.store {
            Store::Float(values) => values[index],
            Store::Whole(values) if self.kind == Kind::U8 => values[index] as u64 as f64,
            Store::Whole(values) => values[index] as f64,
        }
    }

    /// The values as floats.
    pub fn numbers(&self) -> Vec<f64> {
        (0..self.len()).map(|index| self.number(index)).collect()
    }

    /// The truth values of a `b1` column, or a refusal naming what was asked of it.
    pub fn truths(&self, what: &str) -> Result<&[i64]> {
        match (&self.store, self.kind) {
            (Store::Whole(values), Kind::B1) => Ok(values),
            _ => Err(problem(format!(
                "{what} needs a true-or-false value, not {}",
                self.kind.name()
            ))),
        }
    }

    /// Convert every value to another kind, as NumPy's `astype` does.
    pub fn cast(&self, kind: Kind) -> Values {
        if kind == self.kind {
            return self.clone();
        }
        match (&self.store, kind.float()) {
            (_, true) => Values::float(
                kind,
                self.numbers()
                    .into_iter()
                    .map(|value| kind.round(value))
                    .collect(),
            ),
            (Store::Whole(values), false) => {
                Values::whole(kind, values.iter().map(|value| kind.wrap(*value)).collect())
            }
            (Store::Float(values), false) => Values::whole(
                kind,
                values
                    .iter()
                    .map(|value| {
                        if kind == Kind::B1 {
                            (*value != 0.0) as i64
                        } else if kind == Kind::U8 && (0.0..u64::MAX as f64).contains(value) {
                            // u64::MAX rounds to 2^64 as f64, the exclusive upper bound.
                            // Convert valid magnitudes before storing their bits in i64.
                            *value as u64 as i64
                        } else {
                            kind.wrap(*value as i64)
                        }
                    })
                    .collect(),
            ),
        }
    }

    /// Keep the values at the given positions.
    pub fn take(&self, positions: &[usize]) -> Values {
        let store = match &self.store {
            Store::Whole(values) => {
                Store::Whole(Arc::new(positions.iter().map(|at| values[*at]).collect()))
            }
            Store::Float(values) => {
                Store::Float(Arc::new(positions.iter().map(|at| values[*at]).collect()))
            }
        };
        Values {
            kind: self.kind,
            store,
        }
    }
}

impl Constant {
    /// The kind a constant takes beside a column of `kind`, following NumPy's weak scalars.
    pub(crate) fn beside(&self, kind: Kind) -> Result<Kind> {
        Ok(match (self, kind) {
            (Constant::Truth(_), kind) => kind,
            (Constant::Whole(_), Kind::B1) => Kind::I8,
            (Constant::Whole(value), kind) if !kind.float() => {
                if kind.wrap(*value) != *value || (kind == Kind::U8 && *value < 0) {
                    return Err(problem(format!(
                        "{value} does not fit beside a {} column",
                        kind.name()
                    )));
                }
                kind
            }
            (Constant::Whole(_), kind) => kind,
            (Constant::Float(_), kind) if kind.float() => kind,
            (Constant::Float(_), _) => Kind::F8,
            (Constant::Text(text), _) => {
                return Err(problem(format!("\"{text}\" is text, not a number")))
            }
            (Constant::Missing(key), _) => return Err(lacking(key, "a number")),
        })
    }

    /// Repeat a constant as a column of a kind.
    pub(crate) fn repeat(&self, kind: Kind, count: usize) -> Values {
        match self {
            Constant::Float(value) => Values::float(Kind::F8, vec![*value; count]).cast(kind),
            Constant::Whole(value) => Values::whole(Kind::I8, vec![*value; count]).cast(kind),
            Constant::Truth(value) => {
                Values::whole(Kind::B1, vec![*value as i64; count]).cast(kind)
            }
            _ => unreachable!("beside refuses text"),
        }
    }

    /// The column a lone constant becomes: a whole number is `i8`, a float `f8`, a truth `b1`.
    pub fn alone(&self, count: usize) -> Result<Values> {
        Ok(match self {
            Constant::Float(_) => self.repeat(Kind::F8, count),
            Constant::Whole(_) => self.repeat(Kind::I8, count),
            Constant::Truth(_) => self.repeat(Kind::B1, count),
            Constant::Text(text) => {
                return Err(problem(format!("\"{text}\" is text, not a number")))
            }
            Constant::Missing(key) => return Err(lacking(key, "a number")),
        })
    }
}

/// Bring two columns of equal length to the kind they combine in.
pub(crate) fn together(left: &Values, right: &Values) -> (Values, Values) {
    let kind = left.kind.with(right.kind);
    (left.cast(kind), right.cast(kind))
}

/// Python's remainder, which takes the sign of the divisor.
fn remainder(left: f64, right: f64) -> f64 {
    let rest = left % right;
    if rest != 0.0 && (rest < 0.0) != (right < 0.0) {
        rest + right
    } else {
        rest
    }
}

/// Apply one operator to two columns of the same length.
fn apply(operator: Operator, left: &Values, right: &Values) -> Result<Values> {
    use Operator::*;
    if matches!(operator, And | Or) {
        let (a, b) = (left.truths("and/or")?, right.truths("and/or")?);
        let join = |(a, b): (&i64, &i64)| if operator == And { a & b } else { a | b };
        return Ok(Values::whole(
            Kind::B1,
            a.iter().zip(b.iter()).map(join).collect(),
        ));
    }
    let (mut left, mut right) = together(left, right);
    let arithmetic = matches!(
        operator,
        Add | Subtract | Multiply | Divide | Remainder | Power
    );
    if arithmetic && left.kind == Kind::B1 {
        return Err(problem(
            "Arithmetic on two true-or-false values; use and, or, or count",
        ));
    }
    if operator == Divide && !left.kind.float() {
        (left, right) = (left.cast(Kind::F8), right.cast(Kind::F8));
    }
    let kind = left.kind;
    match (&left.store, &right.store) {
        (Store::Float(a), Store::Float(b)) => {
            let pairs = a.iter().zip(b.iter());
            if arithmetic {
                let compute = |(a, b): (&f64, &f64)| match operator {
                    Add => a + b,
                    Subtract => a - b,
                    Multiply => a * b,
                    Divide => a / b,
                    Remainder => remainder(*a, *b),
                    _ if *b == 2.0 => a * a,
                    _ => libm::pow(*a, *b),
                };
                return Ok(Values::float(
                    kind,
                    pairs.map(compute).map(|value| kind.round(value)).collect(),
                ));
            }
            let compare = |(a, b): (&f64, &f64)| match operator {
                Less => a < b,
                LessEqual => a <= b,
                Greater => a > b,
                GreaterEqual => a >= b,
                Equal => a == b,
                _ => a != b,
            };
            Ok(Values::whole(
                Kind::B1,
                pairs.map(compare).map(i64::from).collect(),
            ))
        }
        (Store::Whole(a), Store::Whole(b)) => {
            let wide = |value: i64| {
                if kind == Kind::U8 {
                    value as u64 as i128
                } else {
                    value as i128
                }
            };
            let pairs = a.iter().zip(b.iter());
            if arithmetic {
                let compute = |(a, b): (&i64, &i64)| -> Result<i64> {
                    Ok(match operator {
                        Add => a.wrapping_add(*b),
                        Subtract => a.wrapping_sub(*b),
                        Multiply => a.wrapping_mul(*b),
                        Remainder => whole_remainder(kind, *a, *b),
                        _ => {
                            let power = u32::try_from(*b).map_err(|_| {
                                problem("A whole number cannot be raised to a negative power")
                            })?;
                            a.wrapping_pow(power)
                        }
                    })
                };
                let values = pairs
                    .map(compute)
                    .map(|value| value.map(|value| kind.wrap(value)));
                return Ok(Values::whole(kind, values.collect::<Result<_>>()?));
            }
            let compare = |(a, b): (&i64, &i64)| match operator {
                Less => wide(*a) < wide(*b),
                LessEqual => wide(*a) <= wide(*b),
                Greater => wide(*a) > wide(*b),
                GreaterEqual => wide(*a) >= wide(*b),
                Equal => a == b,
                _ => a != b,
            };
            Ok(Values::whole(
                Kind::B1,
                pairs.map(compare).map(i64::from).collect(),
            ))
        }
        _ => unreachable!("together gives one kind"),
    }
}

impl Array {
    /// The number of events an array covers, when it is not a constant.
    pub fn events(&self) -> Option<usize> {
        match self {
            Array::Constant(_) => None,
            Array::Event(values) => Some(values.len()),
            Array::Object { offsets, .. } => Some(offsets.len() - 1),
        }
    }

    /// Build a list-per-event array, checking that the lists hold exactly the values.
    pub fn object(counts: &[usize], values: Values) -> Result<Array> {
        let mut offsets = vec![0];
        for count in counts {
            offsets.push(offsets[offsets.len() - 1] + count);
        }
        if offsets[counts.len()] != values.len() {
            return Err(problem(
                "A list column's counts do not add up to its values",
            ));
        }
        Ok(Array::Object {
            offsets: Arc::new(offsets),
            values,
        })
    }

    /// Give each event's value to every member of that event's list.
    pub(crate) fn spread(values: &Values, offsets: &[usize]) -> Values {
        let mut positions = Vec::with_capacity(offsets[offsets.len() - 1]);
        for event in 0..offsets.len() - 1 {
            positions.extend(std::iter::repeat_n(
                event,
                offsets[event + 1] - offsets[event],
            ));
        }
        values.take(&positions)
    }

    /// Apply an operator between two arrays, spreading constants and per-event values as needed.
    pub fn combine(&self, operator: Operator, other: &Array) -> Result<Array> {
        self.zip(
            other,
            &|constant, beside| beside_column(operator, constant, beside),
            &|left, right| constants(operator, left, right),
            &|left, right| apply(operator, left, right),
        )
    }

    /// Pair two arrays value by value: `adopt` turns a constant into a column beside the other
    /// side, `both` handles two constants, and `apply` works on two columns of one length.
    pub fn zip(
        &self,
        other: &Array,
        adopt: &dyn Fn(&crate::Constant, &Values) -> Result<Values>,
        both: &dyn Fn(&crate::Constant, &crate::Constant) -> Result<crate::Constant>,
        apply: &dyn Fn(&Values, &Values) -> Result<Values>,
    ) -> Result<Array> {
        use Array::*;
        let listed = |offsets: &Arc<Vec<usize>>, values: Values| Object {
            offsets: offsets.clone(),
            values,
        };
        match (self, other) {
            (Constant(a), Constant(b)) => both(a, b).map(Constant),
            (Constant(a), Event(b)) => Ok(Event(apply(&adopt(a, b)?, b)?)),
            (Event(a), Constant(b)) => Ok(Event(apply(a, &adopt(b, a)?)?)),
            (Constant(a), Object { offsets, values }) => {
                Ok(listed(offsets, apply(&adopt(a, values)?, values)?))
            }
            (Object { offsets, values }, Constant(b)) => {
                Ok(listed(offsets, apply(values, &adopt(b, values)?)?))
            }
            (Event(a), Event(b)) => {
                same(a.len(), b.len())?;
                Ok(Event(apply(a, b)?))
            }
            (Event(a), Object { offsets, values }) => {
                same(a.len(), offsets.len() - 1)?;
                Ok(listed(offsets, apply(&Self::spread(a, offsets), values)?))
            }
            (Object { offsets, values }, Event(b)) => {
                same(offsets.len() - 1, b.len())?;
                Ok(listed(offsets, apply(values, &Self::spread(b, offsets))?))
            }
            (
                Object { offsets, values },
                Object {
                    offsets: others,
                    values: b,
                },
            ) => {
                if offsets != others {
                    return Err(problem(
                        "Two list columns with different numbers of members per event cannot be combined",
                    ));
                }
                Ok(listed(offsets, apply(values, b)?))
            }
        }
    }
}

/// Turn a constant into a column beside another column, in the kind NumPy would give it there.
fn beside_column(operator: Operator, constant: &Constant, beside: &Values) -> Result<Values> {
    if matches!(operator, Operator::And | Operator::Or) {
        return match constant {
            Constant::Truth(_) => Ok(constant.repeat(Kind::B1, beside.len())),
            Constant::Missing(key) => Err(lacking(key, "a true-or-false value")),
            _ => Err(problem("and/or needs true-or-false values")),
        };
    }
    Ok(constant.repeat(constant.beside(beside.kind)?, beside.len()))
}

/// Python's remainder for whole numbers, which takes the sign of the divisor; zero for a zero divisor.
fn whole_remainder(kind: Kind, left: i64, right: i64) -> i64 {
    if right == 0 {
        return 0;
    }
    if kind == Kind::U8 {
        return (left as u64 % right as u64) as i64;
    }
    let rest = left.wrapping_rem(right);
    if rest != 0 && (rest < 0) != (right < 0) {
        rest + right
    } else {
        rest
    }
}

/// Require two columns to cover the same events.
fn same(left: usize, right: usize) -> Result<()> {
    if left == right {
        Ok(())
    } else {
        Err(problem(format!(
            "Columns of {left} and {right} events cannot be combined"
        )))
    }
}

/// Apply an operator to two constants.
fn constants(operator: Operator, left: &Constant, right: &Constant) -> Result<Constant> {
    use Operator::*;
    if let (Constant::Missing(key), _) | (_, Constant::Missing(key)) = (left, right) {
        return match operator {
            Less | LessEqual | Greater | GreaterEqual | Equal | NotEqual => {
                Ok(Constant::Truth(false))
            }
            And | Or => Err(lacking(key, "a true-or-false value")),
            _ => Err(lacking(key, "a number")),
        };
    }
    if let (Constant::Text(a), Constant::Text(b)) = (left, right) {
        return match operator {
            Equal => Ok(Constant::Truth(a == b)),
            NotEqual => Ok(Constant::Truth(a != b)),
            _ => Err(problem("Text can only be compared with == and !=")),
        };
    }
    if matches!(left, Constant::Text(_)) || matches!(right, Constant::Text(_)) {
        return match operator {
            Equal => Ok(Constant::Truth(false)),
            NotEqual => Ok(Constant::Truth(true)),
            _ => Err(problem("Text cannot be combined with a number")),
        };
    }
    let (a, b) = (left.alone(1)?, right.alone(1)?);
    let result = apply(operator, &a, &b)?;
    Ok(match (&result.store, result.kind) {
        (Store::Whole(values), Kind::B1) => Constant::Truth(values[0] != 0),
        (Store::Whole(values), _) => Constant::Whole(values[0]),
        (Store::Float(values), _) => Constant::Float(values[0]),
    })
}
