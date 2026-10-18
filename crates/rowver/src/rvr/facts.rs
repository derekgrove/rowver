use super::cancellation::Cancellation;
use super::{list, string};
use crate::numeric::NumericArray;
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::BTreeSet;

/// Validate mandatory flat-column limits and the enclosing display interval.
pub(super) fn validate(
    record: &Value,
    array: &NumericArray,
    cancellation: Cancellation<'_>,
) -> Result<()> {
    let name = string(record, "name")?;
    let lim = list(record, "lim")?;
    let nice = list(record, "nice")?;
    if lim.len() != 2 || nice.len() != 2 {
        return Err(invalid(format!(
            "{name}: lim and nice must contain two endpoints"
        )));
    }
    let lo = lim[0]
        .as_f64()
        .ok_or_else(|| invalid(format!("{name}: invalid lower limit")))?;
    let hi = lim[1]
        .as_f64()
        .ok_or_else(|| invalid(format!("{name}: invalid upper limit")))?;
    let nice_lo = nice[0]
        .as_f64()
        .ok_or_else(|| invalid(format!("{name}: invalid nice lower endpoint")))?;
    let nice_hi = nice[1]
        .as_f64()
        .ok_or_else(|| invalid(format!("{name}: invalid nice upper endpoint")))?;
    if ![lo, hi, nice_lo, nice_hi]
        .iter()
        .all(|value| value.is_finite())
        || nice_lo > lo
        || hi > nice_hi
        || nice_lo >= nice_hi
        || lo > hi
    {
        return Err(invalid(format!(
            "{name}: display interval must enclose finite ordered limits"
        )));
    }
    let float = array.kind.starts_with('f');
    let mut actual_lo = f64::INFINITY;
    let mut actual_hi = f64::NEG_INFINITY;
    for index in 0..array.count {
        cancellation.at(index)?;
        let value = array.value(index)?;
        if float && !value.is_finite() {
            return Err(invalid(format!("{name}: floating values must be finite")));
        }
        actual_lo = actual_lo.min(value);
        actual_hi = actual_hi.max(value);
    }
    if array.count == 0 {
        actual_lo = 0.0;
        actual_hi = 0.0;
    }
    if lo != actual_lo || hi != actual_hi {
        return Err(invalid(format!(
            "{name}: declared limits differ from the array"
        )));
    }
    if !array.kind.starts_with('f') {
        integers(record, array, cancellation)?;
    }
    if let Some(names) = record.get("names") {
        let names = names
            .as_object()
            .ok_or_else(|| invalid(format!("{name}: names must be an object")))?;
        let vals = list(record, "vals")?;
        for (key, label) in names {
            let key = key
                .parse::<i128>()
                .map_err(|_| invalid(format!("{name}: label keys must be integer values")))?;
            if !label.is_string() || !vals.iter().any(|value| exact(value).ok() == Some(key)) {
                return Err(invalid(format!("{name}: labels must name members of vals")));
            }
        }
    }
    Ok(())
}

/// Validate integer extrema and value labels without a lossy f64 conversion.
fn integers(record: &Value, array: &NumericArray, cancellation: Cancellation<'_>) -> Result<()> {
    let name = string(record, "name")?;
    let mut values = BTreeSet::new();
    let mut lo = i128::MAX;
    let mut hi = i128::MIN;
    for index in 0..array.count {
        cancellation.at(index)?;
        let value = array.integer(index)?;
        lo = lo.min(value);
        hi = hi.max(value);
        if values.len() <= 64 {
            values.insert(value);
        }
    }
    if array.count == 0 {
        lo = 0;
        hi = 0;
    }
    let lim: Vec<i128> = list(record, "lim")?
        .iter()
        .map(exact)
        .collect::<Result<_>>()?;
    if lim != [lo, hi] {
        return Err(invalid(format!(
            "{name}: integer limits differ from exact array values"
        )));
    }
    let nice: Vec<i128> = list(record, "nice")?
        .iter()
        .map(exact)
        .collect::<Result<_>>()?;
    if nice[0] > lo || hi > nice[1] {
        return Err(invalid(format!(
            "{name}: nice does not enclose exact integer limits"
        )));
    }
    if let Some(vals) = record.get("vals") {
        let vals = vals
            .as_array()
            .ok_or_else(|| invalid(format!("{name}: vals must be an array")))?;
        let vals: Vec<i128> = vals.iter().map(exact).collect::<Result<_>>()?;
        if values.len() > 64 || vals != values.into_iter().collect::<Vec<_>>() {
            return Err(invalid(format!(
                "{name}: vals differ from exact distinct array values"
            )));
        }
    } else if values.len() <= 64 {
        return Err(invalid(format!(
            "{name}: integer columns with at most 64 values require vals"
        )));
    }
    Ok(())
}

/// Require an integer JSON spelling large enough for every supported dtype.
fn exact(value: &Value) -> Result<i128> {
    value
        .as_number()
        .and_then(|number| number.to_string().parse().ok())
        .ok_or_else(|| invalid("Integer column facts must contain exact integers"))
}
