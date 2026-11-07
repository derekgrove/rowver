//! Columns of numbers, one per event or a list per event, and the arithmetic between them.
//!
//! Floats are held in 64 bits and rounded to their kind after every operation, which gives exactly
//! the 32-bit result for `+ - * /`. Whole numbers are held in 64 bits (a `u8` by its bits) and
//! wrapped to their kind. The README's "Kinds" and "Numbers, exactly" sections state every rule.

use crate::kind::Kind;
use crate::{data, problem, Error, Result};
use std::sync::Arc;

/// The numbers of one column, all of one kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    pub kind: Kind,
    pub store: Store,
}

/// How numbers are held while they are worked on, shared so that using a column again copies nothing.
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
        true => recorded(key),
        false => format!("this sample has no {key}: give it one in samples.toml, or give every sample one under [defaults]"),
    };
    problem(format!("sample.{key} is used as {what}, but {why}"))
}

/// The words for `key`, a count a press records of each sample, which no expression reads.
pub(crate) fn recorded(key: &str) -> String {
    format!("{key} is a count rvr press records of each sample, which no expression can read")
}

/// What `operator` refuses text with, when it is not `==` or `!=`.
pub(crate) fn text_refused(operator: Operator) -> String {
    match operator {
        _ if operator.arithmetic() => format!("Text cannot be combined with a number: {} takes numbers", operator.word()),
        Operator::And | Operator::Or => format!("{} needs a true-or-false value, not text", operator.word()),
        _ => format!("Text can only be compared with == and !=, not {}", operator.word()),
    }
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

/// The arithmetic, comparison and truth operators of an expression.
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

impl Operator {
    /// Whether it computes a number: `+ - * / % **`.
    pub fn arithmetic(self) -> bool {
        use Operator::*;
        matches!(self, Add | Subtract | Multiply | Divide | Remainder | Power)
    }

    /// The word or symbol an expression writes it with.
    pub fn word(self) -> &'static str {
        use Operator::*;
        match self {
            Add => "+",
            Subtract => "-",
            Multiply => "*",
            Divide => "/",
            Remainder => "%",
            Power => "**",
            Less => "<",
            LessEqual => "<=",
            Greater => ">",
            GreaterEqual => ">=",
            Equal => "==",
            NotEqual => "!=",
            And => "and",
            Or => "or",
        }
    }
}

/// The bits of the one NaN a worked-out `f8` value is written as: quiet and positive, its payload zero.
pub const QUIET_NAN_F8: u64 = 0x7ff8_0000_0000_0000;
/// The bits of the same NaN in an `f4`.
pub const QUIET_NAN_F4: u32 = 0x7fc0_0000;
const QUIET_NAN: f64 = f64::from_bits(QUIET_NAN_F8);

/// An `f4` value as a double, a NaN's sign and payload kept bit for bit, as no machine's conversion need keep them.
fn widened(value: f32) -> f64 {
    let bits = value.to_bits();
    match value.is_nan() {
        true => f64::from_bits(u64::from(bits >> 31) << 63 | 0x7ff << 52 | u64::from(bits & 0x007f_ffff) << 29),
        false => f64::from(value),
    }
}

/// A double held for an `f4` as its `f4`: a NaN's sign and payload as `widened` made them.
fn narrowed(value: f64) -> f32 {
    let bits = value.to_bits();
    let payload = (bits >> 29) as u32 & 0x007f_ffff;
    match value.is_nan() {
        // A payload below what an f4 holds keeps it a NaN as the quiet one.
        true => f32::from_bits(((bits >> 63) as u32) << 31 | 0xff << 23 | if payload == 0 { 0x0040_0000 } else { payload }),
        false => value as f32,
    }
}

