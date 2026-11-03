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
use column::{Indices, Measure, Packed};
pub use derived::derived;
pub use spool::{Order, Spool, Spooled};

/// The first eight bytes of every `.rvr` file.
const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

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
}

/// The data section of these rows and columns, packed a piece at a time beside `beside` on `workers` threads.
pub fn stream(
    rows: Rows,
    columns: Vec<(String, Source)>,
    workers: usize,
    beside: &Path,
) -> Result<(Value, Laid)> {
    // Each array or column is packed into a file of its own in the folder laid, the heaviest first.
    let mut laid = Laid::beside(beside)?;
    let dir = laid.dir.clone();
    let mut edges = vec![0_usize];
    let mut seen = HashSet::new();
    for Sample { name, rows, .. } in &rows.samples {
        if name.is_empty() || !seen.insert(name) {
            return Err(invalid("Sample names must be nonempty and unique"));
        }
        edges.push(edges[edges.len() - 1] + rows);
    }
    let mut unique = HashSet::new();
    if columns
        .iter()
        .any(|(name, _)| name.is_empty() || !unique.insert(name))
    {
        return Err(invalid("Column names must be nonempty and unique"));
    }
    let count = edges[edges.len() - 1];
    let per_sample: Vec<usize> = rows.samples.iter().map(|s| s.rows).collect();
    let mut weights = vec![count * 4, rows.entry.weight()];
    weights.extend(columns.iter().map(|(_, source)| source.weight()));
    let (names, sources): (Vec<String>, Vec<Source>) = columns.into_iter().unzip();
    let sources: Vec<std::sync::Mutex<Option<Source>>> = std::iter::once(rows.entry)
        .chain(sources)
        .map(|source| std::sync::Mutex::new(Some(source)))
        .collect();
    let take = |at: usize| {
        let mut held = sources[at].lock().unwrap_or_else(|e| e.into_inner());
        held.take()
            .ok_or_else(|| invalid("A column was packed twice"))
    };
    // Each file's row bounds, within which its entries strictly increase.
    let mut files = vec![0_usize];
    for file in &rows.files {
        let held = file["rows"].as_u64().ok_or_else(|| invalid("A data.files record has no rows"))?;
        files.push(files[files.len() - 1] + held as usize);
    }
    let mut items: Vec<usize> = (0..names.len() + 2).collect();
    items.sort_by_key(|item| std::cmp::Reverse(weights[*item]));
    // A failure names its file: one it read (the spool, a file a column is copied from), or else this one.
    let work = |item: &usize| -> Result<(usize, Packed, std::path::PathBuf)> {
        let path = dir.join(item.to_string());
        let pack = || -> Result<Packed> {
            let mut out = std::io::BufWriter::new(std::fs::File::create(&path)?);
            let packed = match item {
                0 => column::plain("sample", &Indices::new(&per_sample)?, &Measure::Nothing, &mut out)?,
                1 => column::entry(take(0)?, &edges, &files, &mut out)?,
                n => column::column(&names[n - 2], take(n - 1)?, &edges, &mut out, &path.with_extension("decoded"))?,
            };
            out.flush()?;
            Ok(packed)
        };
        match pack() {
            Ok(packed) => Ok((*item, packed, path)),
            Err(e @ crate::Error::Io(_)) => Err(crate::about(&path, e)),
            Err(e) => Err(e),
        }
    };
    let mut packed = crate::parallel::map(&items, workers.max(1), work)?;
    packed.sort_by_key(|(item, _, _)| *item);
    let (mut packed, files): (Vec<Packed>, Vec<std::path::PathBuf>) = packed.into_iter().map(|(_, p, o)| (p, o)).unzip();
    let mut end = 0;
    for one in &mut packed {
        one.place(&mut end);
    }
    let mut data = Map::new();
    data.insert("rows".into(), count.into());
    let samples = rows.samples.iter();
    data.insert(
        "samples".into(),
        samples
            .map(|s| json!({"name": s.name, "label": s.label, "rows": s.rows}))
            .collect(),
    );
    data.insert("files".into(), Value::Array(rows.files));
    data.insert("sample".into(), Value::Object(packed[0].record.clone()));
    data.insert("entry".into(), Value::Object(packed[1].record.clone()));
    let cols = packed[2..]
        .iter()
        .map(|one| Value::Object(one.record.clone()));
    data.insert("cols".into(), cols.collect());
    let mut data = Value::Object(data);
    let checksum = data_checksum(&data)?;
    data["sha256"] = checksum.into();
    laid.files = files;
    Ok((data, laid))
}

/// A file's compressed arrays in a folder of their own, in file order; deleted when dropped.
#[derive(Debug)]
pub struct Laid {
    dir: std::path::PathBuf,
    files: Vec<std::path::PathBuf>,
}

impl Laid {
    /// An empty folder at `path` with `.arrays` added to its name.
    fn beside(path: &Path) -> Result<Self> {
        let mut name = path.as_os_str().to_owned();
        name.push(".arrays");
        let dir = std::path::PathBuf::from(name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| crate::about(&dir, e.into()))?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| crate::about(&dir, e.into()))?;
        Ok(Self {
            dir,
            files: Vec::new(),
        })
    }

    /// The records of `derived` under `header`, worked out from these arrays before they are written.
    pub fn derived(&self, header: &Value, derived: &[(String, String)]) -> Result<Vec<Value>> {
        let file = crate::rvr::Reader::laid(&self.dir, &self.files, header.clone())?;
        crate::rvr::derived::records(&file, derived, &[])
    }
}

