//! A flat column's facts (docs/FORMAT.md §4), compared as numbers: their forms and `nice` made
//! again from `lim` on opening, and every fact made again from the values in full validation.

use super::{field, MEND};
use crate::decimal::{nice, Decimal};
use crate::numeric::width;
use crate::write::NumericBuffer;
use crate::write::facts::Facts;
use crate::{invalid, Result};
use serde_json::{Map, Value};

/// A JSON number as the whole number it is, whatever its spelling.
fn whole(value: &Value) -> Option<i128> {
    Decimal::parse(&value.as_number()?.to_string())?.to_whole()
}

/// A JSON number as the finite double it reads as.
fn double(value: &Value) -> Option<f64> {
    value.as_f64().filter(|x| x.is_finite())
}

/// How `same` compares two numbers.
#[derive(Clone, Copy)]
pub(super) enum Numbers {
    /// As the whole numbers they are.
    Whole,
    /// As the doubles they read as (so `-0.0` is `0.0`).
    Double,
    /// As written: whole numbers (no fraction or exponent) as whole numbers, other numbers as
    /// doubles, the one never equal to the other.
    Written,
}

/// Whether two values are the same, numbers compared as `numbers` says, lists in order, objects
/// by key in any order, everything else as it is.
pub(super) fn same(a: &Value, b: &Value, numbers: Numbers) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match numbers {
            Numbers::Whole => whole(a).is_some() && whole(a) == whole(b),
            Numbers::Double => double(a).is_some() && double(a) == double(b),
            Numbers::Written => match (crate::json::whole(x), crate::json::whole(y)) {
                (true, true) => same(a, b, Numbers::Whole),
                (false, false) => same(a, b, Numbers::Double),
                _ => false,
            },
        },
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b, numbers)),
        (Value::Object(a), Value::Object(b)) => a.len() == b.len() && a.iter().all(|(key, a)| b.get(key).is_some_and(|b| same(a, b, numbers))),
        _ => a == b,
    }
}

/// How a column of `kind`'s facts are compared: as whole numbers, or as doubles.
pub(super) fn numbers(kind: &str) -> Numbers {
    match crate::numeric::whole_kind(kind) {
        true => Numbers::Whole,
        false => Numbers::Double,
    }
}

/// Check a flat column's facts on opening: `lim` two numbers in order (whole numbers for a
/// whole-number kind), `nice` the rule's for them, and `vals` (on a whole-number kind only)
/// whole numbers ascending, at most 64, between `lim`'s ends, there whenever `lim` spans 64
/// values or fewer.
pub(super) fn forms(record: &Value, kind: &str, place: &str) -> Result<()> {
    let wholes = crate::numeric::whole_kind(kind);
    let number = |value: &Value| match wholes {
        true => whole(value).map(Decimal::whole),
        false => double(value).map(Decimal::of),
    };
    let what = if wholes { "whole numbers" } else { "finite numbers" };
    let lim = field(record, place, "lim")?;
    let ends = lim.as_array().filter(|ends| ends.len() == 2).and_then(|ends| ends.iter().map(number).collect::<Option<Vec<_>>>());
    let Some([lo, hi]) = ends.as_deref().map(|ends| [ends[0], ends[1]]) else {
        return Err(invalid(format!("{place}.lim must be two {what}, [min, max] (docs/FORMAT.md §4): {MEND}")));
    };
    let ordered = match wholes {
        true => whole(&lim[0]) <= whole(&lim[1]),
        false => double(&lim[0]) <= double(&lim[1]),
    };
    if !ordered {
        return Err(invalid(format!("{place}.lim is {lim}, and its first end is its least value (docs/FORMAT.md §4): {MEND}")));
    }
    let want: Vec<Value> = nice(lo, hi, wholes).iter().map(|end| match wholes {
        true => serde_json::Number::from_i128(end.to_whole().unwrap_or_default()).map_or(Value::Null, Value::Number),
        false => serde_json::Number::from_f64(end.to_f64()).map_or(Value::Null, Value::Number),
    }).collect();
    let have = field(record, place, "nice")?;
    if !same(have, &Value::Array(want.clone()), numbers(kind)) {
        return Err(invalid(format!("{place}.nice is {have}, and the rule of docs/FORMAT.md §4 makes {} of its lim {lim}: {MEND}", Value::Array(want))));
    }
    let Some(vals) = record.get("vals") else {
        let (lo, hi) = (whole(&lim[0]).unwrap_or_default(), whole(&lim[1]).unwrap_or_default());
        if wholes && hi.checked_sub(lo).is_some_and(|span| span < 64) {
            return Err(invalid(format!("{place} has no vals, and a whole-number column whose lim {lim} spans 64 values or fewer lists them (docs/FORMAT.md §4): {MEND}")));
        }
        return Ok(());
    };
    if !wholes {
        return Err(invalid(format!("{place} has vals, which a float column never has (docs/FORMAT.md §4): take it out")));
    }
    let listed = vals.as_array().and_then(|vals| vals.iter().map(whole).collect::<Option<Vec<_>>>());
    let fits = listed.as_ref().is_some_and(|listed| {
        let ends = match (listed.first(), listed.last()) {
            (Some(first), Some(last)) => Some(*first) == whole(&lim[0]) && Some(*last) == whole(&lim[1]),
            _ => whole(&lim[0]) == Some(0) && whole(&lim[1]) == Some(0),
        };
        listed.len() <= 64 && listed.windows(2).all(|pair| pair[0] < pair[1]) && ends
    });
    match fits {
        true => Ok(()),
        false => Err(invalid(format!("{place}.vals must be every distinct value, ascending, from lim's first end to its last, at most 64 of them (docs/FORMAT.md §4): {MEND}"))),
    }
}

/// Check a flat column's facts against its values: each made again, and equal as a number.
pub(super) fn values(record: &Value, array: &NumericBuffer, place: &str) -> Result<()> {
    let mut facts = Facts::new(&array.kind);
    for chunk in array.bytes.chunks(width(&array.kind)? << 16) {
        facts.add(chunk)?;
    }
    agree(record, &facts.finish(), &array.kind, place)
}

/// Require a record's facts to be `want`'s, compared as numbers, `vals` there exactly when wanted.
pub(super) fn agree(record: &Value, want: &Map<String, Value>, kind: &str, place: &str) -> Result<()> {
    for key in ["lim", "nice", "vals"] {
        match (record.get(key), want.get(key)) {
            (None, None) => {}
            (Some(have), Some(want)) if same(have, want, numbers(kind)) => {}
            (None, Some(want)) => return Err(invalid(format!("{place} has no {key}, and its values give {want} (docs/FORMAT.md §4): {MEND}"))),
            (Some(_), None) => return Err(invalid(format!("{place} has vals, and its values are more than 64 distinct ones, which have none (docs/FORMAT.md §4): {MEND}"))),
            (Some(have), Some(want)) => return Err(invalid(format!("{place}.{key} is {have}, and its values give {want} (docs/FORMAT.md §4): {MEND}"))),
        }
    }
    Ok(())
}
