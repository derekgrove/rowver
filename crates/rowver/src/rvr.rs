//! Read and check `.rvr` files: a header on opening, the whole file in full validation.

use crate::write::data_checksum;
use crate::{invalid, Result, VERSION};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) mod arrays;
mod columns;
pub mod derived;
pub(crate) mod facts;
mod identity;
mod reader;
pub(crate) mod samples;
mod snapshot;
mod texts;

pub use crate::numeric::{push_shown, width};
pub use identity::chosen_in_holes;
pub use reader::{Held, Reader};
pub use snapshot::{study_name, toolchain};
pub use texts::{derived_kinds, kinds};

/// The eight bytes every `.rvr` file starts and ends with.
pub(crate) const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

/// The kinds of hole a file records (docs/FORMAT.md, "Holes"); a reader refuses any other.
pub const HOLE_KINDS: [&str; 5] = ["unreachable", "damaged", "gone", "changed", "refused"];

/// What every refusal of a file's own facts says to do.
pub(crate) const MEND: &str = "press its study again with rvr press, which writes it as docs/FORMAT.md says";

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
    let (file, size, text, seen) = head(path)?;
    let length = size - 20 - text.len();
    let (header, end) = checked(&text).and_then(|(header, end)| arrays::fill(end, length).map(|()| (header, end)))?;
    let empty = length == 0 && end > 0;
    let reads = identity::choices(&header)?;
    texts::validate(&header, &reads)?;
    // The arrays and derived columns are read through the handle the file was checked through.
    let file = Reader::of(path, file, seen, header.clone(), 8, length as u64);
    if !empty {
        read_body(&file, &reads)?;
    }
    let derived = match (listed_derived(&header), empty) {
        (0, _) => 0,
        (_, true) => derived::checked_empty(&file)?,
        (_, false) => derived::checked(&file)?,
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

/// The file at `path` opened to read, its length, and its length and time of change; a folder is
/// refused as `IsADirectory`.
pub(crate) fn opened(path: &Path) -> Result<(crate::change::Handle, usize, reader::Stamp)> {
    // Looked for before it is opened, so that a refusal leaves no descriptor behind.
    crate::change::unheld_at(path)?;
    let file = crate::change::Handle::new(crate::change::unheld(std::fs::File::open(path)?, path)?);
    if file.metadata()?.is_dir() {
        return Err(std::io::Error::from(std::io::ErrorKind::IsADirectory).into());
    }
    let seen = reader::stamp(&file)?;
    let size = usize::try_from(seen.0).map_err(|_| invalid(crate::change::LONG))?;
    Ok((file, size, seen))
}

/// How many derived columns a checked header records.
fn listed_derived(header: &Value) -> usize {
    header["derived"]["columns"].as_array().map_or(0, Vec::len)
}

/// A file's header as the file holds it: the JSON text between its arrays and its length, unchecked.
pub fn header_text(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    head(path.as_ref()).map(|(_, _, text, _)| text)
}

/// The file at `path` opened and its header read: the file, its length, the header's text, and the
/// file's length and time of change as it was opened.
pub(crate) fn head(path: &Path) -> Result<(crate::change::Handle, usize, Vec<u8>, reader::Stamp)> {
    let (file, size, seen) = opened(path)?;
    let text = headed(path, &file, size, &seen)?;
    Ok((file, size, text, seen))
}

/// The header's text of the file at `path`, opened as `file` (`size` bytes, `seen`), as `agreed` reads it.
fn headed(path: &Path, file: &std::fs::File, size: usize, seen: &reader::Stamp) -> Result<Vec<u8>> {
    agreed(path, file, seen, || framed_text(file, size))
}

/// The header's text of `file`, `size` bytes long, its frame checked.
fn framed_text(file: &std::fs::File, size: usize) -> Result<Vec<u8>> {
    let start = frame(file, size)?;
    let mut text = vec![0; size - 12 - start];
    crate::write::read_at(file, &mut text, start as u64)?;
    Ok(text)
}

/// What `read` reads of the file at `path` (opened as `file`, `seen`), taken when two reads in a
/// row agree, no journal beside it between them (docs/SPEC.md §8.12): a read torn by a change
/// cannot agree with one made after it, and while it is made its journal is there. Read three
/// times at most: the file changed while it was read when no two agree.
fn agreed<T: PartialEq>(path: &Path, file: &std::fs::File, seen: &reader::Stamp, mut read: impl FnMut() -> Result<T>) -> Result<T> {
    let mut was = read();
    for _ in 0..2 {
        settled(path, file, seen)?;
        let now = read();
        let same = match (&was, &now) {
            (Ok(was), Ok(now)) => was == now,
            (Err(was), Err(now)) => was.to_string() == now.to_string(),
            _ => false,
        };
        if same {
            return now;
        }
        was = now;
    }
    Err(reader::changed(path))
}

/// The file at `path` opened and its frame checked (docs/FORMAT.md §1), as `agreed` reads it: the
/// file, its length, and where its header starts.
pub(crate) fn framed(path: &Path) -> Result<(crate::change::Handle, usize, usize)> {
    let (file, size, seen) = opened(path)?;
    let start = agreed(path, &file, &seen, || frame(&file, size))?;
    Ok((file, size, start))
}

/// What a reader of the file at `path` is told while a writer changes it.
fn busy(path: &Path) -> crate::Error {
    invalid(format!("{} is being changed by another program now: wait for it to finish, then open it again", path.display()))
}

/// Refuse the file at `path`, opened as `file` (`seen`), when what was just read of it may not be
/// what it holds: while the journal of a change to it is beside it (§12), or when it changed since
/// it was opened. Looked for after the read, so that a reader that loses the race to a writer
/// says so, and not that a healthy file is damaged.
fn settled(path: &Path, file: &std::fs::File, seen: &reader::Stamp) -> Result<()> {
    let journal = crate::change::journal(&crate::change::resolved(path));
    if journal.exists() {
        // A writer that holds the file's lock is changing it now; else the change was cut short.
        return Err(match file.try_lock_shared() {
            Err(std::fs::TryLockError::WouldBlock) => busy(path),
            _ => invalid(format!("{}: a change to it was cut short ({} is beside it): any rvr command on the file puts it back as it was", path.display(), journal.display())),
        });
    }
    match reader::stamp(file).ok().as_ref() == Some(seen) {
        true => Ok(()),
        false => Err(reader::changed(path)),
    }
}

/// Check the frame of `file`, `size` bytes long (docs/FORMAT.md §1): where its header starts.
pub(crate) fn frame(file: &std::fs::File, size: usize) -> Result<usize> {
    if size < 20 {
        return Err(invalid(format!("it is not an .rvr file: it holds {size} bytes, and an .rvr file holds at least 20 (the magic, a header's length and the magic again): name an .rvr file")));
    }
    let (mut first, mut last) = ([0; 8], [0; 12]);
    crate::write::read_at(file, &mut first, 0)?;
    crate::write::read_at(file, &mut last, size as u64 - 12)?;
    if &first != MAGIC {
        return Err(invalid(NOT_RVR));
    }
    if &last[4..] != MAGIC {
        return Err(invalid(CUT_SHORT));
    }
    let length = u32::from_le_bytes([last[0], last[1], last[2], last[3]]) as usize;
    match (size - 20).checked_sub(length) {
        Some(arrays) => Ok(8 + arrays),
        None => Err(invalid("the header's length (the 4 bytes before the last 8) is more than the file holds: the file was cut short; copy it again")),
    }
}

/// What a file that does not end in the magic is told.
pub(crate) const CUT_SHORT: &str = "it does not end in the .rvr magic (its last 8 bytes): the file was cut short, or a change to it was (any rvr command on it puts back a change cut short); copy it again, or press its study again";

/// What a reader of an empty file's columns is told.
const EMPTY: &str = "the file is empty: rvr empty let its columns go, and rvr refill presses them again from the study it carries";

/// What a file that is not an `.rvr` file is told.
const NOT_RVR: &str = "it is not an .rvr file (its first 8 bytes are not the .rvr magic): name an .rvr file";

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

/// Each tool key a header's text holds, by name, and where its object's bytes lie there, unparsed (docs/FORMAT.md §11).
pub fn tool_texts(text: &[u8]) -> Result<Vec<(String, std::ops::Range<usize>)>> {
    let members = crate::json::members(text).ok_or_else(|| invalid("its header is not one JSON object (docs/FORMAT.md §1): the file is damaged; copy it again, or press its study again"))?;
    let named = members.into_iter().map(|m| (crate::json::name(&text[m.name]), m.value));
    Ok(named.filter(|(name, _)| !TOP.contains(&name.as_str())).collect())
}

/// The names of a header's tool keys (docs/FORMAT.md §11), in its order, nothing cloned.
pub fn tool_names(header: &Value) -> impl Iterator<Item = &str> {
    header.as_object().into_iter().flatten().map(|(key, _)| key.as_str()).filter(|key| !TOP.contains(key))
}

/// The tool keys of a header (docs/FORMAT.md §11): every top-level key that is not the format's.
pub fn tool_keys(header: &Value) -> Map<String, Value> {
    let root = header.as_object().into_iter().flatten();
    root.filter(|(key, _)| !TOP.contains(&key.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect()
}

/// The format's key that `key`, not one of them, is a slip of (docs/FORMAT.md §11): ignoring
/// case, at most one edit from it, a swap of two neighbours counting as one.
fn slip(key: &str) -> Option<&'static str> {
    let lower = key.to_lowercase();
    TOP.iter().find(|top| **top != key && rvr_recipe::slips(&lower, top) <= 1).copied()
}

/// Whether `name` is a tool key's name: one or more ASCII letters, digits, `_`, `-` and `.`.
fn tool_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Where the arrays a header lays out end, checked to fill the `length` bytes from byte 8 to the
/// header (or none: an empty file): what a change in place writes after.
pub(crate) fn placed(header: &Value, length: usize) -> Result<u64> {
    let end = arrays::layout(&header["data"])?;
    arrays::fill(end, length)?;
    Ok(end)
}

/// Where the arrays a header lays out end, after byte 8, and each one's `at` and `size`, in the
/// order of its records.
pub(crate) fn spans(header: &Value) -> Result<(u64, Vec<(u64, u64)>)> {
    let end = arrays::layout(&header["data"])?;
    let spans = arrays::records(&header["data"]).iter().map(|(record, _, _)| (record["at"].as_u64().unwrap_or(0), record["size"].as_u64().unwrap_or(0))).collect();
    Ok((end, spans))
}

/// Refuse `key` as a tool key's name, saying why, when it is not one (docs/FORMAT.md §11).
pub fn tool_key(key: &str) -> Result<()> {
    not_own(key)?;
    if let Some(near) = slip(key) {
        return Err(invalid(format!("the top-level key {key:?} is a slip of {near}, one of the format's own keys, not a tool key (docs/FORMAT.md §11): give the tool key a name further from it")));
    }
    if !tool_name(key) {
        return Err(invalid(format!("{key:?} is not a tool key's name, which is one or more ASCII letters, digits, _, - and . (docs/FORMAT.md §11): rename the tool key")));
    }
    Ok(())
}

/// Refuse `key`, saying why, when it is one of the format's own keys (docs/FORMAT.md §2, §11).
pub(crate) fn not_own(key: &str) -> Result<()> {
    match TOP.contains(&key) {
        true => Err(invalid(format!("{key} is the format's own key, not a tool key (docs/FORMAT.md §2, §11): give the tool key a name of its own"))),
        false => Ok(()),
    }
}

/// Parse a header's text and check every fact docs/FORMAT.md §12 checks "On opening" but that the arrays fill the file.
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
        if let Some(near) = slip(key) {
            return Err(invalid(format!("the top-level key {key:?} is a slip of {near}, one of the format's own keys, not a tool key (docs/FORMAT.md §11): write it {near}, or give the tool key a name further from it")));
        }
        if !TOP.contains(&key.as_str()) && !tool_name(key) {
            return Err(invalid(format!("the top-level key {key:?} is no field of the format and not a tool key's name (one or more ASCII letters, digits, _, - and ., docs/FORMAT.md §11): rename the tool key")));
        }
        if key != "rowver" && !value.is_object() {
            return Err(invalid(format!("{key} must be an object (docs/FORMAT.md §2, §11): {MEND}")));
        }
    }
    let data = object(header, "", "data")?;
    let made = object(header, "", "made")?;
    exactly(data, "data", &["rows", "chunk_rows", "samples", "files", "sample", "entry", "cols", "sha256", "sample_cols"], &[])?;
    let (fewest, most) = rvr_recipe::CHUNK_BOUNDS;
    if !(fewest..=most).contains(&count(data, "data", "chunk_rows")?) {
        return Err(invalid(format!("data.chunk_rows is {}, and a chunk holds from 1,024 to 16,777,216 rows (docs/FORMAT.md §3): {MEND}", data["chunk_rows"])));
    }
    snapshot::validate(made)?;
    identity::records(data, made)?;
    columns::records(data, made)?;
    let end = arrays::layout(data)?;
    samples::columns(data, made)?;
    if let Some(derived) = header.get("derived") {
        derived::validate(derived, data)?;
    }
    if data_checksum(data).map_err(|place| invalid(format!("{place} has no sha256: {MEND}")))? != checksum(data, "data")? {
        return Err(invalid(format!("data.sha256 is not the SHA-256 of the arrays' sha256s joined in record order (docs/FORMAT.md §8): {MEND}")));
    }
    crate::sign::verify(header)?;
    Ok(end)
}

