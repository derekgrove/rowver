//! Read and check `.rvr` files: a header on opening, the whole file in full validation.

use crate::numeric::NumericArray;
use crate::write::data_checksum;
use crate::{invalid, Result, VERSION};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

mod arrays;
mod columns;
pub mod derived;
mod facts;
mod identity;
mod reader;
pub(crate) mod samples;
mod snapshot;
mod texts;

use arrays::Arrays;
pub use crate::numeric::{push_shown, width};
pub use reader::{Held, Reader};
pub use snapshot::{study_name, toolchain};
pub use texts::{derived_kinds, kinds};

const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

/// The tables every recipe writes, even empty, in their order.
pub const RECIPE_TABLES: [&str; 5] = ["define", "keep", "columns", "totals", "derived"];

/// The kinds of hole a file records (docs/FORMAT.md, "Holes"); a reader refuses any other.
pub const HOLE_KINDS: [&str; 5] = ["unreachable", "damaged", "gone", "changed", "refused"];

/// What every refusal of a file's own facts says to do.
const MEND: &str = "press its study again with rvr press, which writes it as docs/FORMAT.md says";

/// Entries `[first, last)` of one file's tree that a press chose but could not read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hole {
    /// The sample the file is of.
    pub sample: String,
    /// The file, as `data.files` names it.
    pub file: String,
    /// The first entry of the hole.
    pub first: u64,
    /// The entry after its last.
    pub last: u64,
    /// One of `HOLE_KINDS`.
    pub kind: String,
    /// What failed, for a person.
    pub why: String,
}

/// Whether a press read a file of `chosen` entries, `in_holes` of them lost, and so it holds the totals (FORMAT.md §6).
pub fn read_file(chosen: u64, in_holes: u64, random: bool) -> bool {
    // Choosing none, all or the first N opens a file of no entries; a random choice opens none.
    chosen > in_holes || (chosen == 0 && !random)
}

/// How many entries the event choice chose in each of `holes(header)`.
pub fn chosen_in_holes(header: &Value) -> Result<Vec<u64>> {
    identity::chosen_in_holes(header)
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
                first: whole_count(&hole["entries"][0]).unwrap_or(0),
                last: whole_count(&hole["entries"][1]).unwrap_or(0),
                kind: hole["kind"].as_str().unwrap_or("").into(),
                why: hole["why"].as_str().unwrap_or("").into(),
            })
        })
        .collect()
}

/// What full validation found of a valid file.
#[derive(Debug, Clone, Serialize)]
pub struct Validated {
    /// How many rows it has.
    pub rows: usize,
    /// How many stored columns it has.
    pub columns: usize,
    /// How many derived columns it has, each worked out and checked (over no rows when empty).
    pub derived: usize,
    /// Whether the file is empty: its header alone, every fact of which is checked.
    pub empty: bool,
    /// Its header.
    pub header: Value,
}

/// Validate a file in full (docs/FORMAT.md §12), one column at a time, its derived columns worked out.
pub fn validate_file(path: impl AsRef<Path>) -> Result<Validated> {
    let path = path.as_ref();
    let (mut file, size) = opened(path)?;
    if size < 12 {
        return Err(invalid(NOT_RVR));
    }
    let mut prefix = [0; 12];
    file.read_exact(&mut prefix)?;
    let start = header_end(&prefix, size)?;
    let mut text = vec![0; start - 12];
    file.read_exact(&mut text)?;
    let (header, end) = checked(&text)?;
    let empty = size == start;
    arrays::fill(end, size - start)?;
    let reads = identity::choices(&header)?;
    texts::validate(&header, &reads)?;
    // The file is read for its derived columns through the handle it was checked through.
    let handle = file.try_clone()?;
    if !empty {
        read_body(&header, Arrays::new(file, size - start), &reads)?;
    }
    let derived = match listed_derived(&header) {
        0 => 0,
        _ => {
            let file = Reader::of(path, handle, header.clone(), start as u64, (size - start) as u64)?;
            match empty {
                true => derived::checked_empty(&file)?,
                false => derived::checked(&file)?,
            }
        }
    };
    let data = &header["data"];
    Ok(Validated {
        rows: count(data, "data", "rows")?,
        columns: list(data, "data", "cols")?.len(),
        derived,
        empty,
        header,
    })
}

/// The file at `path` opened to read, and its length; a folder is refused as `IsADirectory`.
pub(crate) fn opened(path: &Path) -> Result<(std::fs::File, usize)> {
    let file = std::fs::File::open(path)?;
    let held = file.metadata()?;
    if held.is_dir() {
        return Err(std::io::Error::from(std::io::ErrorKind::IsADirectory).into());
    }
    let size = usize::try_from(held.len()).map_err(|_| invalid("the file is longer than this computer can address"))?;
    Ok((file, size))
}

/// How many derived columns a checked header records.
fn listed_derived(header: &Value) -> usize {
    header["derived"]["columns"].as_array().map_or(0, Vec::len)
}

