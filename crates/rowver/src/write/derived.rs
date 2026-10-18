//! Build a derived file: columns computed from another file's columns, naming that parent.
//!
//! Ported from: RowVer 13.0.1 (`2ec3a16`) `rowver/derive.py` `derive` and `column_mark`, for
//! columns that are already computed.
//! Documentation: `FORMAT.md` (repository root) ("derived")
//! Differences: the caller computes the columns and says which parent columns it read; every
//! column is marked by the whole source, a newline, and its name, the format's rule for a
//! column that no readable function file made.

use super::{build_with, Labels, Skim, Values};
use crate::rvr::digest;
use crate::rvr::samples::RESERVED;
use crate::{invalid, Result};
use serde_json::{json, Map, Value};

/// What a derived file records of where its columns came from.
#[derive(Clone, Debug)]
pub struct Lineage<'a> {
    /// The parent's whole header.
    pub parent: &'a Value,
    /// The path the parent was opened with; a hint only.
    pub path: &'a str,
    /// The checksum of every parent column and sample column that was read, by name.
    pub read: Map<String, Value>,
    /// The exact text that computed the columns.
    pub source: &'a str,
    /// The program writing the file.
    pub by: &'a str,
    /// Plain JSON values the computation was given.
    pub parameters: Map<String, Value>,
    /// What ran `source`.
    pub runtime: &'a str,
    /// When the file is written, UTC to the second.
    pub built: &'a str,
}

/// Read a member of a parent's header that must be there.
fn held<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    let found = value.get(key).filter(|found| !found.is_null());
    found.ok_or_else(|| invalid(format!("The parent's header has no {key}")))
}

/// The header and the compressed arrays of a derived file, compressed on every core.
///
/// `sample` and `entry` are the parent's arrays, read from it; `columns` are the new columns,
/// one value or one list per parent row.
pub fn derived(
    lineage: Lineage,
    sample: Vec<i32>,
    entry: Vec<i64>,
    columns: Vec<(String, Values)>,
    labels: Labels,
) -> Result<(Value, Vec<Vec<u8>>)> {
    let workers = crate::parallel::cores();
    derived_with(lineage, sample, entry, columns, labels, workers)
}

/// As `derived`, compressing on at most `workers` threads; the bytes do not depend on how many.
pub fn derived_with(
    lineage: Lineage,
    sample: Vec<i32>,
    entry: Vec<i64>,
    columns: Vec<(String, Values)>,
    labels: Labels,
    workers: usize,
) -> Result<(Value, Vec<Vec<u8>>)> {
    if columns.is_empty() {
        return Err(invalid("There is no column to derive"));
    }
    let above = held(lineage.parent, "data")?;
    let made = held(lineage.parent, "made")?;
    let shared = held(above, "sample_cols")?;
    let named = |list: &Value, name: &str| {
        let records = list.as_array().into_iter().flatten();
        records.into_iter().any(|record| record["name"] == name)
    };
    for (name, _) in &columns {
        if named(&above["cols"], name) || named(shared, name) {
            return Err(invalid(format!("The parent already holds {name}")));
        }
        if RESERVED.contains(&name.as_str()) {
            return Err(invalid(format!("A column cannot be named {name}")));
        }
    }
    let samples = held(above, "samples")?.as_array().into_iter().flatten();
    let samples = samples.map(|record| {
        let rows = record["rows"]
            .as_u64()
            .and_then(|rows| rows.try_into().ok());
        let name = record["name"].as_str().zip(rows);
        name.map(|(name, rows)| (name.to_owned(), rows))
            .ok_or_else(|| invalid("The parent's samples are not readable"))
    });
    let marks = columns.iter().map(|(name, _)| {
        let mark = digest(format!("{}\n{name}", lineage.source).as_bytes());
        (name.clone(), Value::String(mark))
    });
    let marks: Map<String, Value> = marks.collect();
    let skim = Skim {
        samples: samples.collect::<Result<_>>()?,
        files: held(above, "files")?
            .as_array()
            .cloned()
            .unwrap_or_default(),
        sample,
        entry,
        columns,
        labels,
    };
    let (mut data, arrays) = build_with(skim, workers)?;
    for array in ["sample", "entry"] {
        if data[array]["sha256"] != above[array]["sha256"] {
            return Err(invalid(format!(
                "The {array} array given is not the parent's"
            )));
        }
    }
    data["sample_cols"] = shared.clone();
    let mut record = Map::new();
    record.insert("built".into(), lineage.built.into());
    record.insert("by".into(), lineage.by.into());
    record.insert(
        "parents".into(),
        json!([{
            "data_sha256": held(above, "sha256")?,
            "made_sha256": held(made, "sha256")?,
            "derived_sha256": lineage.parent.get("derived").map(|step| &step["sha256"]),
            "rows": held(above, "rows")?,
            "path": lineage.path,
            "read": lineage.read,
        }]),
    );
    record.insert("source".into(), lineage.source.into());
    record.insert("columns".into(), Value::Object(marks));
    record.insert("parameters".into(), Value::Object(lineage.parameters));
    record.insert("runtime".into(), lineage.runtime.into());
    let sum = super::made_checksum(&Value::Object(record.clone()))?;
    let mut lineage = Map::new();
    lineage.insert("sha256".into(), sum.into());
    lineage.append(&mut record);
    let header = json!({"rowver": 13, "data": data, "made": made, "derived": lineage});
    Ok((header, arrays))
}