impl Values {
    /// The same values, each NaN among them made `QUIET_NAN`.
    fn settled(mut self) -> Values {
        if let Store::Float(values) = &mut self.store {
            if let Some(first) = values.iter().position(|v| v.is_nan()) {
                Arc::make_mut(values)[first..].iter_mut().filter(|v| v.is_nan()).for_each(|v| *v = QUIET_NAN);
            }
        }
        self
    }

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
            Kind::F4 => Self::float(kind, bytes.as_chunks::<4>().0.iter().map(|part| widened(f32::from_le_bytes(*part))).collect()),
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
                    bytes.extend_from_slice(&narrowed(*value).to_le_bytes());
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
    pub(crate) fn truths(&self, what: &str) -> Result<&[i64]> {
        match (&self.store, self.kind) {
            (Store::Whole(values), Kind::B1) => Ok(values),
            _ => Err(not_truth(what, self.kind.name())),
        }
    }

    /// The values as floats, when they are held as floats.
    pub(crate) fn floats(&self) -> Option<&[f64]> {
        match &self.store {
            Store::Float(held) => Some(held),
            Store::Whole(_) => None,
        }
    }

    /// The values as whole numbers, when they are held as whole numbers.
    pub(crate) fn wholes(&self) -> Option<&[i64]> {
        match &self.store {
            Store::Whole(held) => Some(held),
            Store::Float(_) => None,
        }
    }

    /// Every value converted to `kind` by the README's conversion rule, refusing one it cannot take.
    pub fn convert(&self, kind: Kind) -> Result<Values> {
        if kind == self.kind {
            return Ok(self.clone());
        }
        let (low, high) = kind.bounds();
        let named = kind.name();
        match &self.store {
            Store::Whole(values) => {
                let from = self.kind;
                if kind.float() {
                    let round = |value: &i64| match (from, kind) {
                        (Kind::U8, Kind::F4) => *value as u64 as f32 as f64,
                        (Kind::U8, _) => *value as u64 as f64,
                        (_, Kind::F4) => *value as f32 as f64,
                        _ => *value as f64,
                    };
                    return Ok(Values::float(kind, values.iter().map(round).collect()));
                }
                let mut out = Vec::with_capacity(values.len());
                for (at, value) in values.iter().enumerate() {
                    let wide = from.widen(*value);
                    out.push(match kind {
                        Kind::B1 => (wide != 0) as i64,
                        _ if wide < low || wide > high => {
                            return Err(data(at, format!("{wide} does not fit in {named}, which holds {low} to {high}")))
                        }
                        _ => wide as i64,
                    });
                }
                Ok(Values::whole(kind, out))
            }
            Store::Float(values) if kind.float() => Ok(Values::float(kind, values.iter().map(|v| kind.round(*v)).collect())),
            Store::Float(values) => {
                let mut out = Vec::with_capacity(values.len());
                for (at, value) in values.iter().enumerate() {
                    if value.is_nan() {
                        let said = match kind {
                            Kind::B1 => "NaN is neither true nor false, so it has no b1 value".to_owned(),
                            _ => format!("NaN is no number, so it has no {named} value"),
                        };
                        return Err(data(at, said));
                    }
                    let whole = value.trunc();
                    out.push(match kind {
                        Kind::B1 => (*value != 0.0) as i64,
                        // Every bound is a power of two, or one less, so that above is the next one up.
                        _ if whole < low as f64 || whole >= high as f64 + 1.0 => {
                            return Err(data(at, format!("{value:?} does not fit in {named}, which holds {low} to {high}")))
                        }
                        Kind::U8 => whole as u64 as i64,
                        _ => whole as i64,
                    });
                }
                Ok(Values::whole(kind, out))
            }
        }
    }

    /// Its first value, as a constant.
    pub(crate) fn constant(&self) -> Constant {
        match (&self.store, self.kind) {
            (Store::Float(values), _) => Constant::Float(values[0]),
            (Store::Whole(values), Kind::B1) => Constant::Truth(values[0] != 0),
            (Store::Whole(values), _) => Constant::Whole(values[0]),
        }
    }

