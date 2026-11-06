//! Write `.rvr` files (docs/FORMAT.md): pack rows and columns a piece at a time from a spool, a
//! file or memory (`stream`, `write_laid`), and write a file again, empty it or refill it.

use crate::json::canonical;
use crate::rvr::digest;
use crate::rvr::samples::{kind_of, value_checksum, RESERVED};
use crate::{invalid, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

mod column;
mod derived;
pub(crate) mod facts;
mod spool;

pub use column::Source;
pub(crate) use column::{read_at, write_at, Measuring};
pub use derived::derived;
pub use spool::{Order, Spool, Spooled};

/// Values of one kind, as the little-endian bytes of that kind.
#[derive(Debug)]
pub struct NumericBuffer {
    /// Their kind: `f4`, `i8`, `b1`.
    pub kind: String,
    /// How many values.
    pub count: usize,
    /// Their bytes, `count` times the kind's width.
    pub bytes: Vec<u8>,
}

/// One column over every row: a value per row, or a list of values per row.
#[derive(Debug)]
pub enum Values {
    /// One value per row.
    Flat(NumericBuffer),
    /// `counts[r]` values of `content` belong to row `r`, in order.
    Jagged {
        /// How many values each row holds.
        counts: Vec<usize>,
        /// Every row's values, in row order.
        content: NumericBuffer,
    },
}

impl Values {
    /// Its values, and for a list column how many each row holds.
    pub(crate) fn parts(self) -> (NumericBuffer, Option<Vec<usize>>) {
        match self {
            Values::Flat(buffer) => (buffer, None),
            Values::Jagged { counts, content } => (content, Some(counts)),
        }
    }
}

/// A number that an `.rvr` array can hold.
pub trait Scalar: Copy {
    /// The header spelling of this type: `f4`, `i8`, `u4`, `b1`.
    const KIND: &'static str;
    /// Append this value's little-endian bytes.
    fn put(self, bytes: &mut Vec<u8>);
}

macro_rules! scalar {
    ($($type:ty => $kind:literal),*) => {$(
        impl Scalar for $type {
            const KIND: &'static str = $kind;
            fn put(self, bytes: &mut Vec<u8>) {
                bytes.extend(self.to_le_bytes());
            }
        }
    )*};
}
scalar!(f64 => "f8", f32 => "f4", i64 => "i8", i32 => "i4", i16 => "i2", i8 => "i1",
        u64 => "u8", u32 => "u4", u16 => "u2", u8 => "u1");

impl Scalar for bool {
    const KIND: &'static str = "b1";
    fn put(self, bytes: &mut Vec<u8>) {
        bytes.push(u8::from(self));
    }
}

/// Hold typed values as the little-endian bytes of their kind.
pub fn array<T: Scalar>(values: &[T]) -> NumericBuffer {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        value.put(&mut bytes);
    }
    NumericBuffer {
        kind: T::KIND.into(),
        count: values.len(),
        bytes,
    }
}

/// A sample as `data.samples` records it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// What every tool calls it.
    pub name: String,
    /// What a plot shows.
    pub label: String,
    /// How many rows it has.
    pub rows: usize,
}

/// Who the rows belong to: everything of a file but its columns.
pub struct Rows<'a> {
    /// Each sample, in row order.
    pub samples: Vec<Sample>,
    /// The files read, in row order, as `{"sample", "path", "rows"}`.
    pub files: Vec<Value>,
    /// Each row's entry number in its source tree, as `i8` values.
    pub entry: Source<'a>,
    /// The rows of one chunk of every array (docs/FORMAT.md §7).
    pub chunk_rows: usize,
}

/// Pack these rows and columns on `workers` threads into `path`, created or truncated, and removed if packing fails: its data section, and the file.
// The arrays lie from byte 8 on, one after another in record order; the header is `write_laid`'s.
pub fn stream(rows: Rows, columns: Vec<(String, Source)>, workers: usize, path: &Path) -> Result<(Value, Laid)> {
    let mut opened = false;
    let streamed = streaming(rows, columns, workers, path, &mut opened);
    // Only a file it created or cut to nothing goes: one refused before that is as it was.
    if streamed.is_err() && opened {
        let _ = std::fs::remove_file(path);
    }
    streamed
}