/// Check every array of a checked file a chunk at a time (docs/FORMAT.md §12): each chunk against
/// its table, the sample index and entries against the samples and files, list offsets, `b1`
/// values, and every flat column's facts made again from its values; every array but the list
/// columns' content in one pool of threads, then the content, whose chunks' members the offsets give.
fn read_body(file: &Reader, reads: &identity::Reads) -> Result<()> {
    let header = file.header();
    let data = &header["data"];
    let rows = file.chunks()?;
    let per = Reader::rows_of(&rows);
    let more: Vec<usize> = per.iter().map(|n| n + 1).collect();
    let mut check = identity::Rows::new(header, reads)?;
    let mut first: Vec<reader::Whole> = vec![(&data["sample"], "i4", "data.sample".into(), (&per[..]).into()), (&data["entry"], "i8", "data.entry".into(), (&per[..]).into())];
    let mut columns = Vec::new();
    for record in list(data, "data", "cols")? {
        let name = text(record, "data.cols", "name")?;
        let place = format!("data.cols[{name:?}]");
        let kind = text(record, &place, "kind")?;
        match kind == "jagged" {
            true => first.push((&record["offsets"], "i8", format!("{place}.offsets"), (&more[..]).into())),
            false => first.push((record, kind, place.clone(), (&per[..]).into())),
        }
        columns.push((record, place, kind));
    }
    // Each flat column's facts made again, each list column's offsets put together (not kept).
    let mut made: Vec<std::result::Result<crate::write::facts::Facts, reader::Stitched>> = columns.iter().map(|(_, _, kind)| match *kind {
        "jagged" => Err(reader::Stitched::new(false)),
        kind => Ok(crate::write::facts::Facts::new(kind)),
    }).collect();
    file.each_chunks(&first, &mut |a, k, values, _| match a {
        0 => check.sample(rows[k].0, rows[k].1, &values),
        1 => check.entry(rows[k].1, &values),
        a => match &mut made[a - 2] {
            Ok(facts) => {
                truth(&values, columns[a - 2].2, &columns[a - 2].1)?;
                facts.add(&values)
            }
            Err(stitched) => stitched.add(&values, &first[a].2),
        },
    })?;
    let mut contents: Vec<reader::Whole> = Vec::new();
    for ((record, place, kind), made) in columns.iter().zip(made) {
        match made {
            Ok(facts) => facts::agree(record, &facts.finish(), kind, place)?,
            Err(stitched) => {
                let content = &record["content"];
                reader::ends(stitched.end, content, place)?;
                contents.push((content, text(content, place, "kind")?, format!("{place}.content"), stitched.members.into()));
            }
        }
    }
    file.each_chunks(&contents, &mut |a, _, values, _| truth(&values, contents[a].1, &contents[a].2))
}

