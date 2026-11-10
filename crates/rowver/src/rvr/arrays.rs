use super::{checksum, count, exactly, MEND};
use crate::numeric::width;
use crate::{invalid, Result};
use serde_json::Value;

/// Where an array record is in the header, written out only when it is said.
pub(crate) enum Place<'a> {
    Named(&'static str),
    /// A column, by name or index, and its `offsets` or `content` for a jagged one.
    Column(Option<&'a str>, usize, Option<&'static str>),
}

impl std::fmt::Display for Place<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Place::Named(name) => f.write_str(name),
            Place::Column(name, index, part) => {
                match name {
                    Some(name) => write!(f, "data.cols[{name:?}]")?,
                    None => write!(f, "data.cols[{index}]")?,
                }
                part.map_or(Ok(()), |part| write!(f, ".{part}"))
            }
        }
    }
}

/// Every array record of `data` in record order (a jagged column's offsets, then content), with its
/// place and the kind of its values.
pub(crate) fn records(data: &Value) -> Vec<(&Value, Place<'_>, &str)> {
    let mut out = vec![(&data["sample"], Place::Named("data.sample"), "i4"), (&data["entry"], Place::Named("data.entry"), "i8")];
    for (index, column) in data["cols"].as_array().into_iter().flatten().enumerate() {
        let name = column["name"].as_str();
        match column["kind"] == "jagged" {
            true => {
                let content = column["content"]["kind"].as_str().unwrap_or("u1");
                out.extend([(&column["offsets"], Place::Column(name, index, Some("offsets")), "i8"), (&column["content"], Place::Column(name, index, Some("content")), content)]);
            }
            false => out.push((column, Place::Column(name, index, None), column["kind"].as_str().unwrap_or("u1"))),
        }
    }
    out
}

/// Check the arrays, in any order, fill the bytes from `at` 0 with no gap and no overlap (list
/// columns that record one offsets array alike share it), each at
/// least its chunk table and a frame of 13 bytes a lane per chunk (docs/FORMAT.md §7, §12), and
/// `sample`'s and `entry`'s fields; where they end.
pub(super) fn layout(data: &Value) -> Result<u64> {
    exactly(&data["sample"], "data.sample", &["at", "size", "sha256"], &[])?;
    exactly(&data["entry"], "data.entry", &["at", "size", "sha256", "by_sample"], &[])?;
    let chunks = crate::chunks::count(data).ok_or_else(|| invalid(format!("data.samples' rows add up past what a count holds (2^64): {MEND}")))?;
    let mut placed = Vec::new();
    // The offsets arrays met, by where they lie: a list column's may be an earlier one's (§7).
    let mut offsets: std::collections::HashMap<(u64, u64), &Value> = std::collections::HashMap::new();
    let mut alike: std::collections::HashMap<&str, (u64, String)> = std::collections::HashMap::new();
    for (record, place, kind) in records(data) {
        let listed = matches!(place, Place::Column(_, _, Some("offsets")));
        let place = place.to_string();
        checksum(record, &place)?;
        let (at, size) = (count(record, &place, "at")? as u64, count(record, &place, "size")? as u64);
        let w = width(kind).unwrap_or(1) as u64;
        let least = chunks.saturating_mul(8 * w + 32 + w * crate::chunks::FRAME as u64);
        if chunks == 0 && size != 0 {
            return Err(invalid(format!("{place}.size is {size}, and an array of no chunks (a file of no rows) holds no bytes: size 0 (docs/FORMAT.md §7): {MEND}")));
        }
        if size < least {
            return Err(invalid(format!("{place}.size is {size}, and its {chunks} chunks take at least {least} bytes: a chunk table of {} bytes, and a frame of 13 bytes or more a lane per chunk (docs/FORMAT.md §7): {MEND}", chunks.saturating_mul(8 * w + 32))));
        }
        // One array recorded alike by each column that shares it takes its bytes once.
        if listed && size > 0 {
            // Equal offsets are one array: two arrays of one `sha256` are the same lists kept twice.
            let (first, column) = alike.entry(record["sha256"].as_str().unwrap_or_default()).or_insert_with(|| (at, place.clone()));
            if *first != at {
                return Err(invalid(format!("{place} holds the offsets {column} holds (their sha256 is one) as an array of its own at {at}: list columns of the same lists share one offsets array, recorded alike by each (docs/FORMAT.md §4): {MEND}")));
            }
            match offsets.get(&(at, size)) {
                Some(first) if *first == record => continue,
                Some(_) => {}
                None => drop(offsets.insert((at, size), record)),
            }
        }
        placed.push((at, size, place));
    }
    // In the order they lie, each array begins where the ones before it end.
    placed.sort_by_key(|(at, _, _)| *at);
    let mut end = 0_u64;
    for (at, size, place) in placed {
        if at != end {
            return Err(invalid(format!("{place}.at is {at}, but the arrays before it end at {end}: the arrays fill the bytes from 8 to the header with no gap and no overlap, in any order (docs/FORMAT.md §7): {MEND}")));
        }
        end = end.checked_add(size).ok_or_else(|| invalid(format!("{place}: the arrays end past 2^64 bytes: {MEND}")))?;
    }
    Ok(end)
}

/// Check the arrays, which `layout` found to end at `end`, fill the `length` bytes from byte 8 to the header, unless the file is empty.
pub(super) fn fill(end: u64, length: usize) -> Result<()> {
    if length != 0 && end != length as u64 {
        return Err(invalid(format!("the arrays' records end at byte {end} after byte 8, and the file holds {length} bytes from byte 8 to its header: the arrays fill them exactly (docs/FORMAT.md §1, §7); a file cut short or with bytes added is damaged: copy it again, or press its study again")));
    }
    Ok(())
}

/// Where the arrays a `data` section lays out end: the furthest end of any; `None` past what a count holds.
pub(crate) fn end(data: &Value) -> Option<u64> {
    records(data).iter().try_fold(0_u64, |end, (record, _, _)| Some(end.max(record["at"].as_u64().unwrap_or(0).checked_add(record["size"].as_u64().unwrap_or(0))?)))
}
