use crate::numeric::NumericArray;
use crate::table::Source;
use crate::{invalid, Column, Result, Table};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

mod arrays;
mod cancellation;
mod derived;
mod facts;
mod identity;
mod reader;
pub(crate) mod samples;
mod snapshot;
mod stack;

use arrays::Arrays;
use cancellation::{CancelReader, Cancellation};
pub use derived::confirm_parent;
pub use reader::{column_checksum, Held, Reader};
pub use stack::{ancestry, Stack};

const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointSummary {
    pub rows: usize,
    pub columns: usize,
    pub header: Value,
}

/// Validate a checkpoint one column at a time without retaining its event table.
pub fn validate_file(path: impl AsRef<Path>) -> Result<CheckpointSummary> {
    validate_file_cancellable(path, || false)
}

/// Validate a checkpoint while allowing its owning build job to cancel the scan.
pub fn validate_file_cancellable(
    path: impl AsRef<Path>,
    cancelled: impl Fn() -> bool,
) -> Result<CheckpointSummary> {
    let cancellation = Cancellation(&cancelled);
    let result = validate_stream(path.as_ref(), cancellation);
    cancellation.check()?;
    result
}

/// Read the shared header and stream columns without retaining the event table.
fn validate_stream(path: &Path, cancellation: Cancellation<'_>) -> Result<CheckpointSummary> {
    cancellation.check()?;
    let file = std::fs::File::open(path)?;
    let size = usize::try_from(file.metadata()?.len())
        .map_err(|_| invalid("Checkpoint length exceeds the current platform"))?;
    if size < 12 {
        return Err(invalid("Input is not a RowVer v13 .rvr container"));
    }
    let mut file = CancelReader {
        reader: file,
        cancellation,
    };
    let mut prefix = [0; 12];
    file.read_exact(&mut prefix)?;
    let start = header_end(&prefix, size)?;
    let mut text = vec![0; start - 12];
    file.read_exact(&mut text)?;
    let header: Value = serde_json::from_slice(&text)?;
    let order = derived::column_order(&text)?;
    read_body(
        &header,
        order.as_deref(),
        Arrays::new(file, size - start, cancellation),
        drop,
    )?;
    Ok(CheckpointSummary {
        rows: count(object(&header, "data")?, "rows")?,
        columns: list(object(&header, "data")?, "cols")?.len(),
        header,
    })
}

/// Read the bytes of a checkpoint's JSON header, without decoding or checking any array.
fn header_text(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let size = usize::try_from(file.metadata()?.len())
        .map_err(|_| invalid("Checkpoint length exceeds the current platform"))?;
    let mut prefix = [0; 12];
    file.read_exact(&mut prefix)
        .map_err(|_| invalid("Input is not a RowVer v13 .rvr container"))?;
    let mut text = vec![0; header_end(&prefix, size)? - 12];
    file.read_exact(&mut text)?;
    Ok(text)
}

/// Identify a checkpoint by the SHA-256 of its header, which names every array's checksum.
pub fn header_digest(path: impl AsRef<Path>) -> Result<String> {
    Ok(digest(&header_text(path)?))
}

/// Read only a checkpoint's header, without decoding or checking any array.
pub fn read_header(path: impl AsRef<Path>) -> Result<Value> {
    let text = header_text(path)?;
    let header: Value = serde_json::from_slice(&text)?;
    if header.get("rowver").and_then(Value::as_u64) != Some(13) {
        return Err(invalid("This reader accepts only RowVer version 13"));
    }
    Ok(header)
}

/// Read an existing checkpoint without opening its source ROOT files.
pub fn load(path: impl AsRef<Path>) -> Result<Arc<Table>> {
    from_bytes(&std::fs::read(path)?)
}

/// Decode and validate a complete pressed RowVer v13 container.
pub fn from_bytes(bytes: &[u8]) -> Result<Arc<Table>> {
    let table = decode(bytes)?;
    if table.header.get("derived").is_some() {
        return Err(invalid(
            "This file holds only derived columns; open it together with its parent",
        ));
    }
    Ok(table)
}

/// Load a derived file's own columns after confirming the parent on disk is the one it names.
pub fn load_derived(path: impl AsRef<Path>, parent: impl AsRef<Path>) -> Result<Arc<Table>> {
    let table = decode(&std::fs::read(path)?)?;
    confirm_parent(&table.header, &validate_file(parent)?.header)?;
    Ok(table)
}

/// Decode and validate one complete container, pressed or derived.
fn decode(bytes: &[u8]) -> Result<Arc<Table>> {
    let start = header_end(bytes, bytes.len())?;
    let header: Value = serde_json::from_slice(&bytes[12..start])?;
    let order = derived::column_order(&bytes[12..start])?;
    let mut columns = Vec::new();
    let (sources, entry) = read_body(
        &header,
        order.as_deref(),
        Arrays::new(
            &bytes[start..],
            bytes.len() - start,
            Cancellation(&|| false),
        ),
        |column| columns.push(column),
    )?;
    Ok(Arc::new(Table {
        header,
        columns,
        sources,
        entries: entry,
    }))
}

/// Validate the shared magic and JSON framing before allocating header storage.
fn header_end(prefix: &[u8], size: usize) -> Result<usize> {
    if prefix.len() < 12 || &prefix[..8] != MAGIC {
        return Err(invalid("Input is not a RowVer v13 .rvr container"));
    }
    let length = u32::from_le_bytes(prefix[8..12].try_into().unwrap()) as usize;
    12_usize
        .checked_add(length)
        .filter(|end| *end <= size)
        .ok_or_else(|| invalid("JSON header length exceeds the file"))
}