/// `stream`, its file left as it is when it fails; `opened` once the file is created or cut.
fn streaming(rows: Rows, columns: Vec<(String, Source)>, workers: usize, path: &Path, opened: &mut bool) -> Result<(Value, Laid)> {
    let mut edges = vec![0_usize];
    let mut seen = HashSet::new();
    for Sample { name, rows, .. } in &rows.samples {
        if name.is_empty() || !seen.insert(name) {
            return Err(invalid(format!("the sample name {name:?} is empty or given twice: give each sample a name of its own")));
        }
        edges.push(edges[edges.len() - 1] + rows);
    }
    let mut unique = HashSet::new();
    if let Some((name, _)) = columns.iter().find(|(name, _)| name.is_empty() || !unique.insert(name)) {
        return Err(invalid(format!("the column name {name:?} is empty or given twice: give each column a name of its own")));
    }
    let count = edges[edges.len() - 1];
    bounded(rows.chunk_rows)?;
    let cut = crate::chunks::rows(&edges, rows.chunk_rows);
    let samples = rows.samples.len();
    // Each file's row bounds, within which its entries strictly increase.
    let mut files = vec![0_usize];
    for file in &rows.files {
        let held = file["rows"].as_u64().ok_or_else(|| invalid(format!("the data.files record {file} has no rows: give each file record its rows, a whole number")))?;
        files.push(files[files.len() - 1] + held as usize);
    }
    let entry = column::Ready::of(rows.entry, "entry")?;
    let entry = entry.pieces().ok_or_else(|| invalid("the entry array cannot be copied from another file: give it as values or spooled pieces"))?;
    let ready = columns.into_iter().map(|(name, source)| column::Ready::of(source, &name).map(|ready| (name, ready))).collect::<Result<Vec<_>>>()?;
    let mut file = std::fs::File::options().read(true).write(true).create(true).truncate(true).open(path).map_err(|e| crate::about(path, e.into()))?;
    *opened = true;
    file.write_all(crate::rvr::MAGIC).map_err(|e| crate::about(path, e.into()))?;
    let file = std::sync::Arc::new(file);
    let (records, end) = packing(column::Out::new(Some(file.clone()), 0), &cut, samples, Some((entry, &files)), &ready, workers, path)?;
    let mut records = records.into_iter();
    let (sample, entry) = (records.next().unwrap_or_default(), records.next().unwrap_or_default());
    let cols: Vec<Map<String, Value>> = records.collect();
    let mut data = Map::new();
    data.insert("rows".into(), count.into());
    data.insert("chunk_rows".into(), rows.chunk_rows.into());
    let samples = rows.samples.iter();
    data.insert("samples".into(), samples.map(|s| json!({"name": s.name, "label": s.label, "rows": s.rows})).collect());
    data.insert("files".into(), Value::Array(rows.files));
    data.insert("sample".into(), Value::Object(sample));
    data.insert("entry".into(), Value::Object(entry));
    data.insert("cols".into(), cols.into_iter().map(Value::Object).collect());
    let mut data = Value::Object(data);
    data_sealed(&mut data)?;
    Ok((data, Laid { path: path.to_path_buf(), file, end, finished: false }))
}

