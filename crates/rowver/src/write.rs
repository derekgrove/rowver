//! Build and write an `.rvr` file: a plain JSON header, then each array compressed on its own.
//!
//! Documentation: `docs/FORMAT.md`.
//! A column is refused, not converted, when its pieces differ in kind; a piece whose column
//! does not have one value per row is refused before joining.

use crate::json::canonical;
use crate::rvr::digest;
use crate::rvr::samples::{kind_of, value_checksum, RESERVED};
use crate::{invalid, NumericBuffer, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;

mod column;
mod derived;
mod facts;
mod spool;

pub use column::Source;
use column::{Indices, Measure, Packed};
pub use derived::{derived, derived_with, Lineage};
pub use spool::{Order, Spool, Spooled};

/// The first eight bytes of every `.rvr` file.
const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

/// The text of each labelled value of an integer column, by column name.
pub type Labels = BTreeMap<String, BTreeMap<i128, String>>;

/// One column over every row: a value per row, or a list of values per row.
#[derive(Debug)]
pub enum Values {
    Flat(NumericBuffer),
    /// `counts[r]` values of `content` belong to row `r`, in order.
    Jagged {
        counts: Vec<usize>,
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

/// Whole arrays ready to be recorded: who the rows belong to, and every column over them.
#[derive(Debug)]
pub struct Skim {
    /// Each sample, in row order.
    pub samples: Vec<Sample>,
    /// The files read, in row order, as `{"sample", "path", "rows"}`.
    pub files: Vec<Value>,
    /// Each row's index into `samples`.
    pub sample: Vec<i32>,
    /// Each row's entry number in its source tree.
    pub entry: Vec<i64>,
    pub columns: Vec<(String, Values)>,
    pub labels: Labels,
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

/// One sample's rows before they are joined with the other samples'.
#[derive(Debug)]
pub struct Piece {
    pub name: String,
    pub label: String,
    pub columns: Vec<(String, Values)>,
    pub entry: Vec<i64>,
}

/// Who the rows belong to: everything of a file but its columns.
pub struct Rows<'a> {
    /// Each sample, in row order.
    pub samples: Vec<Sample>,
    /// The files read, in row order, as `{"sample", "path", "rows"}`.
    pub files: Vec<Value>,
    /// Each row's entry number in its source tree, as `i8` values.
    pub entry: Source<'a>,
    /// The text of each labelled value of an integer column.
    pub labels: Labels,
}

/// Where each array of a file is packed into: memory, or a file of its own in this folder.
enum Target<'a> {
    Memory,
    Files(&'a Path),
}

/// One packed array or column and where its bytes went.
enum Out {
    Memory(Vec<u8>),
    File(std::path::PathBuf),
}

/// The data section of a file of these columns, packed on up to `workers` threads, the heaviest
/// first; each array or column's bytes, in file order.
fn lay(
    rows: Rows,
    columns: Vec<(String, Source)>,
    workers: usize,
    into: Target,
) -> Result<(Value, Vec<(Packed, Out)>)> {
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
    let labels = &rows.labels;
    let mut items: Vec<usize> = (0..names.len() + 2).collect();
    items.sort_by_key(|item| std::cmp::Reverse(weights[*item]));
    let work = |item: &usize| -> Result<(usize, Packed, Out)> {
        let mut memory = Vec::new();
        let (mut file, path) = match into {
            Target::Memory => (None, None),
            Target::Files(dir) => {
                let path = dir.join(item.to_string());
                (
                    Some(std::io::BufWriter::new(std::fs::File::create(&path)?)),
                    Some(path),
                )
            }
        };
        let out: &mut dyn Write = match &mut file {
            Some(file) => file,
            None => &mut memory,
        };
        let packed = match item {
            0 => column::plain(
                "sample",
                &Indices::new(&per_sample)?,
                &Measure::Nothing,
                out,
            )?,
            1 => column::entry(take(0)?, &edges, out)?,
            n => {
                let name = &names[n - 2];
                let scratch = path.as_ref().map(|path| path.with_extension("decoded"));
                column::column(
                    name,
                    take(n - 1)?,
                    &edges,
                    labels.get(name),
                    out,
                    scratch.as_deref(),
                )?
            }
        };
        if let Some(file) = &mut file {
            file.flush()?;
        }
        Ok((*item, packed, path.map_or(Out::Memory(memory), Out::File)))
    };
    let mut packed = crate::parallel::map(&items, workers.max(1), work)?;
    packed.sort_by_key(|(item, _, _)| *item);
    let mut packed: Vec<(Packed, Out)> = packed.into_iter().map(|(_, p, o)| (p, o)).collect();
    let mut end = 0;
    for (one, _) in &mut packed {
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
    data.insert("sample".into(), Value::Object(packed[0].0.record.clone()));
    data.insert("entry".into(), Value::Object(packed[1].0.record.clone()));
    let cols = packed[2..]
        .iter()
        .map(|(one, _)| Value::Object(one.record.clone()));
    data.insert("cols".into(), cols.collect());
    let mut data = Value::Object(data);
    let checksum = data_checksum(&data)?;
    data["sha256"] = checksum.into();
    Ok((data, packed))
}

/// The data section and the compressed arrays of whole arrays, compressed on every core.
pub fn build(skim: Skim) -> Result<(Value, Vec<Vec<u8>>)> {
    build_with(skim, crate::parallel::cores())
}

/// As `build`, compressing on at most `workers` threads; the bytes do not depend on how many.
pub fn build_with(skim: Skim, workers: usize) -> Result<(Value, Vec<Vec<u8>>)> {
    let rows = skim.entry.len();
    let mut edges = vec![0_usize];
    for sample in &skim.samples {
        edges.push(edges[edges.len() - 1] + sample.rows);
    }
    if edges[edges.len() - 1] != rows || skim.sample.len() != rows {
        return Err(invalid("Sample row counts do not sum to rows"));
    }
    for index in 0..skim.samples.len() {
        let own = &skim.sample[edges[index]..edges[index + 1]];
        if own
            .iter()
            .any(|sample| usize::try_from(*sample) != Ok(index))
        {
            return Err(invalid("Each row must name the sample whose rows hold it"));
        }
    }
    let columns = skim
        .columns
        .into_iter()
        .map(|(name, values)| (name, Source::Values(values)))
        .collect();
    let rows = Rows {
        samples: skim.samples,
        files: skim.files,
        entry: Source::Values(Values::Flat(array(&skim.entry))),
        labels: skim.labels,
    };
    let (data, packed) = lay(rows, columns, workers, Target::Memory)?;
    let mut arrays = Vec::new();
    for (one, out) in packed {
        let Out::Memory(mut bytes) = out else {
            unreachable!("packed into memory")
        };
        // A list column's offsets come before its content.
        if let [(offsets, _), _] = one.arrays[..] {
            let content = bytes.split_off(offsets);
            arrays.extend([bytes, content]);
        } else {
            arrays.push(bytes);
        }
    }
    Ok((data, arrays))
}

/// As `build_with`, but from columns of any source, packed a piece at a time into a folder beside
/// `beside`: nothing held grows with the rows.
pub fn stream(
    rows: Rows,
    columns: Vec<(String, Source)>,
    workers: usize,
    beside: &Path,
) -> Result<(Value, Laid)> {
    let mut laid = Laid::beside(beside)?;
    let (data, packed) = lay(rows, columns, workers, Target::Files(&laid.dir))?;
    laid.files = packed
        .into_iter()
        .map(|(_, out)| match out {
            Out::File(path) => path,
            Out::Memory(_) => unreachable!("packed into files"),
        })
        .collect();
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
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            files: Vec::new(),
        })
    }
}

impl Drop for Laid {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// SHA-256 of every array's sha256, in file order, as one ASCII string.
pub fn data_checksum(data: &Value) -> Result<String> {
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

/// Concatenate one column's pieces, which must agree in shape and kind.
fn join(name: &str, pieces: Vec<Values>) -> Result<Values> {
    let mut pieces = pieces.into_iter();
    let mut joined = pieces.next().ok_or_else(|| invalid("No samples"))?;
    for piece in pieces {
        match (&mut joined, piece) {
            (Values::Flat(all), Values::Flat(more)) if all.kind == more.kind => {
                all.count += more.count;
                all.bytes.extend(more.bytes);
            }
            (
                Values::Jagged { counts, content },
                Values::Jagged {
                    counts: more_counts,
                    content: more,
                },
            ) if content.kind == more.kind => {
                counts.extend(more_counts);
                content.count += more.count;
                content.bytes.extend(more.bytes);
            }
            _ => return Err(invalid(format!("{name}: samples differ in its kind"))),
        }
    }
    Ok(joined)
}

/// The data section and the compressed arrays of a skim held one sample at a time,
/// compressed on every core.
pub fn pack(
    pieces: Vec<Piece>,
    labels: Labels,
    files: Vec<Value>,
) -> Result<(Value, Vec<Vec<u8>>)> {
    pack_with(pieces, labels, files, crate::parallel::cores())
}

/// As `pack`, compressing on at most `workers` threads; the bytes do not depend on how many.
pub fn pack_with(
    pieces: Vec<Piece>,
    labels: Labels,
    files: Vec<Value>,
    workers: usize,
) -> Result<(Value, Vec<Vec<u8>>)> {
    let samples: Vec<Sample> = pieces
        .iter()
        .map(|piece| Sample {
            name: piece.name.clone(),
            label: piece.label.clone(),
            rows: piece.entry.len(),
        })
        .collect();
    let mut sample = Vec::new();
    for (index, record) in samples.iter().enumerate() {
        let index = i32::try_from(index).map_err(|_| invalid("Too many samples"))?;
        sample.extend(std::iter::repeat_n(index, record.rows));
    }
    let mut entry = Vec::new();
    let mut parts: Vec<(String, Vec<Values>)> = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        let rows = piece.entry.len();
        entry.extend(piece.entry);
        let mut own: HashMap<String, Values> = HashMap::new();
        for (name, values) in piece.columns {
            let held = match &values {
                Values::Flat(flat) => flat.count,
                Values::Jagged { counts, .. } => counts.len(),
            };
            if held != rows {
                return Err(invalid(format!(
                    "{name}: needs one value per row of sample {}",
                    piece.name
                )));
            }
            if index == 0 {
                parts.push((name, vec![values]));
            } else {
                own.insert(name, values);
            }
        }
        for (name, joined) in parts.iter_mut().skip_while(|_| index == 0) {
            let values = own.remove(name);
            joined.push(values.ok_or_else(|| {
                invalid(format!("{name}: sample {} does not hold it", piece.name))
            })?);
        }
    }
    let columns = parts
        .into_iter()
        .map(|(name, pieces)| Ok((name.clone(), join(&name, pieces)?)))
        .collect::<Result<_>>()?;
    build_with(
        Skim {
            samples,
            files,
            sample,
            entry,
            columns,
            labels,
        },
        workers,
    )
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

/// SHA-256 of the complete `made` section, or the `derived` one, excluding its own checksum.
pub fn made_checksum(made: &Value) -> Result<String> {
    let mut body = made.clone();
    if let Some(body) = body.as_object_mut() {
        body.shift_remove("sha256");
    }
    Ok(digest(canonical(&body)?.as_bytes()))
}

/// Send the magic, the header's length and text, then the arrays.
fn frame(mut out: impl Write, header: &Value, arrays: &[Vec<u8>]) -> Result<()> {
    let text = serde_json::to_string(header)?;
    let length = u32::try_from(text.len()).map_err(|_| invalid("The header is too long"))?;
    out.write_all(MAGIC)?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(text.as_bytes())?;
    for packed in arrays {
        out.write_all(packed)?;
    }
    Ok(out.flush()?)
}

/// The bytes of one `.rvr` file: the magic, the header's length and text, then the arrays.
pub fn to_bytes(header: &Value, arrays: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    frame(&mut bytes, header, arrays)?;
    Ok(bytes)
}

/// Write one `.rvr` file whose arrays `stream` laid beside it.
pub fn write_laid(path: impl AsRef<Path>, header: &Value, laid: Laid) -> Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    frame(&mut out, header, &[])?;
    for file in &laid.files {
        std::io::copy(&mut std::fs::File::open(file)?, &mut out)?;
        std::fs::remove_file(file)?;
    }
    Ok(out.flush()?)
}

/// Write one `.rvr` file in place; publish it through a staged path.
pub fn write(path: impl AsRef<Path>, header: &Value, arrays: &[Vec<u8>]) -> Result<()> {
    let file = std::fs::File::create(path)?;
    frame(std::io::BufWriter::new(file), header, arrays)
}
