use super::cancellation::Cancellation;
use super::{count, list, object, string};
use crate::numeric::NumericArray;
use crate::table::Source;
use crate::{invalid, EventIdentity, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Validate sample/file spans and recover exact source metadata once per file.
pub(super) fn validate(
    data: &Value,
    made: &Value,
    sample: &NumericArray,
    entry: &NumericArray,
    rows: usize,
    cancellation: Cancellation<'_>,
) -> Result<Vec<Source>> {
    let mut names = HashMap::new();
    let mut sample_end = 0_usize;
    for (index, record) in list(data, "samples")?.iter().enumerate() {
        let name = string(record, "name")?;
        if name.is_empty() || names.insert(name.to_owned(), index).is_some() {
            return Err(invalid("Sample names must be nonempty and unique"));
        }
        let end = sample_end
            .checked_add(count(record, "rows")?)
            .filter(|end| *end <= rows)
            .ok_or_else(|| invalid("Sample rows exceed total rows"))?;
        for row in sample_end..end {
            cancellation.at(row)?;
            if sample.signed(row)? != index as i64 {
                return Err(invalid("Sample identity arrays disagree with sample spans"));
            }
        }
        sample_end = end;
    }
    if sample_end != rows {
        return Err(invalid("Sample row counts do not sum to rows"));
    }
    let sources = object(made, "files")?.as_object().unwrap();
    if sources.len() != names.len() || sources.keys().any(|name| !names.contains_key(name)) {
        return Err(invalid("made.files samples differ from data.samples"));
    }
    let mut expected = HashSet::new();
    for (name, source) in sources {
        for (path, metadata) in object(source, "files")?.as_object().unwrap() {
            if path.is_empty()
                || string(metadata, "uuid")?.is_empty()
                || string(metadata, "tree")?.is_empty()
            {
                return Err(invalid(
                    "Source files must have nonempty paths, UUIDs, and tree names",
                ));
            }
            count(metadata, "entries")?;
            if metadata.get("stop").is_some() {
                count(metadata, "stop")?;
            }
            expected.insert((name.as_str(), path.as_str()));
        }
    }
    let mut files = Vec::new();
    let mut start = 0_usize;
    let mut seen_files = HashSet::new();
    let mut previous_sample = 0;
    for record in list(data, "files")? {
        let name = string(record, "sample")?;
        let path = string(record, "path")?;
        let index = *names
            .get(name)
            .ok_or_else(|| invalid("Input file names an unknown sample"))?;
        if index < previous_sample
            || !seen_files.insert((name, path))
            || !expected.contains(&(name, path))
        {
            return Err(invalid(
                "Input files are duplicated, out of sample order, or absent from made.files",
            ));
        }
        previous_sample = index;
        let source = &sources[name]["files"][path];
        let entries = count(source, "entries")?;
        let read = if source.get("stop").is_some() {
            entries.min(count(source, "stop")?)
        } else {
            entries
        };
        let end = start
            .checked_add(count(record, "rows")?)
            .filter(|end| *end <= rows)
            .ok_or_else(|| invalid("Input file rows exceed total rows"))?;
        let mut seen_entries = HashSet::new();
        for row in start..end {
            cancellation.at(row)?;
            let source_entry = entry.signed(row)?;
            if sample.signed(row)? != index as i64
                || source_entry < 0
                || source_entry as u64 >= read as u64
                || !seen_entries.insert(source_entry)
            {
                return Err(invalid("File rows disagree with their sample or contain duplicate/out-of-range source entries"));
            }
        }
        files.push(Source {
            identity: EventIdentity {
                sample: name.into(),
                file: path.into(),
                entry: 0,
                uuid: string(source, "uuid")?.into(),
                tree: string(source, "tree")?.into(),
            },
            start,
            end,
        });
        start = end;
    }
    if start != rows || seen_files != expected {
        return Err(invalid(
            "Input file rows or file identities do not match the complete source snapshot",
        ));
    }
    Ok(files)
}
