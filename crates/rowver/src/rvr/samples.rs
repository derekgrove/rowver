use super::{checksum, digest, exactly, hex, list, nonempty, MEND};
use crate::numeric::{width, NumericArray};
use crate::{invalid, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub(crate) use rvr_recipe::RESERVED;

/// The half-open row range of every sample, in `data.samples` order (their sum checked on opening).
pub(super) fn spans(data: &Value) -> Result<Vec<(usize, usize)>> {
    let mut start = 0_usize;
    let mut spans = Vec::new();
    for record in list(data, "data", "samples")? {
        let end = start.checked_add(super::whole_count(&record["rows"]).unwrap_or(0) as usize).ok_or_else(|| invalid(format!("data.samples' rows add up past 2^64: {MEND}")))?;
        spans.push((start, end));
        start = end;
    }
    Ok(spans)
}

/// Require one recorded checksum per sample, each matching the bytes `part` gives for that sample.
fn compare(
    record: &Value,
    spans: &[(usize, usize)],
    place: &str,
    part: impl Fn(usize, usize) -> Result<String>,
) -> Result<()> {
    let recorded = list(record, place, "by_sample")?;
    for (index, (sum, (start, end))) in recorded.iter().zip(spans).enumerate() {
        if sum.as_str() != Some(part(*start, *end)?.as_str()) {
            return Err(invalid(format!("{place}.by_sample[{index}] is not the SHA-256 of that sample's part of the array (docs/FORMAT.md §8): {MEND}")));
        }
    }
    Ok(())
}

/// Check the per-sample checksums of an array holding one value per row.
pub(super) fn per_row(
    record: &Value,
    array: &NumericArray,
    spans: &[(usize, usize)],
    place: &str,
) -> Result<()> {
    let w = width(&array.kind)?;
    compare(record, spans, place, |start, end| Ok(digest(&array.bytes[start * w..end * w])))
}

/// Check the per-sample checksums of a jagged column's rebased offsets and of its content.
pub(super) fn jagged(
    record: &Value,
    offsets: &[usize],
    content: &NumericArray,
    spans: &[(usize, usize)],
    place: &str,
) -> Result<()> {
    compare(&record["offsets"], spans, &format!("{place}.offsets"), |start, end| {
        let mut hash = Sha256::new();
        for offset in &offsets[start..=end] {
            hash.update(((offset - offsets[start]) as i64).to_le_bytes());
        }
        Ok(hex(&hash.finalize()))
    })?;
    let w = width(&content.kind)?;
    compare(&record["content"], spans, &format!("{place}.content"), |start, end| Ok(digest(&content.bytes[offsets[start] * w..offsets[end] * w])))
}

/// Whether a JSON number is written as a whole number.
pub(super) fn whole(number: &serde_json::Number) -> bool {
    !number.to_string().contains(['.', 'e', 'E'])
}

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
    if complete && numbers.iter().all(|number| whole(number)) {
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
            ("i8", Value::Number(number)) if whole(number) => {
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
        let agrees = held.get(key.as_str()).is_some_and(|(k, v)| *k == kind && v.iter().zip(&values).all(|(a, b)| super::facts::same(a, b, super::facts::Numbers::Written)));
        if !agrees {
            return Err(invalid(format!("data.sample_cols has no column {key:?} of kind {kind} holding each sample's metadata value, which the samples' metadata gives (docs/FORMAT.md §5): {MEND}")));
        }
    }
    if expected != held.len() {
        return Err(invalid(format!("data.sample_cols holds a column no key of the samples' metadata makes (docs/FORMAT.md §5): {MEND}")));
    }
    Ok(())
}