/// Pack `ready` on `workers` threads into `out`, after the rows' own `sample` and `entry` arrays
/// when `own` gives their entries and each file's row bounds (a new file): every record placed at
/// the chunks `cut`, and where the arrays end; a failed read or write named against `path`.
fn packing<'e>(mut out: column::Out, cut: &[crate::chunks::Rows], samples: usize, own: Option<(&'e dyn column::Pieces, &[usize])>, ready: &'e [(String, column::Ready)], workers: usize, path: &Path) -> Result<(Vec<Map<String, Value>>, u64)> {
    let written = |e: crate::Error| match e {
        e @ crate::Error::Io(_) => crate::about(path, e),
        e => e,
    };
    let mut records = std::thread::scope(|scope| -> Result<Vec<_>> {
        let pool = crate::parallel::Pool::new(scope, workers);
        let mut records = Vec::new();
        if let Some((entry, files)) = own {
            records.push(column::sample(cut, &mut out, &pool).map_err(written)?);
            records.push(column::entry(entry, cut, samples, files, &mut out, &pool).map_err(written)?);
        }
        for (name, ready) in ready {
            records.push(column::packed(name, ready, cut, samples, &mut out, &pool).map_err(written)?);
        }
        out.settle().map_err(written)?;
        Ok(records)
    })?;
    for record in &mut records {
        out.placed(record, cut)?;
    }
    let end = out.at;
    out.flushed().map_err(written)?;
    Ok((records, end))
}

/// Pack `columns` of the rows `data` lays out on `workers` threads into `file` (at `path`), their
/// arrays from `at` (counted from byte 8) on, or only count their bytes with no file: each one's
/// record, and where the arrays then end.
pub(crate) fn added(file: Option<std::sync::Arc<std::fs::File>>, at: u64, data: &Value, columns: Vec<(String, Source)>, workers: usize, path: &Path) -> Result<(Vec<Value>, u64)> {
    let mut unique = HashSet::new();
    let taken: Vec<&str> = data["cols"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str()).collect();
    if let Some((name, _)) = columns.iter().find(|(name, _)| name.is_empty() || taken.contains(&name.as_str()) || !unique.insert(name)) {
        return Err(invalid(format!("the column name {name:?} is empty, given twice, or one the file has: give each column a name of its own")));
    }
    let cut = crate::chunks::of(data)?;
    let samples = data["samples"].as_array().map_or(0, Vec::len);
    let ready = columns.into_iter().map(|(name, source)| column::Ready::of(source, &name).map(|ready| (name, ready))).collect::<Result<Vec<_>>>()?;
    let (records, end) = packing(column::Out::new(file, at), &cut, samples, None, &ready, workers, path)?;
    Ok((records.into_iter().map(Value::Object).collect(), end))
}

/// Refuse rows (`samples`, their `entry`) that are not, in order, those of `data`, whose files' entries are `read`.
// `read` holds the entries of each of `data.files`, in order, as they were read and checked. Cut at
// its chunks, the samples give its `sample` array's `sha256` (docs/FORMAT.md §7, §8), no array
// written; the entries are compared with those read; the error says what differs.
pub fn rows_agree(samples: &[Sample], entry: Source, data: &Value, read: &[&[u64]]) -> Result<()> {
    let cut = crate::chunks::of(data)?;
    let mut edges = vec![0_usize];
    for sample in samples {
        edges.push(edges[edges.len() - 1] + sample.rows);
    }
    let (rows, held) = (edges[edges.len() - 1], cut.last().map_or(0, |last| last.2));
    if rows != held {
        return Err(invalid(format!("they are {rows} rows, and the file holds {held}")));
    }
    let files = data["files"].as_array().map_or(&[][..], Vec::as_slice);
    let file = |at: usize| format!("{} of {}", files[at]["path"].as_str().unwrap_or("?"), files[at]["sample"].as_str().unwrap_or("?"));
    if read.len() != files.len() {
        return Err(invalid(format!("the entries read are of {} files, and the file lists {}: give the entries of each of its files, in order", read.len(), files.len())));
    }
    if let Some(at) = (0..files.len()).find(|at| files[*at]["rows"].as_u64() != Some(read[*at].len() as u64)) {
        return Err(invalid(format!("{} entries of {} were read, and the file lists {} rows of it: give the entries of each of its files, in order", read[at].len(), file(at), files[at]["rows"])));
    }
    let sha256 = column::sample_sha256(&edges, &cut)?;
    if data["sample"]["sha256"].as_str() != Some(sha256.as_str()) {
        return Err(invalid(format!("their sample array's sha256 is {sha256}, and the file's data.sample.sha256 is {}", data["sample"]["sha256"])));
    }
    let entry = column::Ready::of(entry, "entry")?;
    let pieces = entry.pieces().ok_or_else(|| invalid("the entries are copied from a file, and they are given as values or spooled pieces"))?;
    if pieces.kind() != "i8" || pieces.listed() {
        return Err(invalid(format!("their entries are {} values{}, and an entry is one i8", pieces.kind(), if pieces.listed() { " in lists" } else { "" })));
    }
    // The file's own, row by row: of which of its files a row is.
    let of = |row: usize| {
        let mut end = 0;
        (0..files.len()).find(|at| {
            end += read[*at].len();
            row < end
        })
    };
    match column::entries_differ(pieces, read)? {
        None => Ok(()),
        Some((row, Some(entry), Some(was))) => Err(invalid(format!("row {row}'s entry is {entry}, and the file's is {was} (a row of {})", of(row).map_or("?".into(), file)))),
        Some((row, Some(_), None)) => Err(invalid(format!("they hold more entries than the file's {row}"))),
        Some((row, None, _)) => Err(invalid(format!("they hold {row} entries, fewer than the file's {rows}"))),
    }
}

