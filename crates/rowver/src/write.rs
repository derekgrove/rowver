//! Build and write an `.rvr` file: a plain JSON header, then each array compressed on its own.
//!
//! Documentation: `FORMAT.md` (repository root).
//! A column is refused, not converted, when its pieces differ in kind; a piece whose column
//! does not have one value per row is refused before joining.

use crate::codec;
use crate::json::canonical;
use crate::numeric::{width, NumericArray};
use crate::rvr::digest;
use crate::rvr::samples::{kind_of, value_checksum, RESERVED};
use crate::{invalid, NumericBuffer, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;

mod derived;
mod facts;

pub use derived::{derived, derived_with, Lineage};

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

/// One array compressed, with the checksum of its plain bytes and the fields recorded after it.
struct Packed {
    bytes: Vec<u8>,
    sha256: String,
    after: Map<String, Value>,
}

/// What is measured of an array besides its own checksum.
enum Measure<'a> {
    Nothing,
    /// `by_sample` over the rows.
    Rows,
    /// A flat column: `by_sample` over the rows, then its facts.
    Column(&'a str, Option<&'a BTreeMap<i128, String>>),
    /// A jagged column's offsets: `by_sample` of each sample's offsets counted from its first.
    Steps(&'a [i64]),
    /// A jagged column's content: `by_sample` between these value bounds.
    Content(Vec<usize>),
}

/// Shuffle, compress, and checksum one array, and measure what its record holds after `sha256`.
fn packed(array: &NumericArray, measure: &Measure, edges: &[usize]) -> Result<Packed> {
    let w = width(&array.kind)?;
    let bytes = codec::pack(&array.bytes, w);
    let mut after = Map::new();
    match measure {
        Measure::Nothing => {}
        Measure::Rows => {
            after.insert("by_sample".into(), by_sample(array, edges)?);
        }
        Measure::Column(name, labels) => {
            after.insert("by_sample".into(), by_sample(array, edges)?);
            after.append(&mut facts::facts(array, *labels, name)?);
        }
        Measure::Steps(offsets) => {
            after.insert("by_sample".into(), by_sample_steps(offsets, edges));
        }
        Measure::Content(bounds) => {
            after.insert("by_sample".into(), by_sample(array, bounds)?);
        }
    }
    Ok(Packed {
        bytes,
        sha256: digest(&array.bytes),
        after,
    })
}

/// Lays compressed arrays one after another and records where each one lies.
struct Packer {
    waiting: std::vec::IntoIter<Packed>,
    arrays: Vec<Vec<u8>>,
    end: usize,
}

impl Packer {
    /// Compress and measure every array of a file at once, on up to `workers` threads, in file order.
    fn of(arrays: &[(&NumericArray, Measure)], edges: &[usize], workers: usize) -> Result<Self> {
        let work = |(array, measure): &(&NumericArray, Measure)| packed(array, measure, edges);
        Ok(Self {
            waiting: crate::parallel::map(arrays, workers, work)?.into_iter(),
            arrays: Vec::new(),
            end: 0,
        })
    }

    /// Place the next array after the ones before it; record its `at`, `size`, and `sha256`.
    fn put(&mut self) -> Result<Map<String, Value>> {
        let next = self.waiting.next();
        let Packed {
            bytes,
            sha256,
            mut after,
        } = next.ok_or_else(|| invalid("An array was not packed"))?;
        let mut record = Map::new();
        record.insert("at".into(), self.end.into());
        record.insert("size".into(), bytes.len().into());
        record.insert("sha256".into(), sha256.into());
        record.append(&mut after);
        self.end += bytes.len();
        self.arrays.push(bytes);
        Ok(record)
    }
}

/// Require a buffer whose bytes are exactly `count` values of a supported kind.
fn checked(buffer: NumericBuffer, name: &str) -> Result<NumericArray> {
    let w = width(&buffer.kind)?;
    if buffer.count.checked_mul(w) != Some(buffer.bytes.len()) {
        return Err(invalid(format!("{name}: bytes do not hold its values")));
    }
    if buffer.kind == "b1" && buffer.bytes.iter().any(|byte| *byte > 1) {
        return Err(invalid(format!("{name}: boolean bytes must be 0 or 1")));
    }
    Ok(buffer.into())
}

/// One sha256 per sample of the values between neighbouring bounds.
fn by_sample(array: &NumericArray, bounds: &[usize]) -> Result<Value> {
    let w = width(&array.kind)?;
    Ok(bounds
        .windows(2)
        .map(|pair| digest(&array.bytes[pair[0] * w..pair[1] * w]))
        .collect())
}

/// One sha256 per sample of a jagged column's offsets, counted from the sample's first value.
fn by_sample_steps(offsets: &[i64], rows: &[usize]) -> Value {
    rows.windows(2)
        .map(|pair| {
            let first = offsets[pair[0]];
            let part: Vec<i64> = offsets[pair[0]..=pair[1]]
                .iter()
                .map(|offset| offset - first)
                .collect();
            digest(&array(&part).bytes)
        })
        .collect()
}

/// One column checked and laid out as the arrays the file will hold.
enum Ready {
    Flat(NumericArray),
    Jagged {
        offsets: Vec<i64>,
        /// The offsets as the `i8` array that is written.
        steps: NumericArray,
        content: NumericArray,
    },
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
    let mut seen = HashSet::new();
    for (index, Sample { name, .. }) in skim.samples.iter().enumerate() {
        if name.is_empty() || !seen.insert(name) {
            return Err(invalid("Sample names must be nonempty and unique"));
        }
        let own = &skim.sample[edges[index]..edges[index + 1]];
        if own
            .iter()
            .any(|sample| usize::try_from(*sample) != Ok(index))
        {
            return Err(invalid("Each row must name the sample whose rows hold it"));
        }
    }
    let sample: NumericArray = array(&skim.sample).into();
    let entry: NumericArray = array(&skim.entry).into();
    let mut names = HashSet::new();
    let mut held = Vec::with_capacity(skim.columns.len());
    for (name, values) in skim.columns {
        if name.is_empty() || !names.insert(name.clone()) {
            return Err(invalid("Column names must be nonempty and unique"));
        }
        let ready = match values {
            Values::Flat(buffer) => {
                let flat = checked(buffer, &name)?;
                if flat.count != rows {
                    return Err(invalid(format!("{name}: needs one value per row")));
                }
                Ready::Flat(flat)
            }
            Values::Jagged { counts, content } => {
                let content = checked(content, &name)?;
                let mut offsets = Vec::with_capacity(counts.len() + 1);
                offsets.push(0_i64);
                for count in &counts {
                    let next = i64::try_from(*count).ok();
                    let next = next.and_then(|count| count.checked_add(offsets[offsets.len() - 1]));
                    offsets.push(next.ok_or_else(|| invalid(format!("{name}: too many values")))?);
                }
                if counts.len() != rows || offsets[rows] as usize != content.count {
                    return Err(invalid(format!(
                        "{name}: needs one list per row, together holding its content"
                    )));
                }
                Ready::Jagged {
                    steps: array(&offsets).into(),
                    offsets,
                    content,
                }
            }
        };
        held.push((name, ready));
    }
    let mut ordered = vec![(&sample, Measure::Nothing), (&entry, Measure::Rows)];
    for (name, ready) in &held {
        match ready {
            Ready::Flat(flat) => ordered.push((flat, Measure::Column(name, skim.labels.get(name)))),
            Ready::Jagged {
                offsets,
                steps,
                content,
            } => {
                let bounds = edges.iter().map(|row| offsets[*row] as usize).collect();
                ordered.extend([
                    (steps, Measure::Steps(offsets)),
                    (content, Measure::Content(bounds)),
                ]);
            }
        }
    }
    let mut packer = Packer::of(&ordered, &edges, workers.max(1))?;
    let mut data = Map::new();
    data.insert("rows".into(), rows.into());
    let samples = skim.samples.iter();
    data.insert(
        "samples".into(),
        samples
            .map(|s| json!({"name": s.name, "label": s.label, "rows": s.rows}))
            .collect(),
    );
    data.insert("files".into(), Value::Array(skim.files));
    data.insert("sample".into(), Value::Object(packer.put()?));
    data.insert("entry".into(), Value::Object(packer.put()?));
    let mut cols = Vec::new();
    for (name, ready) in &held {
        let mut col = Map::new();
        col.insert("name".into(), name.clone().into());
        match ready {
            Ready::Flat(flat) => {
                col.insert("kind".into(), flat.kind.clone().into());
                col.append(&mut packer.put()?);
            }
            Ready::Jagged { content, .. } => {
                col.insert("kind".into(), "jagged".into());
                col.insert("offsets".into(), Value::Object(packer.put()?));
                let mut record = Map::new();
                record.insert("kind".into(), content.kind.clone().into());
                record.insert("count".into(), content.count.into());
                record.append(&mut packer.put()?);
                col.insert("content".into(), Value::Object(record));
            }
        }
        cols.push(Value::Object(col));
    }
    data.insert("cols".into(), Value::Array(cols));
    let mut data = Value::Object(data);
    let checksum = data_checksum(&data)?;
    data["sha256"] = checksum.into();
    Ok((data, packer.arrays))
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

/// Write one `.rvr` file in place; publish it through a staged path.
pub fn write(path: impl AsRef<Path>, header: &Value, arrays: &[Vec<u8>]) -> Result<()> {
    let file = std::fs::File::create(path)?;
    frame(std::io::BufWriter::new(file), header, arrays)
}
