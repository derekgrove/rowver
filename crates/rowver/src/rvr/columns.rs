//! The records of `data.cols` (docs/FORMAT.md §4): each one's fields, kind and name, its
//! `by_sample`, and the forms of its facts.

use super::samples::RESERVED;
use super::{checksum, count, exactly, facts, field, hex64, list, nonempty, text, MEND};
use crate::numeric::width;
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::HashSet;

/// Whether a kind is one a flat column, or a jagged column's content, may have.
pub(super) fn flat(kind: &str) -> bool {
    kind != "jagged" && width(kind).is_ok()
}

/// What a kind that is none of the format's is told.
pub(super) fn unknown_kind(place: &str, kind: &str) -> crate::Error {
    invalid(format!("{place}.kind is {kind:?}, and a kind is one of f8, f4, i8, i4, i2, i1, u8, u4, u2, u1, b1 (or jagged, for a column) (docs/FORMAT.md §4): {MEND}"))
}

/// Check a record's `by_sample`: one SHA-256 per sample, as 64 lowercase hexadecimal characters.
pub(super) fn by_sample(record: &Value, place: &str, samples: usize) -> Result<()> {
    let sums = list(record, place, "by_sample")?;
    if sums.len() != samples || !sums.iter().all(|sum| sum.as_str().is_some_and(hex64)) {
        return Err(invalid(format!("{place}.by_sample must hold one SHA-256 per sample of data.samples ({samples}), each 64 lowercase hexadecimal characters (docs/FORMAT.md §8): {MEND}")));
    }
    Ok(())
}

/// Check every column record, and that they are the columns `made.recipe.columns` names, in order.
pub(super) fn records(data: &Value, made: &Value) -> Result<()> {
    let samples = list(data, "data", "samples")?.len();
    by_sample(&data["entry"], "data.entry", samples)?;
    let columns = list(data, "data", "cols")?;
    let mut names = HashSet::new();
    for (index, record) in columns.iter().enumerate() {
        let name = nonempty(record, &format!("data.cols[{index}]"), "name")?;
        let place = format!("data.cols[{name:?}]");
        if RESERVED.contains(&name) {
            return Err(invalid(format!("{place}: no column is named sample, entry or file, names the format gives (docs/FORMAT.md §4): name it otherwise in the recipe, and press again")));
        }
        if !names.insert(name) {
            return Err(invalid(format!("{place}: two columns have this name, and each has its own (docs/FORMAT.md §4): {MEND}")));
        }
        record_of(record, &place, &["name"], true, samples)?;
    }
    super::snapshot::columns(made, columns)
}

/// Check a column's record after `head` (its name, and a derived column's expression): its kind,
/// and for a flat column its checksums and facts, for a jagged one its offsets' and content's, each
/// array's `at` and `size` too when the file `stores` it.
pub(super) fn record_of(record: &Value, place: &str, head: &[&str], stores: bool, samples: usize) -> Result<()> {
    let array: &[&str] = if stores { &["at", "size", "sha256", "by_sample"] } else { &["sha256", "by_sample"] };
    let fields = |rest: &[&'static str]| head.iter().chain(rest).copied().collect::<Vec<&str>>();
    match text(record, place, "kind")? {
        "jagged" => {
            exactly(record, place, &fields(&["kind", "offsets", "content"]), &[])?;
            let (offsets, content) = (format!("{place}.offsets"), format!("{place}.content"));
            exactly(field(record, place, "offsets")?, &offsets, array, &[])?;
            exactly(field(record, place, "content")?, &content, &[&["kind", "count"][..], array].concat(), &[])?;
            let kind = text(&record["content"], &content, "kind")?;
            if !flat(kind) {
                return Err(unknown_kind(&content, kind));
            }
            count(&record["content"], &content, "count")?;
            for (part, place) in [(&record["offsets"], &offsets), (&record["content"], &content)] {
                checksum(part, place)?;
                by_sample(part, place, samples)?;
            }
        }
        kind if flat(kind) => {
            exactly(record, place, &fields(&[&["kind"][..], array, &["lim", "nice", "vals"]].concat()), &["vals"])?;
            checksum(record, place)?;
            by_sample(record, place, samples)?;
            facts::forms(record, kind, place)?;
        }
        kind => return Err(unknown_kind(place, kind)),
    }
    Ok(())
}