/// A file's header as the file holds it: the JSON text between its length and its arrays, unchecked.
pub fn header_text(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let (mut file, size) = opened(path.as_ref())?;
    let mut prefix = [0; 12];
    file.read_exact(&mut prefix).map_err(|_| invalid(NOT_RVR))?;
    let mut text = vec![0; header_end(&prefix, size)? - 12];
    file.read_exact(&mut text)?;
    Ok(text)
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
        None => Err(invalid(format!("rowver must be the whole number {VERSION}, the format version: {MEND}"))),
    }
}

/// The top level's own keys; any other is a tool's.
const TOP: [&str; 5] = ["rowver", "data", "made", "derived", "signature"];

/// A whole number with its thousands apart by commas: `50,000`.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Entries `[first, last)` in words, as every tool here writes a run: `entries 50,000 to 99,999`, or `entry 7`.
pub fn span(first: u64, last: u64) -> String {
    match last.saturating_sub(first) {
        1 => format!("entry {}", grouped(first)),
        _ => format!("entries {} to {}", grouped(first), grouped(last.saturating_sub(1))),
    }
}

/// The tool keys of a header (docs/FORMAT.md §11): every top-level key that is not the format's.
pub fn tool_keys(header: &Value) -> Map<String, Value> {
    let root = header.as_object().into_iter().flatten();
    root.filter(|(key, _)| !TOP.contains(&key.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect()
}

/// Whether `name` is a tool key's name: one or more ASCII letters, digits, `_`, `-` and `.`.
fn tool_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Parse a header's text and check every fact it holds that needs no array and no parse of the
/// study's texts (docs/FORMAT.md §12, "On opening"), but that the arrays fill the file, which the
/// caller checks with `arrays::fill`.
pub fn checked_header(text: &[u8]) -> Result<Value> {
    checked(text).map(|(header, _)| header)
}

/// `checked_header`, and where the arrays its records lay out end.
fn checked(text: &[u8]) -> Result<(Value, u64)> {
    let header = crate::json::strict(text)?;
    let end = checks(&header)?;
    Ok((header, end))
}

/// Every check `checked_header` makes, of a header parsed as `json::strict` parses it; where
/// the arrays its records lay out end.
fn checks(header: &Value) -> Result<u64> {
    let root = header.as_object().ok_or_else(|| invalid(format!("the header must be one JSON object: {MEND}")))?;
    version(header)?;
    for (key, value) in root {
        if !TOP.contains(&key.as_str()) && !tool_name(key) {
            return Err(invalid(format!("the top-level key {key:?} is no field of the format and not a tool key's name (one or more ASCII letters, digits, _, - and ., docs/FORMAT.md §11): rename the tool key")));
        }
        if key != "rowver" && !value.is_object() {
            return Err(invalid(format!("{key} must be an object (docs/FORMAT.md §2, §11): {MEND}")));
        }
    }
    let data = object(header, "", "data")?;
    let made = object(header, "", "made")?;
    exactly(data, "data", &["rows", "samples", "files", "sample", "entry", "cols", "sha256", "sample_cols"], &[])?;
    snapshot::validate(made)?;
    identity::records(data, made)?;
    columns::records(data, made)?;
    let end = arrays::layout(data)?;
    samples::columns(data, made)?;
    if let Some(derived) = header.get("derived") {
        derived::validate(derived, data)?;
    }
    if data_checksum(data)? != checksum(data, "data")? {
        return Err(invalid(format!("data.sha256 is not the SHA-256 of the arrays' sha256s joined in file order (docs/FORMAT.md §8): {MEND}")));
    }
    crate::sign::verify(header)?;
    Ok(end)
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
        .ok_or_else(|| invalid("the header's length (bytes 8 to 11) is more than the file holds: the file was cut short; copy it again"))
}

/// Check every array of a checked header as it streams past, one at a time.
fn read_body(header: &Value, mut decoder: Arrays<impl Read>, reads: &identity::Reads) -> Result<()> {
    let data = &header["data"];
    let rows = count(data, "data", "rows")?;
    let sample = decoder.read(&data["sample"], "i4", rows, "data.sample")?;
    let entry = decoder.read(&data["entry"], "i8", rows, "data.entry")?;
    identity::rows(header, &sample, &entry, reads)?;
    drop(sample);
    let spans = samples::spans(data)?;
    samples::per_row(&data["entry"], &entry, &spans, "data.entry")?;
    drop(entry);
    for record in list(data, "data", "cols")? {
        let name = text(record, "data.cols", "name")?;
        let place = format!("data.cols[{name:?}]");
        let kind = text(record, &place, "kind")?;
        if kind == "jagged" {
            let offsets = decoder.read(&record["offsets"], "i8", rows + 1, &format!("{place}.offsets"))?;
            let content = &record["content"];
            let count = count(content, &place, "count")?;
            let array = decoder.read(content, text(content, &place, "kind")?, count, &format!("{place}.content"))?;
            let offsets = jagged_offsets(&offsets, count, &place)?;
            samples::jagged(record, &offsets, &array, &spans, &place)?;
        } else {
            let array = decoder.read(record, kind, rows, &place)?;
            facts::values(record, &array, &place)?;
            samples::per_row(record, &array, &spans, &place)?;
        }
    }
    decoder.finish(data)
}

/// Require offsets that start at 0, never decrease, and end at the content's count.
fn jagged_offsets(array: &NumericArray, count: usize, place: &str) -> Result<Vec<usize>> {
    let mut offsets = Vec::with_capacity(array.count);
    for index in 0..array.count {
        let offset = usize::try_from(array.signed(index)?).ok().filter(|offset| *offset <= count && offsets.last().is_none_or(|before| before <= offset));
        offsets.push(offset.ok_or_else(|| {
            invalid(format!("{place}.offsets decrease, or pass the content's count, at row {index} (docs/FORMAT.md §4): {MEND}"))
        })?);
    }
    if offsets.first() != Some(&0) || offsets.last() != Some(&count) {
        return Err(invalid(format!("{place}.offsets must start at 0 and end at content.count, {count} (docs/FORMAT.md §4): {MEND}")));
    }
    Ok(offsets)
}

/// A record's member `key`, refused when missing; `place` is where the record is.
fn field<'a>(value: &'a Value, place: &str, key: &str) -> Result<&'a Value> {
    value.get(key).ok_or_else(|| invalid(format!("{} is missing (docs/FORMAT.md): {MEND}", at(place, key))))
}

