//! The facts of a flat column (docs/FORMAT.md §4): `lim` over its finite values, `nice` by the
//! exact decimal rule, and for whole-number kinds `vals`.

use crate::decimal::{nice, Decimal};
use crate::numeric::{each_integer, each_value};
use crate::Result;
use serde_json::{Map, Number, Value};
use std::collections::BTreeSet;

/// The most distinct values an integer column may have and still list them.
const FEW: usize = 64;

/// A double as a JSON number.
fn float(value: f64) -> Value {
    Value::Number(Number::from_f64(value).expect("a fact is a finite number"))
}

/// An exact whole number as a JSON number.
fn whole(value: i128) -> Value {
    Value::Number(Number::from_i128(value).expect("arbitrary precision holds every i128"))
}

/// The facts of one flat column, gathered a piece at a time in row order.
pub(crate) struct Facts {
    kind: String,
    whole: bool,
    bytes: usize,
    low: f64,
    high: f64,
    lo: i128,
    hi: i128,
    distinct: BTreeSet<i128>,
    last: Option<i128>,
}

impl Facts {
    /// Nothing seen yet of a column of `kind`.
    pub(crate) fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            whole: crate::numeric::whole_kind(kind),
            bytes: 0,
            low: f64::INFINITY,
            high: f64::NEG_INFINITY,
            lo: i128::MAX,
            hi: i128::MIN,
            distinct: BTreeSet::new(),
            last: None,
        }
    }

    /// See the next values, as little-endian bytes. NaN and infinities are passed over.
    pub(crate) fn add(&mut self, bytes: &[u8]) -> Result<()> {
        self.bytes += bytes.len();
        if !self.whole {
            let (low, high) = (&mut self.low, &mut self.high);
            return each_value(&self.kind, bytes, |value| {
                if value.is_finite() {
                    *low = if value < *low { value } else { *low };
                    *high = if value > *high { value } else { *high };
                }
            });
        }
        let (lo, hi, distinct, last) = (&mut self.lo, &mut self.hi, &mut self.distinct, &mut self.last);
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

    /// Take in the facts of the values after these, seen apart: one pass over both, in row order.
    pub(crate) fn merge(&mut self, other: Facts) {
        self.bytes += other.bytes;
        // As add() does, so the first of two equal ends stays (0.0 or -0.0).
        if other.low < self.low {
            self.low = other.low;
        }
        if other.high > self.high {
            self.high = other.high;
        }
        (self.lo, self.hi) = (self.lo.min(other.lo), self.hi.max(other.hi));
        for value in other.distinct {
            if self.distinct.len() > FEW {
                break;
            }
            self.distinct.insert(value);
        }
    }

    /// The facts: `lim`, `nice`, and `vals` on a whole-number column of at most 64 values.
    pub(crate) fn finish(self) -> Map<String, Value> {
        let mut out = Map::new();
        if !self.whole {
            let [lo, hi] = match self.low <= self.high {
                true => [self.low, self.high],
                false => [0.0, 0.0],
            };
            let ends = nice(Decimal::of(lo), Decimal::of(hi), false);
            out.insert("lim".into(), Value::Array(vec![float(lo), float(hi)]));
            out.insert("nice".into(), ends.iter().map(|end| float(end.to_f64())).collect());
            return out;
        }
        let [lo, hi] = match self.bytes {
            0 => [0, 0],
            _ => [self.lo, self.hi],
        };
        let ends = nice(Decimal::whole(lo), Decimal::whole(hi), true);
        out.insert("lim".into(), Value::Array(vec![whole(lo), whole(hi)]));
        out.insert("nice".into(), ends.iter().map(|end| whole(end.to_whole().expect("a nice end of a whole number is whole"))).collect());
        if self.distinct.len() <= FEW {
            out.insert("vals".into(), self.distinct.iter().copied().map(whole).collect());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Facts;

    // FORMAT 4
    #[test]
    fn facts_merged_in_chunk_order_are_those_of_one_pass_in_row_order() {
        let bytes = |values: &[f64]| values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        // A zero of each sign, at the low end and at the high end, in the first chunk and a later one.
        for (rest, first, later) in [(1.0, 0.0, -0.0), (1.0, -0.0, 0.0), (-1.0, 0.0, -0.0), (-1.0, -0.0, 0.0)] {
            let chunks = [vec![rest, first, rest], vec![rest; 3], vec![rest, later, rest]];
            let mut once = Facts::new("f8");
            once.add(&bytes(&chunks.concat())).unwrap();
            let mut merged = Facts::new("f8");
            for chunk in &chunks {
                let mut seen = Facts::new("f8");
                seen.add(&bytes(chunk)).unwrap();
                merged.merge(seen);
            }
            let (once, merged) = (once.finish(), merged.finish());
            assert_eq!(serde_json::to_string(&merged).unwrap(), serde_json::to_string(&once).unwrap(), "{rest} {first} {later}");
            let zero = if first.is_sign_negative() { "-0.0" } else { "0.0" };
            let lim = if rest > 0.0 { format!("[{zero},1.0]") } else { format!("[-1.0,{zero}]") };
            assert_eq!(once["lim"].to_string(), lim, "the zero first seen");
        }
    }
}