    /// The values at positions `from..to`.
    pub fn slice(&self, from: usize, to: usize) -> Values {
        let store = match &self.store {
            Store::Whole(held) => Store::Whole(Arc::new(held[from..to].to_vec())),
            Store::Float(held) => Store::Float(Arc::new(held[from..to].to_vec())),
        };
        Values { kind: self.kind, store }
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
    pub(crate) fn repeat(&self, kind: Kind, count: usize) -> Result<Values> {
        match self {
            Constant::Float(value) => Values::float(Kind::F8, vec![*value; count]).convert(kind),
            Constant::Whole(value) => Values::whole(Kind::I8, vec![*value; count]).convert(kind),
            Constant::Truth(value) => {
                Values::whole(Kind::B1, vec![*value as i64; count]).convert(kind)
            }
            _ => unreachable!("beside refuses text"),
        }
    }

    /// The column a lone constant becomes: a whole number is `i8`, a float `f8`, a truth `b1`.
    pub fn alone(&self, count: usize) -> Result<Values> {
        Ok(match self {
            Constant::Float(_) => self.repeat(Kind::F8, count)?,
            Constant::Whole(_) => self.repeat(Kind::I8, count)?,
            Constant::Truth(_) => self.repeat(Kind::B1, count)?,
            Constant::Text(text) => {
                return Err(problem(format!("\"{text}\" is text, not a number")))
            }
            Constant::Missing(key) => return Err(lacking(key, "a number")),
        })
    }
}

/// A constant as a column beside `beside`, in the kind it takes there.
pub(crate) fn adopt(constant: &Constant, beside: &Values) -> Result<Values> {
    constant.repeat(constant.beside(beside.kind)?, beside.len())
}

/// Bring two columns of equal length to the kind they combine in.
pub(crate) fn together(left: &Values, right: &Values) -> Result<(Values, Values)> {
    let kind = left.kind.with(right.kind);
    Ok((left.convert(kind)?, right.convert(kind)?))
}

/// Python's remainder, which takes the sign of the divisor (a zero too), from libm's exact `fmod`.
fn remainder(left: f64, right: f64) -> f64 {
    let rest = libm::fmod(left, right);
    if rest == 0.0 {
        libm::copysign(0.0, right)
    } else if (rest < 0.0) != (right < 0.0) {
        rest + right
    } else {
        rest
    }
}

/// `base` to a whole power, wrapping in 64 bits, by squaring.
fn power(mut base: i64, mut exponent: u64) -> i64 {
    let mut out: i64 = 1;
    while exponent > 0 {
        if exponent & 1 == 1 {
            out = out.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exponent >>= 1;
    }
    out
}

/// Whether `a OPERATOR b` holds, for a comparison.
fn holds<T: PartialOrd>(operator: Operator, a: T, b: T) -> bool {
    use Operator::*;
    match operator {
        Less => a < b,
        LessEqual => a <= b,
        Greater => a > b,
        GreaterEqual => a >= b,
        Equal => a == b,
        _ => a != b,
    }
}

/// The words a truth operator refuses a number with.
pub(crate) fn not_truth(what: &str, kind: &str) -> Error {
    problem(format!("{what} needs a true-or-false value, not {kind}: to test a number, compare it, as x != 0"))
}

/// Apply one operator to two columns of the same length.
fn apply(operator: Operator, left: &Values, right: &Values) -> Result<Values> {
    use Operator::*;
    if matches!(operator, And | Or) {
        let (a, b) = (left.truths(operator.word())?, right.truths(operator.word())?);
        let join = |(a, b): (&i64, &i64)| if operator == And { a & b } else { a | b };
        return Ok(Values::whole(Kind::B1, a.iter().zip(b.iter()).map(join).collect()));
    }
    let arithmetic = operator.arithmetic();
    // Two whole numbers compare exactly, each as its own kind holds it, never through a float.
    if let (false, Some(a), Some(b)) = (arithmetic, left.wholes(), right.wholes()) {
        let (from, to) = (left.kind, right.kind);
        let compare = |(a, b): (&i64, &i64)| holds(operator, from.widen(*a), to.widen(*b)) as i64;
        return Ok(Values::whole(Kind::B1, a.iter().zip(b).map(compare).collect()));
    }
    let (mut left, mut right) = together(left, right)?;
    if arithmetic && left.kind == Kind::B1 {
        return Err(problem("Arithmetic on two true-or-false values: use and, or, or count, or make one a number, as if(x, 1, 0)"));
    }
    if operator == Divide && !left.kind.float() {
        (left, right) = (left.convert(Kind::F8)?, right.convert(Kind::F8)?);
    }
    let kind = left.kind;
    match (&left.store, &right.store) {
        (Store::Float(a), Store::Float(b)) => {
            let pairs = a.iter().zip(b.iter());
            if !arithmetic {
                return Ok(Values::whole(Kind::B1, pairs.map(|(a, b)| holds(operator, a, b) as i64).collect()));
            }
            let compute = |(a, b): (&f64, &f64)| match operator {
                Add => a + b,
                Subtract => a - b,
                Multiply => a * b,
                Divide => a / b,
                Remainder => remainder(*a, *b),
                _ if *b == 2.0 => a * a,
                _ => libm::pow(*a, *b),
            };
            Ok(Values::float(kind, pairs.map(compute).map(|value| kind.round(value)).collect()))
        }
        (Store::Whole(a), Store::Whole(b)) => {
            let mut out = Vec::with_capacity(a.len());
            for (at, (a, b)) in a.iter().zip(b.iter()).enumerate() {
                let value = match operator {
                    Add => a.wrapping_add(*b),
                    Subtract => a.wrapping_sub(*b),
                    Multiply => a.wrapping_mul(*b),
                    Remainder => whole_remainder(kind, *a, *b),
                    _ if kind != Kind::U8 && *b < 0 => {
                        return Err(data(at, format!("{a} ** {b}: a whole number cannot be raised to a negative power; make it a float first, as {a}.0 ** {b}")))
                    }
                    _ => power(*a, *b as u64),
                };
                out.push(kind.wrap(value));
            }
            Ok(Values::whole(kind, out))
        }
        _ => unreachable!("together gives one kind"),
    }
}

/// The first event whose list has another length in `a` than in `b`, offsets over the same events.
pub(crate) fn differs(a: &[usize], b: &[usize]) -> Option<usize> {
    (0..a.len().min(b.len()).saturating_sub(1)).find(|&e| a[e + 1] - a[e] != b[e + 1] - b[e])
}

/// Refuse two lists `what` needs alike, at the first event where their lengths differ.
pub(crate) fn unlike(a: &[usize], b: &[usize], what: &str) -> Error {
    match differs(a, b) {
        Some(e) => data(e, format!("{what}, and in this event one has {} members and the other {}", a[e + 1] - a[e], b[e + 1] - b[e])),
        None => problem(format!("{what}, and they cover {} and {} events", a.len() - 1, b.len() - 1)),
    }
}

/// The lists `offsets` lays out over `values`, a data error among the members being of their event.
fn listed(offsets: &Arc<Vec<usize>>, values: Result<Values>) -> Result<Array> {
    match values {
        Ok(values) => Ok(Array::Object { offsets: offsets.clone(), values }),
        Err(Error(said, at, cause)) => Err(Error(said, at.map(|member| offsets.partition_point(|offset| *offset <= member) - 1), cause)),
    }
}

impl Array {
    /// The same array, each NaN it holds made `QUIET_NAN`: what a machine makes of 0/0 is not the same everywhere.
    pub fn settled(self) -> Array {
        match self {
            Array::Event(values) => Array::Event(values.settled()),
            Array::Object { offsets, values } => Array::Object { offsets, values: values.settled() },
            constant => constant,
        }
    }

    /// The number of events an array covers, when it is not a constant.
    pub fn events(&self) -> Option<usize> {
        match self {
            Array::Constant(_) => None,
            Array::Event(values) => Some(values.len()),
            Array::Object { offsets, .. } => Some(offsets.len() - 1),
        }
    }

    /// Its kind, and whether it holds a list per event; `None` for a fixed value.
    pub fn shape(&self) -> Option<(Kind, bool)> {
        match self {
            Array::Constant(_) => None,
            Array::Event(values) => Some((values.kind, false)),
            Array::Object { values, .. } => Some((values.kind, true)),
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

    /// This array's values `map`ped in its shape; a constant is `fixed`'s.
    pub(crate) fn map_values(&self, fixed: impl FnOnce(&Constant) -> Result<Array>, map: impl FnOnce(&Values) -> Result<Values>) -> Result<Array> {
        match self {
            Array::Constant(constant) => fixed(constant),
            Array::Event(values) => Ok(Array::Event(map(values)?)),
            Array::Object { offsets, values } => listed(offsets, map(values)),
        }
    }

    /// An operator between two arrays, a comparison with a field the sample lacks false.
    pub(crate) fn combine(&self, operator: Operator, other: &Array) -> Result<Array> {
        let compares = !matches!(operator, Operator::And | Operator::Or) && !operator.arithmetic();
        match (self, other) {
            (Array::Constant(Constant::Missing(_)), beside) | (beside, Array::Constant(Constant::Missing(_))) if compares => {
                return Ok(beside.all(false));
            }
            // Text beside a column of numbers: never equal, and nothing else.
            (Array::Constant(Constant::Text(_)), beside) | (beside, Array::Constant(Constant::Text(_))) if beside.events().is_some() => {
                return match operator {
                    Operator::Equal => Ok(beside.all(false)),
                    Operator::NotEqual => Ok(beside.all(true)),
                    _ => Err(problem(text_refused(operator))),
                };
            }
            _ => {}
        }
        self.zip(
            other,
            &|constant, beside| beside_column(operator, constant, beside),
            &|left, right| constants(operator, left, right),
            &|left, right| apply(operator, left, right),
        )
    }

    /// One truth value in the shape of this array: one per event, or per member of each list.
    fn all(&self, truth: bool) -> Array {
        let every = |values: &Values| Values::whole(Kind::B1, vec![truth as i64; values.len()]);
        match self {
            Array::Constant(_) => Array::Constant(Constant::Truth(truth)),
            Array::Event(values) => Array::Event(every(values)),
            Array::Object { offsets, values } => Array::Object { offsets: offsets.clone(), values: every(values) },
        }
    }

    /// Events `from..to` of this array; a fixed value is the same for all.
    pub fn rows(&self, from: usize, to: usize) -> Array {
        match self {
            Array::Constant(_) => self.clone(),
            Array::Event(values) => Array::Event(values.slice(from, to)),
            Array::Object { offsets, values } => {
                let (start, end) = (offsets[from], offsets[to]);
                let offsets: Vec<usize> = offsets[from..=to].iter().map(|o| o - start).collect();
                Array::Object { offsets: Arc::new(offsets), values: values.slice(start, end) }
            }
        }
    }

    /// The events at `positions` of this array, in that order; a fixed value is the same for all.
    pub fn picked(&self, positions: &[usize]) -> Array {
        match self {
            Array::Constant(_) => self.clone(),
            Array::Event(values) => Array::Event(values.take(positions)),
            Array::Object { offsets, values } => {
                let (mut members, mut lists) = (Vec::new(), Vec::with_capacity(positions.len() + 1));
                lists.push(0);
                for &at in positions {
                    members.extend(offsets[at]..offsets[at + 1]);
                    lists.push(members.len());
                }
                Array::Object { offsets: Arc::new(lists), values: values.take(&members) }
            }
        }
    }

    /// The events of every one of `runs`, one after another, of one kind and shape; one run as it is.
    pub fn joined(mut runs: Vec<Array>) -> Result<Array> {
        match runs.len() {
            0 => return Err(problem("There are no runs of events to join")),
            1 => return Ok(runs.remove(0)),
            _ => {}
        }
        let differ = || problem("The runs of one expression gave columns of different kinds or shapes");
        // Each run's values, only those its lists cover, and its lists counted from 0.
        let mut parts: Vec<(Values, Option<Vec<usize>>)> = Vec::with_capacity(runs.len());
        for run in &runs {
            parts.push(match run {
                Array::Event(values) => (values.clone(), None),
                Array::Object { offsets, values } => {
                    let (start, end) = (offsets[0], offsets[offsets.len() - 1]);
                    (values.slice(start, end), Some(offsets.iter().map(|o| o - start).collect()))
                }
                Array::Constant(_) => return Err(differ()),
            });
        }
        let (kind, listed) = (parts[0].0.kind, parts[0].1.is_some());
        if parts.iter().any(|(values, lists)| values.kind != kind || lists.is_some() != listed) {
            return Err(differ());
        }
        let store = match &parts[0].0.store {
            Store::Whole(_) => Store::Whole(Arc::new(parts.iter().flat_map(|(v, _)| v.wholes().into_iter().flatten().copied()).collect())),
            Store::Float(_) => Store::Float(Arc::new(parts.iter().flat_map(|(v, _)| v.floats().into_iter().flatten().copied()).collect())),
        };
        let values = Values { kind, store };
        if !listed {
            return Ok(Array::Event(values));
        }
        let mut offsets = vec![0];
        for (_, own) in &parts {
            let end = offsets[offsets.len() - 1];
            offsets.extend(own.iter().flatten().skip(1).map(|o| end + o));
        }
        Ok(Array::Object { offsets: Arc::new(offsets), values })
    }

    /// Every value converted to `kind` (`Values::convert`), a data error naming its event.
    pub fn convert(&self, kind: Kind) -> Result<Array> {
        self.map_values(|_| Err(problem("stored needs a column, not a fixed value")), |values| values.convert(kind))
    }

    /// Two arrays value by value: `adopt` makes a constant a column, `both` takes two constants, `apply` two columns.
    pub(crate) fn zip(
        &self,
        other: &Array,
        adopt: &dyn Fn(&crate::Constant, &Values) -> Result<Values>,
        both: &dyn Fn(&crate::Constant, &crate::Constant) -> Result<crate::Constant>,
        apply: &dyn Fn(&Values, &Values) -> Result<Values>,
    ) -> Result<Array> {
        use Array::*;
        match (self, other) {
            (Constant(a), Constant(b)) => both(a, b).map(Constant),
            (Constant(a), Event(b)) => Ok(Event(apply(&adopt(a, b)?, b)?)),
            (Event(a), Constant(b)) => Ok(Event(apply(a, &adopt(b, a)?)?)),
            (Constant(a), Object { offsets, values }) => listed(offsets, apply(&adopt(a, values)?, values)),
            (Object { offsets, values }, Constant(b)) => listed(offsets, apply(values, &adopt(b, values)?)),
            (Event(a), Event(b)) => {
                same(a.len(), b.len())?;
                Ok(Event(apply(a, b)?))
            }
            (Event(a), Object { offsets, values }) => {
                same(a.len(), offsets.len() - 1)?;
                listed(offsets, apply(&Self::spread(a, offsets), values))
            }
            (Object { offsets, values }, Event(b)) => {
                same(offsets.len() - 1, b.len())?;
                listed(offsets, apply(values, &Self::spread(b, offsets)))
            }
            (
                Object { offsets, values },
                Object {
                    offsets: others,
                    values: b,
                },
            ) => {
                if offsets != others {
                    return Err(unlike(offsets, others, "Two lists are combined member by member"));
                }
                listed(offsets, apply(values, b))
            }
        }
    }
}

/// Turn a constant into a column beside another column, in the kind NumPy would give it there.
fn beside_column(operator: Operator, constant: &Constant, beside: &Values) -> Result<Values> {
    if matches!(operator, Operator::And | Operator::Or) {
        return match constant {
            Constant::Truth(_) => constant.repeat(Kind::B1, beside.len()),
            Constant::Missing(key) => Err(lacking(key, "a true-or-false value")),
            Constant::Text(text) => Err(problem(format!("{} needs a true-or-false value, not the text {text:?}", operator.word()))),
            _ => Err(not_truth(operator.word(), "a number")),
        };
    }
    adopt(constant, beside)
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
            _ => Err(problem(text_refused(operator))),
        };
    }
    if matches!(left, Constant::Text(_)) || matches!(right, Constant::Text(_)) {
        return match operator {
            Equal => Ok(Constant::Truth(false)),
            NotEqual => Ok(Constant::Truth(true)),
            _ => Err(problem(text_refused(operator))),
        };
    }
    let (a, b) = (left.alone(1)?, right.alone(1)?);
    Ok(apply(operator, &a, &b).map_err(|e| problem(e.0))?.constant())
}