/// `place.key`, or `key` at the top level.
fn at(place: &str, key: &str) -> String {
    match place {
        "" => key.into(),
        _ => format!("{place}.{key}"),
    }
}

/// A member that must be an object.
fn object<'a>(value: &'a Value, place: &str, key: &str) -> Result<&'a Value> {
    let member = field(value, place, key)?;
    match member.is_object() {
        true => Ok(member),
        false => Err(invalid(format!("{} must be an object: {MEND}", at(place, key)))),
    }
}

/// A member that must be a list.
fn list<'a>(value: &'a Value, place: &str, key: &str) -> Result<&'a Vec<Value>> {
    field(value, place, key)?
        .as_array()
        .ok_or_else(|| invalid(format!("{} must be a list: {MEND}", at(place, key))))
}

/// A member that must be text.
fn text<'a>(value: &'a Value, place: &str, key: &str) -> Result<&'a str> {
    field(value, place, key)?
        .as_str()
        .ok_or_else(|| invalid(format!("{} must be text: {MEND}", at(place, key))))
}

/// A member that must be nonempty text.
fn nonempty<'a>(value: &'a Value, place: &str, key: &str) -> Result<&'a str> {
    match text(value, place, key)? {
        "" => Err(invalid(format!("{} must be nonempty text: {MEND}", at(place, key)))),
        held => Ok(held),
    }
}

/// A count, as every count of the format is: a whole number written without a fraction or an
/// exponent, from 0 to 2^63 − 1.
fn whole_count(value: &Value) -> Option<u64> {
    let text = value.as_number()?.to_string();
    let n: i128 = text.parse().ok().filter(|_| !text.contains(['.', 'e', 'E']))?;
    u64::try_from(n).ok().filter(|n| *n <= i64::MAX as u64)
}

/// A member that must be a count.
fn count(value: &Value, place: &str, key: &str) -> Result<usize> {
    whole_count(field(value, place, key)?)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| invalid(format!("{} must be a whole number from 0 to 2^63 − 1, written without a fraction or an exponent (docs/FORMAT.md §1): {MEND}", at(place, key))))
}

/// Whether text is 64 lowercase hexadecimal characters.
fn hex64(text: &str) -> bool {
    crate::study::hex(text, 64)
}

/// A record's `sha256`: 64 lowercase hexadecimal characters.
fn checksum<'a>(value: &'a Value, place: &str) -> Result<&'a str> {
    let hash = text(value, place, "sha256")?;
    match hex64(hash) {
        true => Ok(hash),
        false => Err(invalid(format!("{} must be 64 lowercase hexadecimal characters (docs/FORMAT.md §8): {MEND}", at(place, "sha256")))),
    }
}

/// Require a record to hold exactly these fields, of which `optional` may be left out.
fn exactly(value: &Value, place: &str, keys: &[&str], optional: &[&str]) -> Result<()> {
    let held: Option<&Map<String, Value>> = value.as_object();
    let held = held.ok_or_else(|| invalid(format!("{place} must be an object: {MEND}")))?;
    if let Some(key) = held.keys().find(|key| !keys.contains(&key.as_str())) {
        return Err(invalid(format!(
            "{place} has a field {key:?}, which the format does not give it ({}): a newer rowver may write it (update rowver), or take it out",
            keys.join(", ")
        )));
    }
    match keys.iter().find(|key| !held.contains_key(**key) && !optional.contains(key)) {
        Some(key) => Err(invalid(format!("{} is missing (docs/FORMAT.md): {MEND}", at(place, key)))),
        None => Ok(()),
    }
}

/// Hash the unchanged byte representation.
pub(crate) fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Encode bytes as lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