/// `data.sha256` made again from its arrays' records (docs/FORMAT.md §8): after columns are added or put in another order.
pub fn data_sealed(data: &mut Value) -> Result<()> {
    let checksum = data_checksum(data).map_err(|place| invalid(format!("a bug: the array record {place} has no sha256; please report it")))?;
    data["sha256"] = checksum.into();
    Ok(())
}

/// A file being written, its header still to come (`write_laid`): removed when dropped without one.
#[derive(Debug)]
pub struct Laid {
    path: std::path::PathBuf,
    file: std::sync::Arc<std::fs::File>,
    end: u64,
    finished: bool,
}

impl Drop for Laid {
    fn drop(&mut self) {
        if !self.finished {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Laid {
    /// The records of `derived` under `header`, worked out from these arrays before the header is written.
    pub fn derived(&self, header: &Value, derived: &[(String, String)]) -> Result<Vec<Value>> {
        let file = crate::rvr::Reader::laid(&self.path, self.file.clone(), self.end, header.clone())?;
        crate::rvr::derived::records(&file, derived, &[])
    }
}

/// SHA-256 of every array's sha256, in record order, as one ASCII string; or the place of an array record with none.
pub(crate) fn data_checksum(data: &Value) -> std::result::Result<String, String> {
    let mut all = String::new();
    for (record, place, _) in crate::rvr::arrays::records(data) {
        all.push_str(record.get("sha256").and_then(Value::as_str).ok_or_else(|| place.to_string())?);
    }
    Ok(digest(all.as_bytes()))
}

/// The sample columns of the samples named: each metadata key of `made.files` that makes one, in order, but `taken`.
pub fn sample_cols(samples: &[&str], files: &Value, taken: &[&str]) -> Result<Vec<Value>> {
    let records: Vec<&Value> = samples
        .iter()
        .map(|name| files.get(name).unwrap_or(&Value::Null))
        .collect();
    let out: HashSet<&str> = taken.iter().copied().chain(RESERVED).collect();
    let mut keys: Vec<&String> = Vec::new();
    for record in &records {
        let metadata = record.get("metadata").and_then(Value::as_object);
        for key in metadata.into_iter().flat_map(Map::keys) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    let mut cols = Vec::new();
    for key in keys {
        let values: Vec<&Value> = records
            .iter()
            .map(|record| record.get("metadata").and_then(|all| all.get(key)))
            .map(|value| value.unwrap_or(&Value::Null))
            .collect();
        if out.contains(key.as_str()) {
            continue;
        }
        if let Some(kind) = kind_of(&values) {
            let sha256 = value_checksum(kind, &values, key)?;
            cols.push(json!({"name": key, "kind": kind, "values": values, "sha256": sha256}));
        }
    }
    Ok(cols)
}

/// SHA-256 of a text as UTF-8, as `made.recipe`, `made.samples` and `made.lock` record it.
pub fn text_checksum(text: &str) -> String {
    digest(text.as_bytes())
}

/// SHA-256 of the complete `made` section, excluding its own checksum.
pub fn made_checksum(made: &Value) -> Result<String> {
    let mut body = made.clone();
    if let Some(body) = body.as_object_mut() {
        body.shift_remove("sha256");
    }
    Ok(digest(canonical(&body)?.as_bytes()))
}

/// Send what follows the arrays (docs/FORMAT.md §1): the header's text, its length and the magic;
/// how many bytes they take.
pub(crate) fn tail(mut out: impl Write, header: &Value) -> Result<u64> {
    framing(&mut out, serde_json::to_string(header)?.as_bytes())
}

/// Send a header's `text` as the file holds it, then its length and the magic; how many bytes they take.
pub(crate) fn framing(mut out: impl Write, text: &[u8]) -> Result<u64> {
    let length = u32::try_from(text.len()).map_err(|_| invalid("the header is longer than 4 GiB, more than the format holds: press fewer samples or files into one file"))?;
    out.write_all(text)?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(crate::rvr::MAGIC)?;
    Ok(text.len() as u64 + 12)
}

/// Write `header` after the arrays of the file `stream` began, then its length and the magic (docs/FORMAT.md §1).
pub fn write_laid(header: &Value, mut laid: Laid) -> Result<()> {
    let path = laid.path.clone();
    let mut out = std::io::BufWriter::new(&*laid.file);
    std::io::Seek::seek(&mut out, std::io::SeekFrom::Start(8 + laid.end)).map_err(|e| crate::about(&path, e.into()))?;
    let tail = tail(&mut out, header).map_err(|e| crate::about(&path, e))?;
    let file = out.into_inner().map_err(|e| crate::about(&path, e.into_error().into()))?;
    file.set_len(8 + laid.end + tail).map_err(|e| crate::about(&path, e.into()))?;
    laid.finished = true;
    Ok(())
}

/// Write the file at `from` again at `to` with another header, its arrays copied byte for byte.
pub fn rewrite(from: impl AsRef<Path>, to: impl AsRef<Path>, header: &Value) -> Result<()> {
    use crate::about;
    let (from, to) = (from.as_ref(), to.as_ref());
    if same_file(from, to) {
        return Err(invalid(format!("{} cannot be written again into itself: write it at another path, then move that over it", from.display())));
    }
    crate::change::restored(from)?;
    let (source, start) = arrays_of(from)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(to).map_err(|e| about(to, e.into()))?);
    copied(&source, 0, start, from, &mut out, to)?;
    tail(&mut out, header).map_err(|e| about(to, e))?;
    out.flush().map_err(|e| about(to, e.into()))
}

/// Whether two paths name one file, through links: one device and inode (one path where the system has no inodes).
pub fn same_file(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    let (a, b) = (a.as_ref(), b.as_ref());
    match cfg!(unix) {
        true => matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(a), Ok(b)) if crate::change::ids(&a) == crate::change::ids(&b)),
        false => matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b),
    }
}