/// Refuse a `b1` value that is not 0 or 1.
fn truth(values: &[u8], kind: &str, place: &str) -> Result<()> {
    match crate::numeric::untrue(kind, values) {
        Some(byte) => Err(invalid(format!("{place}: a b1 value is {byte}, and a b1 value is 0 or 1 (docs/FORMAT.md §4): {MEND}"))),
        None => Ok(()),
    }
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

/// A count, as every count of the format is: a whole number written as one, from 0 to 2^63 − 1.
fn whole_count(value: &Value) -> Option<u64> {
    crate::json::natural(value).filter(|n| *n <= i64::MAX as u64)
}

/// A member that must be a count.
fn count(value: &Value, place: &str, key: &str) -> Result<usize> {
    whole_count(field(value, place, key)?)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| invalid(format!("{} must be a whole number from 0 to 2^63 − 1, written without a fraction or an exponent (docs/FORMAT.md §1): {MEND}", at(place, key))))
}

/// A record's `sha256`: 64 lowercase hexadecimal characters.
fn checksum<'a>(value: &'a Value, place: &str) -> Result<&'a str> {
    let hash = text(value, place, "sha256")?;
    match crate::study::hex(hash, 64) {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// What `headed`'s reads of the file at `path` agree on, the file holding `versions[0]` when
    /// opened and each next one written over it after each read, its length and time kept.
    fn read_as_changed(path: &Path, versions: &[&[u8]]) -> crate::Result<Vec<u8>> {
        std::fs::write(path, versions[0]).unwrap();
        let time = std::fs::metadata(path).unwrap().modified().unwrap();
        let (file, size, seen) = super::opened(path).unwrap();
        let mut next = versions[1..].iter();
        super::agreed(path, &file, &seen, || {
            let read = super::framed_text(&file, size);
            if let Some(version) = next.next() {
                std::fs::write(path, version).unwrap();
                std::fs::File::options().write(true).open(path).unwrap().set_modified(time).unwrap();
                assert_eq!(super::reader::stamp(&file).unwrap(), seen, "the same length and time");
            }
            read
        })
    }

    // SPEC 1.5, 8.12; FORMAT 12
    #[test]
    fn a_change_of_the_header_alone_its_length_and_time_kept_made_between_two_reads_is_never_taken_for_the_file() {
        let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/rvr/valid/fixture.rvr")).unwrap();
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-agreed.rvr", std::process::id()));
        let start = fixture.len() - 12 - u32::from_le_bytes(fixture[fixture.len() - 12..fixture.len() - 8].try_into().unwrap()) as usize;
        let header = |bytes: &[u8]| bytes[start..bytes.len() - 12].to_vec();
        let (mut torn, mut cut) = (fixture.clone(), fixture.clone());
        torn[start] = b'x';
        let last = cut.len() - 1;
        cut[last] = b'x';
        // Read alone, each is what it is: the file, or damaged.
        assert_eq!(read_as_changed(&path, &[&fixture]).unwrap(), header(&fixture));
        assert_eq!(read_as_changed(&path, &[&torn]).unwrap(), header(&torn));
        assert!(read_as_changed(&path, &[&cut]).unwrap_err().to_string().contains("it does not end in the .rvr magic"));
        // A first read torn, or cut short, by a change made before the second is never the file's.
        assert_eq!(read_as_changed(&path, &[&torn, &fixture]).unwrap(), header(&fixture));
        assert_eq!(read_as_changed(&path, &[&cut, &fixture]).unwrap(), header(&fixture));
        assert_eq!(read_as_changed(&path, &[&fixture, &torn]).unwrap(), header(&torn));
        // A file changed after every read: no two reads in a row agree.
        for versions in [[&fixture[..], &torn, &fixture], [&torn, &fixture, &cut]] {
            let said = crate::plain(&path, &read_as_changed(&path, &versions).unwrap_err());
            assert!(said.contains("changed while it was read (emptied, or written again): open it again"), "{said}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 8.12; FORMAT 12
    #[test]
    fn a_reader_that_loses_the_race_to_a_writer_is_told_so() {
        let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/rvr/valid/fixture.rvr")).unwrap();
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-raced.rvr", std::process::id()));
        let journal = crate::change::journal(&path);
        std::fs::write(&path, &fixture).unwrap();
        // A reader opens the file and finds no journal; a writer then locks it, journals it and cuts
        // its tail short before the reader reads its header.
        let (file, size, seen) = super::opened(&path).unwrap();
        let held = std::fs::File::options().write(true).open(&path).unwrap();
        held.lock().unwrap();
        std::fs::write(&journal, b"a journal").unwrap();
        held.set_len(fixture.len() as u64 - 100).unwrap();
        let said = super::headed(&path, &file, size, &seen).unwrap_err().to_string();
        assert!(said.contains("is being changed by another program now: wait for it to finish, then open it again"), "{said}");
        // The change finished before the reader looked again: the file changed since it was opened.
        std::fs::remove_file(&journal).unwrap();
        drop(held);
        let said = crate::plain(&path, &super::headed(&path, &file, size, &seen).unwrap_err());
        assert!(said.contains("changed while it was read (emptied, or written again): open it again"), "{said}");
        std::fs::remove_file(&path).unwrap();
    }
}
