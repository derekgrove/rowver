use crate::numeric::NumericArray;
use crate::table::Source;
use crate::write::data_checksum;
use crate::{invalid, Column, Result, Table, VERSION};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

mod arrays;
mod cancellation;
pub mod derived;
mod facts;
mod identity;
mod reader;
pub(crate) mod samples;
mod snapshot;

use arrays::Arrays;
use cancellation::{CancelReader, Cancellation};
pub use reader::{Held, Reader};

const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

/// The kinds of hole a file records (docs/FORMAT.md, "Holes"); a reader refuses any other.
pub const HOLE_KINDS: [&str; 6] = ["tape", "unreachable", "damaged", "gone", "changed", "refused"];

/// Entries `[first, last)` of one file's tree that a press chose but could not read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hole {
    pub sample: String,
    pub file: String,
    pub first: u64,
    pub last: u64,
    pub kind: String,
    pub why: String,
}

/// How many entries the event choice chose in each of `holes(header)`.
pub fn chosen_in_holes(header: &Value) -> Result<Vec<u64>> {
    identity::chosen_in_holes(object(header, "data")?, object(header, "made")?)
}

/// Every hole a checked header records, in the order of `data.files`, then of entries.
pub fn holes(header: &Value) -> Vec<Hole> {
    let files = header["data"]["files"].as_array().into_iter().flatten();
    files
        .flat_map(|file| {
            let (sample, at) = (file["sample"].as_str().unwrap_or(""), file["path"].as_str().unwrap_or(""));
            let record = &header["made"]["files"][sample]["files"][at];
            record["holes"].as_array().into_iter().flatten().map(move |hole| Hole {
                sample: sample.into(),
                file: at.into(),
                first: hole["entries"][0].as_u64().unwrap_or(0),
                last: hole["entries"][1].as_u64().unwrap_or(0),
                kind: hole["kind"].as_str().unwrap_or("").into(),
                why: hole["why"].as_str().unwrap_or("").into(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointSummary {
    pub rows: usize,
    pub columns: usize,
    /// How many derived columns it has, each worked out and checked unless the file is empty.
    pub derived: usize,
    /// Whether the file is empty: its header alone, every fact of which is checked.
    pub empty: bool,
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
        return Err(invalid(NOT_RVR));
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
    let header = checked_header(&text)?;
    let empty = size == start;
    if !empty {
        read_body(&header, Arrays::new(file, size - start, cancellation), drop)?;
    }
    cancellation.check()?;
    // Only a file with derived columns is opened again, to work them out.
    let derived = match derived::agreed(&header)? {
        0 => 0,
        n if empty => n,
        _ => derived::checked_while(&Reader::open(path)?, &|| cancellation.check())?,
    };
    Ok(CheckpointSummary {
        rows: count(object(&header, "data")?, "rows")?,
        columns: list(object(&header, "data")?, "cols")?.len(),
        derived,
        empty,
        header,
    })
}

/// Read the bytes of a checkpoint's JSON header, without decoding or checking any array.
fn header_text(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let size = usize::try_from(file.metadata()?.len())
        .map_err(|_| invalid("Checkpoint length exceeds the current platform"))?;
    let mut prefix = [0; 12];
    file.read_exact(&mut prefix).map_err(|_| invalid(NOT_RVR))?;
    let mut text = vec![0; header_end(&prefix, size)? - 12];
    file.read_exact(&mut text)?;
    Ok(text)
}

/// Identify a checkpoint by the SHA-256 of its header, which names every array's checksum.
pub fn header_digest(path: impl AsRef<Path>) -> Result<String> {
    Ok(digest(&header_text(path)?))
}

/// Read only a file's header, as it is, without checking it or any array.
pub fn read_header(path: impl AsRef<Path>) -> Result<Value> {
    let header: Value = serde_json::from_slice(&header_text(path)?)?;
    version(&header)?;
    Ok(header)
}

/// What a reader of an empty file's columns is told.
const EMPTY: &str = "the file is empty: rvr empty let its columns go, and rvr refill presses them again from the study it carries";

/// What a file that is not an `.rvr` file is told.
const NOT_RVR: &str = "Input is not an .rvr file (its first bytes are not the .rvr magic)";

/// Refuse a header of another version, saying what to do.
fn version(header: &Value) -> Result<()> {
    match header.get("rowver").and_then(Value::as_u64) {
        Some(VERSION) => Ok(()),
        Some(other) => Err(invalid(format!(
            "This file is rowver {other}; this reader reads rowver {VERSION} only. Press its study again with rvr press."
        ))),
        None => Err(invalid("The header has no rowver version")),
    }
}

/// Parse a header's text and check every fact it holds that needs no array: its version, its
/// `made` (the recipe, samples and lock texts and their checksums), the sample records, the
/// sample columns, `data.sha256`, the derived columns' records, and the signature, when it has one.
pub fn checked_header(text: &[u8]) -> Result<Value> {
    let header: Value = serde_json::from_slice(text)?;
    version(&header)?;
    let root = header
        .as_object()
        .ok_or_else(|| invalid("Header must be an object"))?;
    for (key, value) in root {
        if key != "rowver" && !value.is_object() {
            return Err(invalid(format!("Header section {key} must be an object")));
        }
    }
    let data = object(&header, "data")?;
    let made = object(&header, "made")?;
    snapshot::validate(made)?;
    snapshot::columns(made, list(data, "cols")?)?;
    samples::columns(data, made)?;
    if let Some(derived) = header.get("derived") {
        derived::validate(derived, data)?;
    }
    identity::records(data, made)?;
    if data_checksum(data)? != checksum(data)? {
        return Err(invalid(
            "data.sha256 does not match the checksums of the arrays its records name",
        ));
    }
    crate::sign::verify(&header)?;
    Ok(header)
}

/// Read a file's stored columns and check them, as `from_bytes` does.
pub fn load(path: impl AsRef<Path>) -> Result<Arc<Table>> {
    from_bytes(&std::fs::read(path)?)
}

/// Decode a file's stored columns and check them; `validate_file` works its derived columns out too.
pub fn from_bytes(bytes: &[u8]) -> Result<Arc<Table>> {
    decode(bytes)
}

/// Decode and validate one complete container.
fn decode(bytes: &[u8]) -> Result<Arc<Table>> {
    let start = header_end(bytes, bytes.len())?;
    let header = checked_header(&bytes[12..start])?;
    let mut columns = Vec::new();
    let (sources, entry) = read_body(
        &header,
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
pub(crate) fn header_end(prefix: &[u8], size: usize) -> Result<usize> {
    if prefix.len() < 12 || &prefix[..8] != MAGIC {
        return Err(invalid(NOT_RVR));
    }
    let length = u32::from_le_bytes(prefix[8..12].try_into().unwrap()) as usize;
    12_usize
        .checked_add(length)
        .filter(|end| *end <= size)
        .ok_or_else(|| invalid("JSON header length exceeds the file"))
}

/// Validate every array of a checked header through the same decoder, retaining columns only
/// for a table load.
fn read_body(
    header: &Value,
    mut decoder: Arrays<'_, impl Read>,
    mut keep: impl FnMut(Column),
) -> Result<(Vec<Source>, NumericArray)> {
    let cancellation = decoder.cancellation;
    cancellation.check()?;
    let data = object(header, "data")?;
    let made = object(header, "made")?;
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

/// Encode bytes as lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
