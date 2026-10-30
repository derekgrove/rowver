//! The display facts of a flat column: `lim`, `nice`, and for integer kinds `vals` and `names`.
//!
//! Ported from: RowVer 13.0.1 (`2ec3a16`) `rowver/rvr.py`: `facts`, `outward`, `FEW`.
//! Underlying behaviours: NumPy 2.5.1 `ndarray.min`, `ndarray.max`, and `unique`; CPython 3.13
//! `round(x, 6)`, `math.floor`, `math.ceil`, and `math.log10`.
//! Documentation: `docs/FORMAT.md` ("lim", "nice", "vals", "names");
//! <https://numpy.org/doc/stable/reference/generated/numpy.unique.html>;
//! <https://docs.python.org/3/library/functions.html#round>
//! Difference: a flat column holding a NaN or an infinity is refused with a message; the Python
//! writer stops on the same column with an arithmetic error. Jagged content has no facts.

use crate::numeric::{each_integer, each_value};
use crate::{invalid, Result};
use serde_json::{Map, Number, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// The most distinct values an integer column may have and still list them.
const FEW: usize = 64;

/// Round away from the range to two significant digits; the caller keeps the limit itself
/// where this falls inside the range.
fn outward(x: f64, up: bool) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    let step = libm::pow(10.0, libm::log10(x.abs()).floor() - 1.0);
    let steps: f64 = format!("{:.6}", x / step).parse().unwrap();
    (if up { steps.ceil() } else { steps.floor() }) * step
}

/// Compare a float with a whole number exactly, as Python compares a float with an int.
fn against(value: f64, whole: i128) -> Ordering {
    let floor = value.floor();
    let fraction = if value > floor {
        Ordering::Greater
    } else {
        Ordering::Equal
    };
    (floor as i128).cmp(&whole).then(fraction)
}

/// Spell a float as a JSON number.
fn float(value: f64) -> Result<Value> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| invalid("A column fact is not a finite number"))
}

/// Spell an exact whole number as a JSON number.
fn whole(value: i128) -> Value {
    Value::Number(Number::from_i128(value).expect("arbitrary precision holds every i128"))
}

/// The facts of one flat column, gathered a piece at a time in row order.
pub(super) struct Facts {
    kind: String,
    seen: usize,
    finite: bool,
    low: f64,
    high: f64,
    lo: i128,
    hi: i128,
    distinct: BTreeSet<i128>,
    last: Option<i128>,
}

impl Facts {
    /// Nothing seen yet of a column of `kind`.
    pub(super) fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            seen: 0,
            finite: true,
            low: f64::INFINITY,
            high: f64::NEG_INFINITY,
            lo: i128::MAX,
            hi: i128::MIN,
            distinct: BTreeSet::new(),
            last: None,
        }
    }

    fn integer(&self) -> bool {
        !self.kind.starts_with('f')
    }

    /// See the next values, as little-endian bytes.
    pub(super) fn add(&mut self, bytes: &[u8]) -> Result<()> {
        self.seen += bytes.len();
        if !self.integer() {
            let (low, high, finite) = (&mut self.low, &mut self.high, &mut self.finite);
            return each_value(&self.kind, bytes, |value| {
                *finite &= value.is_finite();
                *low = if value < *low { value } else { *low };
                *high = if value > *high { value } else { *high };
            });
        }
        let (lo, hi, distinct, last) = (
            &mut self.lo,
            &mut self.hi,
            &mut self.distinct,
            &mut self.last,
        );
        // Rows of a sample often repeat a value; only a change can add to the set.
        each_integer(&self.kind, bytes, |value| {
            *lo = (*lo).min(value);
            *hi = (*hi).max(value);
            if *last != Some(value) && distinct.len() <= FEW {
                distinct.insert(value);
            }
            *last = Some(value);
        })
    }

    /// The facts, in the order the Python writer records them.
    pub(super) fn finish(
        self,
        labels: Option<&BTreeMap<i128, String>>,
        name: &str,
    ) -> Result<Map<String, Value>> {
        let integer = self.integer();
        let mut out = Map::new();
        if self.seen == 0 {
            out.insert("lim".into(), Value::Array(vec![whole(0), whole(0)]));
            out.insert("nice".into(), Value::Array(vec![whole(0), whole(1)]));
            if integer {
                out.insert("vals".into(), Value::Array(Vec::new()));
            }
            return Ok(out);
        }
        if !integer {
            let (lo, hi) = (self.low, self.high);
            if !self.finite {
                return Err(invalid(format!("{name}: some of its values are not finite numbers (NaN or infinity, as a division by zero gives), and a file holds only finite ones: give those rows a finite value")));
            }
            let mut nice = [outward(lo, false).min(lo), outward(hi, true).max(hi)];
            if nice[0] == nice[1] {
                nice[1] += 1.0;
            }
            out.insert("lim".into(), Value::Array(vec![float(lo)?, float(hi)?]));
            out.insert(
                "nice".into(),
                Value::Array(vec![float(nice[0])?, float(nice[1])?]),
            );
            return Ok(out);
        }
        let (lo, hi, distinct) = (self.lo, self.hi, self.distinct);
        let mut nice = [
            Some(outward(lo as f64, false))
                .filter(|end| against(*end, lo).is_le())
                .map_or(lo, |end| end.floor() as i128),
            Some(outward(hi as f64, true))
                .filter(|end| against(*end, hi).is_ge())
                .map_or(hi, |end| end.ceil() as i128),
        ];
        if nice[0] == nice[1] {
            nice[1] += 1;
        }
        out.insert("lim".into(), Value::Array(vec![whole(lo), whole(hi)]));
        out.insert(
            "nice".into(),
            Value::Array(vec![whole(nice[0]), whole(nice[1])]),
        );
        if distinct.len() <= FEW {
            out.insert(
                "vals".into(),
                Value::Array(distinct.iter().copied().map(whole).collect()),
            );
            if let Some(labels) = labels.filter(|labels| !labels.is_empty()) {
                let names = distinct
                    .iter()
                    .filter_map(|value| {
                        Some((value.to_string(), labels.get(value)?.clone().into()))
                    })
                    .collect();
                out.insert("names".into(), Value::Object(names));
            }
        }
        Ok(out)
    }
}
