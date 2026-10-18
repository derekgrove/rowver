//! The display facts of a flat column: `lim`, `nice`, and for integer kinds `vals` and `names`.
//!
//! Ported from: RowVer 13.0.1 (`2ec3a16`) `rowver/rvr.py`: `facts`, `outward`, `FEW`.
//! Underlying behaviours: NumPy 2.5.1 `ndarray.min`, `ndarray.max`, and `unique`; CPython 3.13
//! `round(x, 6)`, `math.floor`, `math.ceil`, and `math.log10`.
//! Documentation: `FORMAT.md` (repository root) ("lim", "nice", "vals", "names");
//! <https://numpy.org/doc/stable/reference/generated/numpy.unique.html>;
//! <https://docs.python.org/3/library/functions.html#round>
//! Difference: a flat column holding a NaN or an infinity is refused with a message; the Python
//! writer stops on the same column with an arithmetic error. Jagged content has no facts.

use crate::numeric::NumericArray;
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
    let step = 10_f64.powf(x.abs().log10().floor() - 1.0);
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

/// Work out the facts of one flat column, in the order the Python writer records them.
pub(super) fn facts(
    array: &NumericArray,
    labels: Option<&BTreeMap<i128, String>>,
    name: &str,
) -> Result<Map<String, Value>> {
    let integer = !array.kind.starts_with('f');
    let mut out = Map::new();
    if array.count == 0 {
        out.insert("lim".into(), Value::Array(vec![whole(0), whole(0)]));
        out.insert("nice".into(), Value::Array(vec![whole(0), whole(1)]));
        if integer {
            out.insert("vals".into(), Value::Array(Vec::new()));
        }
        return Ok(out);
    }
    if !integer {
        let (mut lo, mut hi, mut finite) = (f64::INFINITY, f64::NEG_INFINITY, true);
        array.each_value(|value| {
            finite &= value.is_finite();
            lo = if value < lo { value } else { lo };
            hi = if value > hi { value } else { hi };
        })?;
        if !finite {
            return Err(invalid(format!("{name}: floating values must be finite")));
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
    let (mut lo, mut hi, mut distinct) = (i128::MAX, i128::MIN, BTreeSet::new());
    // Rows of a sample often repeat a value; only a change can add to the set.
    let mut last = None;
    array.each_integer(|value| {
        lo = lo.min(value);
        hi = hi.max(value);
        if last != Some(value) && distinct.len() <= FEW {
            distinct.insert(value);
        }
        last = Some(value);
    })?;
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
                .filter_map(|value| Some((value.to_string(), labels.get(value)?.clone().into())))
                .collect();
            out.insert("names".into(), Value::Object(names));
        }
    }
    Ok(out)
}