impl Drop for Laid {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// SHA-256 of every array's sha256, in file order, as one ASCII string.
pub(crate) fn data_checksum(data: &Value) -> Result<String> {
    let mut all = String::new();
    let mut add = |record: &Value| {
        let hash = record.get("sha256").and_then(Value::as_str);
        hash.map(|hash| all.push_str(hash))
            .ok_or_else(|| invalid("An array record has no sha256"))
    };
    add(&data["sample"])?;
    add(&data["entry"])?;
    for col in data["cols"].as_array().into_iter().flatten() {
        if col["kind"] == "jagged" {
            add(&col["offsets"])?;
            add(&col["content"])?;
        } else {
            add(col)?;
        }
    }
    Ok(digest(all.as_bytes()))
}

/// The sample columns of a file: every metadata key of `made.files` that makes one over the
/// samples named, in the order the keys first appear, but for a row column's name (`taken`).
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

/// Send the magic, then the header's length and text.
fn frame(mut out: impl Write, header: &Value) -> Result<()> {
    let text = serde_json::to_string(header)?;
    let length = u32::try_from(text.len()).map_err(|_| invalid("the header is longer than 4 GiB, more than the format holds: press fewer samples or files into one file"))?;
    out.write_all(MAGIC)?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(text.as_bytes())?;
    Ok(())
}

/// Write one `.rvr` file whose arrays `stream` laid beside it; each failure names its file.
pub fn write_laid(path: impl AsRef<Path>, header: &Value, laid: Laid) -> Result<()> {
    let path = path.as_ref();
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| crate::about(path, e.into()))?);
    frame(&mut out, header).map_err(|e| crate::about(path, e))?;
    for file in &laid.files {
        copied(arrays_of(file, 0)?, file, &mut out, path)?;
        std::fs::remove_file(file).map_err(|e| crate::about(file, e.into()))?;
    }
    out.flush().map_err(|e| crate::about(path, e.into()))
}

/// Write the file at `from` again at `to` with another header, its arrays copied byte for byte.
pub fn rewrite(from: impl AsRef<Path>, to: impl AsRef<Path>, header: &Value) -> Result<()> {
    use crate::about;
    let (from, to) = (from.as_ref(), to.as_ref());
    if std::fs::canonicalize(to).is_ok_and(|to| std::fs::canonicalize(from).is_ok_and(|from| from == to)) {
        return Err(invalid(format!("{} cannot be written again into itself: write it at another path, then move that over it", from.display())));
    }
    let (start, _) = header_end(from).map_err(|e| about(from, e))?;
    let source = arrays_of(from, start)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(to).map_err(|e| about(to, e.into()))?);
    frame(&mut out, header).map_err(|e| about(to, e))?;
    copied(source, from, out, to)
}

/// The file at `path`, opened at its arrays, which begin at `start`.
fn arrays_of(path: &Path, start: u64) -> Result<std::fs::File> {
    use std::io::{Seek, SeekFrom};
    let opened = std::fs::File::open(path).and_then(|mut file| file.seek(SeekFrom::Start(start)).map(|_| file));
    opened.map_err(|e| crate::about(path, e.into()))
}

/// Copy the rest of `source` (the file at `from`) to `out` (at `to`), each failure naming its file.
fn copied(mut source: std::fs::File, from: &Path, mut out: impl Write, to: &Path) -> Result<()> {
    use std::io::Read;
    let written = |e: std::io::Error| crate::about(to, e.into());
    let mut buffer = vec![0; 1 << 16];
    loop {
        let n = source.read(&mut buffer).map_err(|e| crate::about(from, e.into()))?;
        if n == 0 {
            return out.flush().map_err(written);
        }
        out.write_all(&buffer[..n]).map_err(written)?;
    }
}

/// Let a file's arrays go, keeping its header byte for byte: how many bytes it gave back.
pub fn empty(path: impl AsRef<Path>) -> Result<u64> {
    let (end, size) = header_end(path.as_ref())?;
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_len(end)?;
    file.sync_all()?;
    Ok(size - end)
}

/// Write at `to` the empty `shell`'s header and the arrays of `from`, whose data section is the shell's.
pub fn refilled(shell: impl AsRef<Path>, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    use crate::about;
    use std::io::Read;
    let (shell, from, to) = (shell.as_ref(), from.as_ref(), to.as_ref());
    let same = |a: &Path| std::fs::canonicalize(to).is_ok_and(|to| std::fs::canonicalize(a).is_ok_and(|a| a == to));
    if same(shell) || same(from) {
        return Err(invalid(format!("{} cannot be written into a file it is made from: write it at another path, then move that over it", to.display())));
    }
    let (end, size) = header_end(shell).map_err(|e| about(shell, e))?;
    if end != size {
        return Err(about(shell, invalid("the file to refill holds its arrays: it is not empty")));
    }
    let mut header = vec![0; end as usize];
    std::fs::File::open(shell).and_then(|mut file| file.read_exact(&mut header)).map_err(|e| about(shell, e.into()))?;
    let (start, _) = header_end(from).map_err(|e| about(from, e))?;
    let source = arrays_of(from, start)?;
    let written = |e: std::io::Error| about(to, e.into());
    let mut out = std::io::BufWriter::new(std::fs::File::create(to).map_err(written)?);
    out.write_all(&header).map_err(written)?;
    copied(source, from, out, to)
}

/// Where a file's header ends, and how long the file is.
fn header_end(path: &Path) -> Result<(u64, u64)> {
    use std::io::Read;
    let (mut file, size) = crate::rvr::opened(path)?;
    let mut prefix = vec![0; 12];
    if file.read_exact(&mut prefix).is_err() {
        prefix.clear();
    }
    let end = crate::rvr::header_end(&prefix, size)?;
    Ok((end as u64, size as u64))
}