/// Validate every fact through the same decoder, retaining columns only for a table load.
fn read_body(
    header: &Value,
    order: Option<&[String]>,
    mut decoder: Arrays<'_, impl Read>,
    mut keep: impl FnMut(Column),
) -> Result<(Vec<Source>, NumericArray)> {
    let cancellation = decoder.cancellation;
    cancellation.check()?;
    if header.get("rowver").and_then(Value::as_u64) != Some(13) {
        return Err(invalid("This reader accepts only RowVer version 13"));
    }
    let root = header
        .as_object()
        .ok_or_else(|| invalid("Header must be an object"))?;
    for (key, value) in root {
        if key != "rowver" && !value.is_object() {
            return Err(invalid(format!("Header section {key} must be an object")));
        }
    }
    let data = object(header, "data")?;
    let made = object(header, "made")?;
    snapshot::validate(made)?;
    match header.get("derived") {
        Some(lineage) => derived::validate(lineage, data, made, order)?,
        None => snapshot::columns(made, list(data, "cols")?)?,
    }
    samples::columns(header, data, made)?;
    let rows = count(data, "rows")?;
    let sample = decoder.read(required(data, "sample")?, "i4", rows, "sample")?;
    let entry = decoder.read(required(data, "entry")?, "i8", rows, "entry")?;
    let sources = identity::validate(data, made, &sample, &entry, rows, cancellation)?;
    drop(sample);
    let spans = samples::spans(data, rows)?;
    samples::per_row(
        required(data, "entry")?,
        &entry,
        &spans,
        "entry",
        cancellation,
    )?;
    let mut names = HashSet::new();
    for record in list(data, "cols")? {
        cancellation.check()?;
        let name = string(record, "name")?;
        if name.is_empty() || !names.insert(name.to_owned()) {
            return Err(invalid("Column names must be nonempty and unique"));
        }
        let kind = string(record, "kind")?;
        let (array, offsets) = if kind == "jagged" {
            let n = rows
                .checked_add(1)
                .ok_or_else(|| invalid("Too many jagged rows"))?;
            let offsets = decoder.read(
                required(record, "offsets")?,
                "i8",
                n,
                &format!("{name} offsets"),
            )?;
            let content = object(record, "content")?;
            let nvalues = count(content, "count")?;
            let array = decoder.read(
                content,
                string(content, "kind")?,
                nvalues,
                &format!("{name} content"),
            )?;
            let offsets = jagged_offsets(&offsets, nvalues, name, cancellation)?;
            samples::jagged(record, &offsets, &array, &spans, name, cancellation)?;
            (array, Some(offsets))
        } else {
            let array = decoder.read(record, kind, rows, name)?;
            facts::validate(record, &array, cancellation)?;
            samples::per_row(record, &array, &spans, name, cancellation)?;
            (array, None)
        };
        keep(Column {
            name: name.to_owned(),
            data: array,
            offsets,
        });
    }
    decoder.finish(data)?;
    Ok((sources, entry))
}

/// Require ordered offsets covering exactly the declared content.
fn jagged_offsets(
    array: &NumericArray,
    nvalues: usize,
    name: &str,
    cancellation: Cancellation<'_>,
) -> Result<Vec<usize>> {
    let mut offsets = Vec::with_capacity(array.count);
    for index in 0..array.count {
        cancellation.at(index)?;
        let offset = usize::try_from(array.signed(index)?)
            .map_err(|_| invalid(format!("{name}: negative jagged offset")))?;
        if offset > nvalues || offsets.last().is_some_and(|previous| *previous > offset) {
            return Err(invalid(format!(
                "{name}: jagged offsets decrease or exceed content"
            )));
        }
        offsets.push(offset);
    }
    if offsets.first() != Some(&0) || offsets.last() != Some(&nvalues) {
        return Err(invalid(format!(
            "{name}: offsets must start at zero and end at content count"
        )));
    }
    Ok(offsets)
}

/// Obtain a required object member with a readable schema error.
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .get(key)
        .ok_or_else(|| invalid(format!("Header is missing {key}")))
}

/// Obtain a required nested object.
fn object<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    let member = required(value, key)?;
    if !member.is_object() {
        return Err(invalid(format!("{key} must be an object")));
    }
    Ok(member)
}

/// Obtain a required list.
fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    required(value, key)?
        .as_array()
        .ok_or_else(|| invalid(format!("{key} must be an array")))
}

/// Obtain a required text field.
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    required(value, key)?
        .as_str()
        .ok_or_else(|| invalid(format!("{key} must be text")))
}

/// Obtain a nonnegative size representable on the current platform.
fn count(value: &Value, key: &str) -> Result<usize> {
    required(value, key)?
        .as_u64()
        .and_then(|count| usize::try_from(count).ok())
        .ok_or_else(|| {
            invalid(format!(
                "{key} must be a nonnegative platform-sized integer"
            ))
        })
}

/// Require a lowercase 64-character SHA-256 digest.
fn checksum(value: &Value) -> Result<&str> {
    let hash = string(value, "sha256")?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "sha256 must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(hash)
}

/// Hash the unchanged byte representation.
pub(crate) fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Encode a digest without adding another dependency.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
