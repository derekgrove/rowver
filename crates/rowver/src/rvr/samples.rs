use super::{checksum, digest, exactly, list, nonempty, MEND};
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(crate) use rvr_recipe::RESERVED;

/// Work out the kind of sample column these per-sample values make, if they make one.
pub(crate) fn kind_of(values: &[&Value]) -> Option<&'static str> {
    let present: Vec<&&Value> = values.iter().filter(|value| !value.is_null()).collect();
    let complete = present.len() == values.len();
    if present.is_empty() {
        return None;
    }
    if present.iter().all(|value| value.is_boolean()) {
        return complete.then_some("b1");
    }
    if present.iter().all(|value| value.is_string()) {
        return Some("text");
    }
    let numbers: Vec<&serde_json::Number> = present
        .iter()
        .map(|value| value.as_number())
        .collect::<Option<_>>()?;
    if complete && numbers.iter().all(|number| crate::json::whole(number)) {
        return numbers
            .iter()
            .all(|number| number.as_i64().is_some())
            .then_some("i8");
    }
    numbers
        .iter()
        .all(|number| number.as_f64().is_some_and(f64::is_finite))
        .then_some("f8")
}

/// Hash a sample column's values as the bytes of its kind, or refuse values its kind does not allow.
pub(crate) fn value_checksum(kind: &str, values: &[&Value], name: &str) -> Result<String> {
    let refused = || invalid(format!("data.sample_cols[{name:?}] has a value its kind {kind} does not allow (docs/FORMAT.md §5): {MEND}"));
    let mut bytes = Vec::new();
    for value in values {
        match (kind, value) {
            ("b1", Value::Bool(flag)) => bytes.push(u8::from(*flag)),
            ("i8", Value::Number(number)) if crate::json::whole(number) => {
                bytes.extend(number.as_i64().ok_or_else(refused)?.to_le_bytes())
            }
            ("f8", Value::Null) => bytes.extend(f64::NAN.to_bits().to_le_bytes()),
            ("f8", Value::Number(number)) => {
                let value = number.as_f64().filter(|value| value.is_finite());
                bytes.extend(value.ok_or_else(refused)?.to_le_bytes())
            }
            ("text", Value::Null) => bytes.push(0),
            ("text", Value::String(text)) => {
                bytes.push(1);
                bytes.extend(text.as_bytes());
                bytes.push(0);
            }
            _ => return Err(refused()),
        }
    }
    Ok(digest(&bytes))
}

/// Check every sample column and that the list is exactly what the samples' metadata gives, in
/// both directions. The names a row column takes are the pressed file's, which `made.recipe.columns` holds.
pub(super) fn columns(data: &Value, made: &Value) -> Result<()> {
    let samples = list(data, "data", "samples")?;
    let pressed = super::snapshot::pressed(made);
    let mut held: HashMap<&str, (&str, Vec<&Value>)> = HashMap::new();
    for (index, record) in list(data, "data", "sample_cols")?.iter().enumerate() {
        let name = nonempty(record, &format!("data.sample_cols[{index}]"), "name")?;
        let place = format!("data.sample_cols[{name:?}]");
        exactly(record, &place, &["name", "kind", "values", "sha256"], &[])?;
        let kind = super::text(record, &place, "kind")?;
        if !["b1", "i8", "f8", "text"].contains(&kind) {
            return Err(invalid(format!("{place}.kind is {kind:?}, and a sample column's kind is b1, i8, f8 or text (docs/FORMAT.md §5): {MEND}")));
        }
        let values = list(record, &place, "values")?;
        if values.len() != samples.len() {
            return Err(invalid(format!("{place}.values holds {} values, and it holds one per sample of data.samples ({}) (docs/FORMAT.md §5): {MEND}", values.len(), samples.len())));
        }
        if RESERVED.contains(&name) || pressed.contains(&name) || held.contains_key(name) {
            return Err(invalid(format!("{place}: a sample column is never named sample, entry or file, nor as a column the recipe pressed or another sample column (docs/FORMAT.md §5): {MEND}")));
        }
        let values: Vec<&Value> = values.iter().collect();
        if value_checksum(kind, &values, name)? != checksum(record, &place)? {
            return Err(invalid(format!("{place}.sha256 is not the SHA-256 of its values as bytes (docs/FORMAT.md §5): {MEND}")));
        }
        held.insert(name, (kind, values));
    }
    let records: Vec<&Value> = samples.iter().map(|sample| &made["files"][sample["name"].as_str().unwrap_or_default()]).collect();
    let out: HashSet<&str> = pressed.into_iter().chain(RESERVED).collect();
    let mut expected = 0;
    let keys = records.iter().filter_map(|record| record["metadata"].as_object()).flat_map(|metadata| metadata.keys());
    for key in keys.collect::<HashSet<_>>() {
        let values: Vec<&Value> = records.iter().map(|record| record["metadata"].get(key).unwrap_or(&Value::Null)).collect();
        let Some(kind) = (!out.contains(key.as_str())).then(|| kind_of(&values)).flatten() else {
            continue;
        };
        expected += 1;
        // Numbers as numbers, whatever their spelling: an f8 column's as doubles, an i8 one's as whole numbers.
        let numbers = super::facts::numbers(kind);
        let agrees = held.get(key.as_str()).is_some_and(|(k, v)| *k == kind && v.iter().zip(&values).all(|(a, b)| super::facts::same(a, b, numbers)));
        if !agrees {
            return Err(invalid(format!("data.sample_cols has no column {key:?} of kind {kind} holding each sample's metadata value, which the samples' metadata gives (docs/FORMAT.md §5): {MEND}")));
        }
    }
    if expected != held.len() {
        return Err(invalid(format!("data.sample_cols holds a column no key of the samples' metadata makes (docs/FORMAT.md §5): {MEND}")));
    }
    Ok(())
}
