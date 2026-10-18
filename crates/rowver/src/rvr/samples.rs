use super::cancellation::Cancellation;
use super::{checksum, count, digest, hex, list, object, string};
use crate::numeric::{width, NumericArray};
use crate::{invalid, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub(crate) const RESERVED: [&str; 3] = ["sample", "entry", "file"];

/// Hash bytes in pieces so that a long sample can still be cancelled.
fn chunked(bytes: &[u8], cancellation: Cancellation<'_>) -> Result<String> {
    let mut hash = Sha256::new();
    for chunk in bytes.chunks(65536) {
        cancellation.check()?;
        hash.update(chunk);
    }
    Ok(hex(&hash.finalize()))
}

/// The half-open row range of every sample, in `data.samples` order, covering exactly `rows`.
pub(super) fn spans(data: &Value, rows: usize) -> Result<Vec<(usize, usize)>> {
    let mut start = 0_usize;
    let mut spans = Vec::new();
    for record in list(data, "samples")? {
        let end = start
            .checked_add(count(record, "rows")?)
            .ok_or_else(|| invalid("Sample rows overflow"))?;
        spans.push((start, end));
        start = end;
    }
    if start != rows {
        return Err(invalid("Sample row counts do not sum to rows"));
    }
    Ok(spans)
}

/// Require one recorded checksum per sample, each matching the bytes `part` gives for that sample.
fn compare(
    record: &Value,
    spans: &[(usize, usize)],
    label: &str,
    cancellation: Cancellation<'_>,
    part: impl Fn(usize, usize) -> Result<String>,
) -> Result<()> {
    let recorded =
        list(record, "by_sample").map_err(|_| invalid(format!("{label}: by_sample is missing")))?;
    if recorded.len() != spans.len() {
        return Err(invalid(format!(
            "{label}: by_sample needs one checksum per sample"
        )));
    }
    for (sum, (start, end)) in recorded.iter().zip(spans) {
        cancellation.check()?;
        if sum.as_str() != Some(part(*start, *end)?.as_str()) {
            return Err(invalid(format!(
                "{label}: by_sample does not match a sample's values"
            )));
        }
    }
    Ok(())
}

/// Check the per-sample checksums of an array holding one value per row.
pub(super) fn per_row(
    record: &Value,
    array: &NumericArray,
    spans: &[(usize, usize)],
    label: &str,
    cancellation: Cancellation<'_>,
) -> Result<()> {
    let w = width(&array.kind)?;
    compare(record, spans, label, cancellation, |start, end| {
        chunked(&array.bytes[start * w..end * w], cancellation)
    })
}

/// Check the per-sample checksums of a jagged column's rebased offsets and of its content.
pub(super) fn jagged(
    record: &Value,
    offsets: &[usize],
    content: &NumericArray,
    spans: &[(usize, usize)],
    name: &str,
    cancellation: Cancellation<'_>,
) -> Result<()> {
    let label = format!("{name} offsets");
    compare(
        object(record, "offsets")?,
        spans,
        &label,
        cancellation,
        |start, end| {
            let mut hash = Sha256::new();
            for (index, offset) in offsets[start..=end].iter().enumerate() {
                cancellation.at(index)?;
                hash.update(((offset - offsets[start]) as i64).to_le_bytes());
            }
            Ok(hex(&hash.finalize()))
        },
    )?;
    let w = width(&content.kind)?;
    let label = format!("{name} content");
    compare(
        object(record, "content")?,
        spans,
        &label,
        cancellation,
        |start, end| {
            chunked(
                &content.bytes[offsets[start] * w..offsets[end] * w],
                cancellation,
            )
        },
    )
}

/// Whether a JSON number is written as a whole number.
fn whole(number: &serde_json::Number) -> bool {
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
    let refused = || {
        invalid(format!(
            "Sample column {name} has a value its kind does not allow"
        ))
    };
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

/// Check every sample column and that the list is exactly what the samples' metadata gives.
pub(super) fn columns(header: &Value, data: &Value, made: &Value) -> Result<()> {
    let samples = list(data, "samples")?;
    let hooks = made.get("cart").and_then(|cart| cart.get("hooks"));
    let derived = header.get("derived").is_some();
    let pressed: Option<Vec<&str>> = match (derived, hooks) {
        (true, None) => None,
        (true, Some(hooks)) => Some(
            object(hooks, "columns")?
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect(),
        ),
        (false, _) => Some(
            list(data, "cols")?
                .iter()
                .map(|column| string(column, "name"))
                .collect::<Result<_>>()?,
        ),
    };
    let own: HashSet<&str> = list(data, "cols")?
        .iter()
        .map(|column| string(column, "name"))
        .collect::<Result<_>>()?;
    let mut held = HashMap::new();
    for record in list(data, "sample_cols")? {
        let fields = record.as_object().map_or(0, serde_json::Map::len);
        let name = string(record, "name")?;
        let kind = string(record, "kind")?;
        let values = list(record, "values")?;
        if fields != 4
            || !["b1", "i8", "f8", "text"].contains(&kind)
            || values.len() != samples.len()
        {
            return Err(invalid(format!(
                "Sample column {name} needs a name, a kind, a sha256, and one value per sample"
            )));
        }
        if RESERVED.contains(&name) || own.contains(name) || held.contains_key(name) {
            return Err(invalid(format!(
                "Sample column {name} repeats a name or takes a column's"
            )));
        }
        let values: Vec<&Value> = values.iter().collect();
        if value_checksum(kind, &values, name)? != checksum(record)? {
            return Err(invalid(format!(
                "Sample column {name} does not match its sha256"
            )));
        }
        held.insert(name, (kind, values));
    }
    let Some(pressed) = pressed else {
        return Ok(());
    };
    let files = object(made, "files")?;
    let records: Vec<&Value> = samples
        .iter()
        .map(|sample| Ok(&files[string(sample, "name")?]))
        .collect::<Result<_>>()?;
    let mut out: HashSet<&str> = pressed.into_iter().chain(RESERVED).collect();
    for record in &records {
        let omitted = record.get("metadata_omitted").and_then(Value::as_array);
        out.extend(omitted.into_iter().flatten().filter_map(Value::as_str));
    }
    let mut expected = 0;
    let keys = records
        .iter()
        .filter_map(|record| record.get("metadata").and_then(Value::as_object))
        .flat_map(|metadata| metadata.keys());
    for key in keys.collect::<HashSet<_>>() {
        let values: Vec<&Value> = records
            .iter()
            .map(|record| {
                record
                    .get("metadata")
                    .and_then(|all| all.get(key))
                    .unwrap_or(&Value::Null)
            })
            .collect();
        let Some(kind) = (!out.contains(key.as_str()))
            .then(|| kind_of(&values))
            .flatten()
        else {
            continue;
        };
        expected += 1;
        if held.get(key.as_str()) != Some(&(kind, values)) {
            return Err(invalid(format!(
                "Sample column {key} is not what the samples' metadata gives"
            )));
        }
    }
    if expected != held.len() {
        return Err(invalid(
            "The sample columns are not exactly those the samples' metadata gives",
        ));
    }
    Ok(())
}