/// The file at `path`, opened at its first byte, and where its header starts (its arrays from byte 8 to there).
fn arrays_of(path: &Path) -> Result<(crate::change::Handle, u64)> {
    let (file, _, start) = crate::rvr::framed(path).map_err(|e| crate::about(path, e))?;
    Ok((file, start as u64))
}

/// Copy the `length` bytes of `source` (the file at `from`) from byte `at` to `out` (at `to`), a
/// mebibyte at a time: a failed read named against `from`, a failed write against `to`.
pub(crate) fn copied(source: &std::fs::File, at: u64, length: u64, from: &Path, mut out: impl Write, to: &Path) -> Result<()> {
    let mut buffer = vec![0; length.min(1 << 20) as usize];
    let mut done = 0;
    while done < length {
        let piece = &mut buffer[..(length - done).min(1 << 20) as usize];
        read_at(source, piece, at + done).map_err(|e| crate::about(from, e.into()))?;
        out.write_all(piece).map_err(|e| crate::about(to, e.into()))?;
        done += piece.len() as u64;
    }
    Ok(())
}

/// Let the arrays of the file `locked` go, its header kept byte for byte: how many bytes it gave back.
// The file is written again beside it, as `FILE.empty-PID-N`, and moved over it (docs/FORMAT.md §1).
pub fn empty(locked: crate::change::Locked) -> Result<u64> {
    let path = locked.path().to_path_buf();
    let (_, start, tail) = locked.tail().map_err(|e| crate::about(&path, e))?;
    let (staged, file) = crate::change::staging(&path, ".empty").map_err(|e| crate::about(&path, e.into()))?;
    let written = (|| -> Result<()> {
        let mut out = std::io::BufWriter::new(file);
        out.write_all(crate::rvr::MAGIC)?;
        out.write_all(&tail)?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        locked.replace(&staged)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    written.map_err(|e| crate::about(&path, e))?;
    Ok(start - 8)
}

/// Move the file at `staged` over `target` with `permissions` (the file there's, when it has one); a failure names its file.
pub fn moved_over(staged: &Path, target: &Path, permissions: Option<std::fs::Permissions>) -> std::io::Result<()> {
    if let Some(permissions) = permissions {
        std::fs::set_permissions(staged, permissions).map_err(|e| crate::tagged(staged, e))?;
    }
    std::fs::rename(staged, target).map_err(|e| crate::tagged(target, e))
}

/// `path` with `suffix` added to its name.
pub fn beside(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// Write at `to` the empty file `shell` locks with the arrays of `from`, each where the shell records it.
// `from`'s data section is the shell's but where its arrays lie, so the file is the one emptied.
pub fn refilled(shell: &crate::change::Locked, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    use crate::about;
    let (from, to) = (from.as_ref(), to.as_ref());
    let (locked, shell) = (shell, shell.path());
    if same_file(from, shell) {
        return Err(invalid(format!("{} is {} itself: refill it from another file, a press of its own study", from.display(), shell.display())));
    }
    if same_file(shell, to) || same_file(from, to) {
        return Err(invalid(format!("{} cannot be written into a file it is made from: write it at another path, then move that over it", to.display())));
    }
    crate::change::restored(from)?;
    // Read through the descriptor that holds its lock: no other is opened while it is held.
    let (_, start, mut text) = locked.tail().map_err(|e| about(shell, e))?;
    if start != 8 {
        return Err(about(shell, invalid("the file to refill holds its arrays: only a file rvr empty emptied is refilled")));
    }
    text.truncate(text.len() - 12);
    // `from` opened once, and its header read and checked once.
    let (source, _, from_text, _) = crate::rvr::head(from).map_err(|e| about(from, e))?;
    let header = |text: &[u8], path: &Path| crate::rvr::checked_header(text).map_err(|e| about(path, e));
    let (was, now) = (header(&text, shell)?, header(&from_text, from)?);
    let (records, from_records) = (crate::rvr::arrays::records(&was["data"]), crate::rvr::arrays::records(&now["data"]));
    if records.len() != from_records.len() {
        return Err(invalid(format!("{} holds other arrays than {} records: refill it from a press of its own study", from.display(), shell.display())));
    }
    for ((shell_record, place, _), (record, _, _)) in records.iter().zip(&from_records) {
        if record["sha256"] != shell_record["sha256"] || record["size"] != shell_record["size"] {
            return Err(invalid(format!("{} does not hold the arrays {} records ({place} differs): refill it from a press of its own study", from.display(), shell.display())));
        }
    }
    // Each array where the shell lays it, and where it lies in `from`: in the shell's order, its
    // arrays filling the bytes from 8 with no gap (its header was checked).
    let ((_, laid), (_, lying)) = (crate::rvr::spans(&was).map_err(|e| about(shell, e))?, crate::rvr::spans(&now).map_err(|e| about(from, e))?);
    let mut spans: Vec<(u64, u64, u64)> = laid.iter().zip(&lying).map(|(&(shell_at, _), &(at, size))| (shell_at, at, size)).collect();
    spans.sort_unstable();
    let written = |e: std::io::Error| about(to, e.into());
    let mut out = std::io::BufWriter::new(std::fs::File::create(to).map_err(written)?);
    out.write_all(crate::rvr::MAGIC).map_err(written)?;
    for (_, at, size) in spans {
        copied(&source, 8 + at, size, from, &mut out, to)?;
    }
    framing(&mut out, &text).map_err(|e| about(to, e))?;
    out.flush().map_err(written)
}

/// Refuse a `chunk_rows` outside the bounds a chunk's rows lie in.
pub(crate) fn bounded(chunk_rows: usize) -> Result<()> {
    let (fewest, most) = rvr_recipe::CHUNK_BOUNDS;
    match (fewest..=most).contains(&chunk_rows) {
        true => Ok(()),
        false => Err(invalid(format!("chunk_rows is {chunk_rows}, and a chunk holds from 1,024 to 16,777,216 rows (docs/FORMAT.md §3): give the recipe's chunk_rows within them, or leave it out"))),
    }
}

#[cfg(test)]
mod tests {
    /// A writer that fails every write.
    struct Full;

    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // SPEC 1.5
    #[test]
    fn a_failed_copy_names_the_file_it_read_or_the_file_it_wrote() {
        let (from, to) = (std::path::Path::new("from.rvr"), std::path::Path::new("to.rvr"));
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-copied.rvr", std::process::id()));
        std::fs::write(&path, vec![7; 3 << 20]).unwrap();
        let source = std::fs::File::open(&path).unwrap();
        // Read past its end (the file cut short as it was read), then written where there is no room.
        let said = crate::plain(from, &super::copied(&source, 1 << 20, 3 << 20, from, Vec::new(), to).unwrap_err());
        assert!(said.starts_with("from.rvr changed while it was read"), "{said}");
        // A source that cannot be read (opened only to be written) is the file read.
        let unreadable = std::fs::File::options().write(true).open(&path).unwrap();
        let said = crate::plain(to, &super::copied(&unreadable, 0, 3 << 20, from, Vec::new(), to).unwrap_err());
        assert!(said.starts_with("from.rvr") && !said.contains("to.rvr"), "{said}");
        let said = crate::plain(from, &super::copied(&source, 0, 3 << 20, from, Full, to).unwrap_err());
        assert!(said.starts_with("to.rvr") && !said.contains("from.rvr"), "{said}");
        // Copied whole, a mebibyte at a time and the rest.
        let mut out = Vec::new();
        super::copied(&source, 5, (3 << 20) - 5, from, &mut out, to).unwrap();
        assert_eq!(out, vec![7; (3 << 20) - 5]);
        std::fs::remove_file(&path).unwrap();
    }

    // SPEC 1.5, 8.12
    #[test]
    fn a_column_added_that_cannot_be_written_is_told_against_the_file_written() {
        let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/rvr/valid/fixture.rvr")).unwrap();
        let length = u32::from_le_bytes(fixture[fixture.len() - 12..fixture.len() - 8].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&fixture[fixture.len() - 12 - length..fixture.len() - 12]).unwrap();
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-added.rvr", std::process::id()));
        std::fs::write(&path, &fixture).unwrap();
        // Opened only to be read, the file refuses the arrays written to it.
        let file = std::sync::Arc::new(std::fs::File::open(&path).unwrap());
        let column = vec![("more".to_owned(), super::Source::Values(super::Values::Flat(super::array(&[1_u8; 5]))))];
        let refused = super::added(Some(file), (fixture.len() - 20 - length) as u64, &header["data"], column, 1, &path).unwrap_err();
        assert!(matches!(&refused, crate::Error::File(at, _) if *at == path), "{refused:?}");
        std::fs::remove_file(&path).unwrap();
    }
}
