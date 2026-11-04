//! One array packed a piece at a time: measured in row order, then compressed from its
//! byte-shuffled form, so no array is held whole.

use super::facts::Facts;
use super::spool::Spooled;
use super::Values;
use crate::numeric::width;
use crate::rvr::{hex, Reader};
use crate::{invalid, Result};
use ruzstd::decoding::StreamingDecoder;
use ruzstd::encoding::{compress, CompressionLevel};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::io::{Read, Write};
use std::path::Path;

/// Where `stream` takes a column from.
pub enum Source<'a> {
    /// Values held whole.
    Values(Values),
    /// What a spool held of it.
    Spooled(Spooled<'a>),
    /// Copied as it lies in a file of the same rows: its arrays, record and checksums.
    Copied(&'a Reader, String),
}

impl Source<'_> {
    /// About how many bytes packing it handles, to start the heaviest first.
    pub(super) fn weight(&self) -> usize {
        match self {
            Source::Values(Values::Flat(buffer)) => buffer.bytes.len(),
            Source::Values(Values::Jagged { counts, content }) => {
                counts.len() * 8 + content.bytes.len()
            }
            Source::Spooled(spooled) => spooled.weight(),
            // Decoded and measured again: about as much work as packing its values.
            Source::Copied(reader, name) => {
                let record = reader.record(name);
                let size = |key: &str| {
                    record.and_then(|r| r.get(key)).map_or(0, |held| {
                        held.get("size").and_then(Value::as_u64).unwrap_or(0) as usize
                    })
                };
                let flat = record.and_then(|r| r["size"].as_u64()).unwrap_or(0) as usize;
                3 * (flat + size("offsets") + size("content"))
            }
        }
    }
}

/// An array's values in row order, held in pieces.
pub(super) trait Pieces {
    /// The kind of the values: `f4`, `i8`, `b1`.
    fn kind(&self) -> &str;
    /// How many pieces hold them.
    fn pieces(&self) -> usize;
    /// How many values piece `piece` holds.
    fn values(&self, piece: usize) -> usize;
    /// Byte `lane` of every value of piece `piece`, in order.
    fn lane(&self, piece: usize, lane: usize) -> Result<Cow<'_, [u8]>>;
    /// The little-endian values of piece `piece`, when they are at hand without unshuffling.
    fn raw(&self, _piece: usize) -> Result<Option<Cow<'_, [u8]>>> {
        Ok(None)
    }
    /// For a list column, how many values each row of piece `piece` holds.
    fn counts(&self, _piece: usize) -> Result<Option<Cow<'_, [usize]>>> {
        Ok(None)
    }
    /// Whether it is a list column.
    fn listed(&self) -> bool {
        false
    }
}

/// Values held whole, or lent: one piece.
pub(super) struct Whole<'a> {
    kind: String,
    w: usize,
    bytes: Cow<'a, [u8]>,
    counts: Option<Cow<'a, [usize]>>,
}

impl Whole<'_> {
    /// Require bytes that are exactly the values of a supported kind.
    pub(super) fn new(values: Values, name: &str) -> Result<Self> {
        let (buffer, counts) = values.parts();
        let w = width(&buffer.kind)?;
        if buffer.count.checked_mul(w) != Some(buffer.bytes.len()) {
            return Err(invalid(format!("{name}: its {} bytes are not {} values of {}, {w} bytes each: give it exactly its values' bytes", buffer.bytes.len(), buffer.count, buffer.kind)));
        }
        Ok(Self {
            kind: buffer.kind,
            w,
            bytes: Cow::Owned(buffer.bytes),
            counts: counts.map(Cow::Owned),
        })
    }
}

impl Pieces for Whole<'_> {
    fn kind(&self) -> &str {
        &self.kind
    }
    fn pieces(&self) -> usize {
        1
    }
    fn values(&self, _piece: usize) -> usize {
        self.bytes.len() / self.w
    }
    fn lane(&self, _piece: usize, lane: usize) -> Result<Cow<'_, [u8]>> {
        Ok(match self.w {
            1 => Cow::Borrowed(&self.bytes),
            w => Cow::Owned(self.bytes.iter().skip(lane).step_by(w).copied().collect()),
        })
    }
    fn raw(&self, _piece: usize) -> Result<Option<Cow<'_, [u8]>>> {
        Ok(Some(Cow::Borrowed(&self.bytes)))
    }
    fn counts(&self, _piece: usize) -> Result<Option<Cow<'_, [usize]>>> {
        Ok(self.counts.as_deref().map(Cow::Borrowed))
    }
    fn listed(&self) -> bool {
        self.counts.is_some()
    }
}

/// How many values are unshuffled, or read of a decoded array, at a time.
const AT_ONCE: usize = 1 << 16;

/// Read `buffer.len()` bytes of `file` from `at`, without moving a shared cursor.
pub(super) fn read_at(file: &std::fs::File, buffer: &mut [u8], at: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::FileExt::read_exact_at(file, buffer, at);
    #[cfg(windows)]
    {
        let (mut buffer, mut at) = (buffer, at);
        while !buffer.is_empty() {
            match std::os::windows::fs::FileExt::seek_read(file, buffer, at)? {
                0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                n => (buffer, at) = (&mut buffer[n..], at + n as u64),
            }
        }
        Ok(())
    }
}

/// Write `bytes` into `file` at `at`, without moving a shared cursor.
pub(super) fn write_at(file: &std::fs::File, bytes: &[u8], at: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::FileExt::write_all_at(file, bytes, at);
    #[cfg(windows)]
    {
        let (mut bytes, mut at) = (bytes, at);
        while !bytes.is_empty() {
            match std::os::windows::fs::FileExt::seek_write(file, bytes, at)? {
                0 => return Err(std::io::ErrorKind::WriteZero.into()),
                n => (bytes, at) = (&bytes[n..], at + n as u64),
            }
        }
        Ok(())
    }
}

/// Each row's index into the samples, as `i4`, in pieces of at most `AT_ONCE` rows.
pub(super) struct Indices(Vec<(i32, usize)>);

impl Indices {
    /// The pieces of samples of these many rows each.
    pub(super) fn new(rows: &[usize]) -> Result<Self> {
        let mut pieces = Vec::new();
        for (sample, rows) in rows.iter().enumerate() {
            let sample = i32::try_from(sample).map_err(|_| invalid("Too many samples"))?;
            for start in (0..*rows).step_by(AT_ONCE) {
                pieces.push((sample, AT_ONCE.min(rows - start)));
            }
        }
        Ok(Self(pieces))
    }
}

impl Pieces for Indices {
    fn kind(&self) -> &str {
        "i4"
    }
    fn pieces(&self) -> usize {
        self.0.len()
    }
    fn values(&self, piece: usize) -> usize {
        self.0[piece].1
    }
    fn lane(&self, piece: usize, lane: usize) -> Result<Cow<'_, [u8]>> {
        let (sample, rows) = self.0[piece];
        Ok(Cow::Owned(vec![sample.to_le_bytes()[lane]; rows]))
    }
}

/// An array decoded into a scratch file in its byte-shuffled order, read as pieces of
/// `AT_ONCE` values.
struct Decoded {
    file: std::fs::File,
    kind: String,
    count: usize,
}

impl Decoded {
    /// Value `index` of an `i8` array.
    fn signed(&self, index: usize) -> Result<i64> {
        let mut value = [0; 8];
        for (lane, byte) in value.iter_mut().enumerate() {
            let mut one = [0];
            read_at(&self.file, &mut one, (lane * self.count + index) as u64)?;
            *byte = one[0];
        }
        Ok(i64::from_le_bytes(value))
    }
}

impl Pieces for Decoded {
    fn kind(&self) -> &str {
        &self.kind
    }
    fn pieces(&self) -> usize {
        self.count.div_ceil(AT_ONCE)
    }
    fn values(&self, piece: usize) -> usize {
        AT_ONCE.min(self.count - piece * AT_ONCE)
    }
    fn lane(&self, piece: usize, lane: usize) -> Result<Cow<'_, [u8]>> {
        let mut bytes = vec![0; self.values(piece)];
        read_at(
            &self.file,
            &mut bytes,
            (lane * self.count + piece * AT_ONCE) as u64,
        )?;
        Ok(Cow::Owned(bytes))
    }
}

/// A list column's offsets, counted from its pieces' counts: one more value than rows.
struct Steps<'a> {
    column: &'a dyn Pieces,
    /// Each piece's first offset, then the last offset.
    bases: Vec<i64>,
    /// Each piece's rows.
    rows: Vec<usize>,
}

impl Steps<'_> {
    /// The offsets piece `piece` begins, as values.
    fn offsets(&self, piece: usize) -> Result<Vec<i64>> {
        if piece == self.bases.len() - 1 {
            return Ok(vec![self.bases[piece]]);
        }
        let counts = self.column.counts(piece)?.unwrap_or_default();
        let mut offset = self.bases[piece];
        Ok(counts
            .iter()
            .map(|count| {
                let at = offset;
                offset += *count as i64;
                at
            })
            .collect())
    }
}

impl Pieces for Steps<'_> {
    fn kind(&self) -> &str {
        "i8"
    }
    fn pieces(&self) -> usize {
        self.bases.len()
    }
    fn values(&self, piece: usize) -> usize {
        self.rows.get(piece).copied().unwrap_or(1)
    }
    fn lane(&self, piece: usize, lane: usize) -> Result<Cow<'_, [u8]>> {
        let offsets = self.offsets(piece)?;
        Ok(Cow::Owned(
            offsets.iter().map(|o| o.to_le_bytes()[lane]).collect(),
        ))
    }
    fn raw(&self, piece: usize) -> Result<Option<Cow<'_, [u8]>>> {
        let offsets = self.offsets(piece)?;
        Ok(Some(Cow::Owned(
            offsets.iter().flat_map(|o| o.to_le_bytes()).collect(),
        )))
    }
}

/// What is measured of an array besides its own checksum.
pub(super) enum Measure<'a> {
    Nothing,
    /// `by_sample` between these value bounds.
    Bounds(&'a [usize]),
    /// `by_sample` between these row bounds, then the column's facts.
    Column(&'a [usize]),
    /// `by_sample` of each sample's offsets counted from its first; the samples' row bounds.
    Steps(&'a [usize]),
    /// The entry array: `by_sample` between the samples' row bounds, each file's entries (between
    /// the second bounds) strictly increasing.
    Entries(&'a [usize], &'a [usize]),
}

/// The checksums of an array taken in row order, a piece at a time.
struct Meter<'a> {
    name: &'a str,
    kind: &'a str,
    w: usize,
    measure: &'a Measure<'a>,
    seen: usize,
    total: Sha256,
    /// The sample being hashed, and its hash so far.
    at: usize,
    sample: Sha256,
    /// For offsets, the first offset of the sample being hashed, and the last offset seen.
    first: Option<i64>,
    last: Option<i64>,
    by_sample: Vec<Value>,
    facts: Option<Facts>,
    /// For the entry array, the file being read and its last entry.
    file: (usize, Option<i64>),
}

impl<'a> Meter<'a> {
    fn new(name: &'a str, kind: &'a str, measure: &'a Measure<'a>) -> Result<Self> {
        Ok(Self {
            name,
            kind,
            w: width(kind)?,
            measure,
            seen: 0,
            total: Sha256::new(),
            at: 0,
            sample: Sha256::new(),
            first: None,
            last: None,
            by_sample: Vec::new(),
            facts: matches!(measure, Measure::Column(..)).then(|| Facts::new(kind)),
            file: (0, None),
        })
    }

    /// Close the sample being hashed.
    fn close(&mut self) {
        self.by_sample
            .push(hex(&self.sample.finalize_reset()).into());
        self.at += 1;
        self.first = None;
    }

    /// See the next values, as little-endian bytes.
    fn add(&mut self, bytes: &[u8]) -> Result<()> {
        if self.kind == "b1" {
            if let Some(byte) = bytes.iter().find(|byte| **byte > 1) {
                return Err(invalid(format!("{}: a b1 value is the byte 0 or 1, and one is {byte}: give false as 0 and true as 1", self.name)));
            }
        }
        self.total.update(bytes);
        if let Some(facts) = &mut self.facts {
            facts.add(bytes)?;
        }
        let name = self.name;
        let too_many = || invalid(format!("{name}: holds more values than its rows"));
        let bounds = match self.measure {
            Measure::Nothing => {
                self.seen += bytes.len() / self.w;
                return Ok(());
            }
            Measure::Steps(edges) if edges.len() < 2 => {
                self.seen += bytes.len() / self.w;
                return Ok(());
            }
            Measure::Steps(edges) => {
                for value in bytes.chunks_exact(8) {
                    let offset = i64::from_le_bytes(value.try_into().unwrap());
                    if self.last.map_or(offset != 0, |last| offset < last) {
                        return Err(invalid(format!(
                            "{name}: offsets must start at zero and not decrease"
                        )));
                    }
                    self.last = Some(offset);
                    let row = self.seen;
                    self.seen += 1;
                    // Each sample's offsets run from its first row through its last row's end,
                    // so the offset between two samples belongs to both.
                    loop {
                        if self.at + 1 >= edges.len() {
                            return Err(too_many());
                        }
                        let first = *self.first.get_or_insert(offset);
                        self.sample.update((offset - first).to_le_bytes());
                        if row != edges[self.at + 1] {
                            break;
                        }
                        self.close();
                        if self.at + 1 == edges.len() {
                            break;
                        }
                    }
                }
                return Ok(());
            }
            Measure::Entries(bounds, files) => {
                for (at, value) in bytes.chunks_exact(8).enumerate() {
                    let (row, entry) = (self.seen + at, i64::from_le_bytes(value.try_into().unwrap()));
                    while self.file.0 + 1 < files.len() && row >= files[self.file.0 + 1] {
                        self.file = (self.file.0 + 1, None);
                    }
                    // Rows no file of `files` holds are not a file's.
                    if self.file.0 + 1 >= files.len() {
                        continue;
                    }
                    if self.file.1.is_some_and(|last| entry <= last) {
                        return Err(invalid(format!("the entries of a file's rows must be strictly increasing, and row {row} holds entry {entry} after a row of entry {}: keep each file's rows in the order of their entries", self.file.1.unwrap_or_default())));
                    }
                    self.file.1 = Some(entry);
                }
                *bounds
            }
            Measure::Bounds(bounds) | Measure::Column(bounds) => *bounds,
        };
        let mut rest = bytes;
        while !rest.is_empty() {
            while self.at + 1 < bounds.len() && self.seen == bounds[self.at + 1] {
                self.close();
            }
            if self.at + 1 >= bounds.len() {
                return Err(too_many());
            }
            let take = ((bounds[self.at + 1] - self.seen) * self.w).min(rest.len());
            self.sample.update(&rest[..take]);
            rest = &rest[take..];
            self.seen += take / self.w;
        }
        Ok(())
    }

    /// The array's checksum, then what its record holds after it.
    fn finish(mut self) -> Result<(String, Map<String, Value>)> {
        let mut after = Map::new();
        let bounds = match self.measure {
            Measure::Nothing => None,
            Measure::Bounds(bounds) | Measure::Column(bounds) | Measure::Steps(bounds) | Measure::Entries(bounds, _) => {
                Some(*bounds)
            }
        };
        if let Some(bounds) = bounds {
            if matches!(self.measure, Measure::Steps(_)) {
                if self.at + 1 != bounds.len() && bounds.len() > 1 {
                    return Err(invalid(format!("{}: offsets end early", self.name)));
                }
            } else {
                while self.at + 1 < bounds.len() {
                    self.close();
                }
            }
            after.insert("by_sample".into(), Value::Array(self.by_sample));
        }
        if let Some(facts) = self.facts {
            after.append(&mut facts.finish());
        }
        Ok((hex(&self.total.finalize()), after))
    }
}

/// Every value of an array, little-endian, a piece at a time.
fn each_raw(
    pieces: &dyn Pieces,
    w: usize,
    mut each: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut buffer = Vec::new();
    for piece in 0..pieces.pieces() {
        if let Some(raw) = pieces.raw(piece)? {
            each(&raw)?;
            continue;
        }
        let lanes = (0..w)
            .map(|lane| pieces.lane(piece, lane))
            .collect::<Result<Vec<_>>>()?;
        let count = pieces.values(piece);
        if lanes.iter().any(|lane| lane.len() != count) {
            return Err(invalid("A piece's lanes do not hold its values"));
        }
        for start in (0..count).step_by(AT_ONCE) {
            let end = (start + AT_ONCE).min(count);
            buffer.clear();
            buffer.resize((end - start) * w, 0);
            for (lane, bytes) in lanes.iter().enumerate() {
                let to = buffer[lane..].iter_mut().step_by(w);
                to.zip(&bytes[start..end])
                    .for_each(|(to, byte)| *to = *byte);
            }
            each(&buffer)?;
        }
    }
    Ok(())
}

/// The byte-shuffled form of an array, lane by lane and piece by piece, as one stream; the first
/// failure ends it and is kept.
struct Shuffled<'a> {
    pieces: &'a dyn Pieces,
    w: usize,
    next: (usize, usize),
    held: Cow<'a, [u8]>,
    at: usize,
    failed: Option<crate::Error>,
}

impl Read for Shuffled<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.at == self.held.len() {
            let (lane, piece) = self.next;
            if lane == self.w || self.pieces.pieces() == 0 || self.failed.is_some() {
                return Ok(0);
            }
            self.next = match piece + 1 == self.pieces.pieces() {
                true => (lane + 1, 0),
                false => (lane, piece + 1),
            };
            match self.pieces.lane(piece, lane) {
                Ok(held) => (self.held, self.at) = (held, 0),
                Err(error) => self.failed = Some(error),
            }
        }
        let n = out.len().min(self.held.len() - self.at);
        out[..n].copy_from_slice(&self.held[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A writer that counts what it is given and keeps its first failure instead of returning it.
struct Counted<'a> {
    out: &'a mut dyn Write,
    size: usize,
    failed: Option<std::io::Error>,
}

impl Write for Counted<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.failed.is_none() {
            match self.out.write_all(bytes) {
                Ok(()) => self.size += bytes.len(),
                Err(error) => self.failed = Some(error),
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Measure one array and write its compressed form to `out`: its record's `size`, `sha256` and
/// what follows them. With no `out`, the array is measured and not written: its record has no
/// `at` or `size`.
fn array<'w>(
    name: &str,
    pieces: &dyn Pieces,
    measure: &Measure,
    out: Option<&mut (dyn Write + 'w)>,
) -> Result<Map<String, Value>> {
    let kind = pieces.kind();
    let w = width(kind)?;
    let mut meter = Meter::new(name, kind, measure)?;
    each_raw(pieces, w, |bytes| meter.add(bytes))?;
    let (sha256, mut after) = meter.finish()?;
    let Some(out) = out else {
        let mut record = Map::new();
        record.insert("sha256".into(), sha256.into());
        record.append(&mut after);
        return Ok(record);
    };
    let mut source = Shuffled {
        pieces,
        w,
        next: (0, 0),
        held: Cow::Borrowed(&[]),
        at: 0,
        failed: None,
    };
    let mut drain = Counted {
        out,
        size: 0,
        failed: None,
    };
    compress(&mut source, &mut drain, CompressionLevel::Fastest);
    if let Some(error) = source.failed {
        return Err(error);
    }
    if let Some(error) = drain.failed {
        return Err(error.into());
    }
    let mut record = Map::new();
    record.insert("at".into(), 0.into());
    record.insert("size".into(), drain.size.into());
    record.insert("sha256".into(), sha256.into());
    record.append(&mut after);
    Ok(record)
}

/// A packed column or array: its record, whose arrays are given their `at` when placed.
pub(super) struct Packed {
    pub(super) record: Map<String, Value>,
    /// Each array's size, and the member of the record that holds it (`None`: the record itself).
    pub(super) arrays: Vec<(usize, Option<&'static str>)>,
}

impl Packed {
    /// Place the arrays after the `end` bytes before them.
    pub(super) fn place(&mut self, end: &mut usize) {
        for (size, member) in &self.arrays {
            let record = match member {
                None => &mut self.record,
                Some(member) => self.record[*member]
                    .as_object_mut()
                    .expect("an array record"),
            };
            record.insert("at".into(), (*end).into());
            *end += size;
        }
    }
}

/// Pack the `sample` or `entry` array, which has no name in the file.
pub(super) fn plain(
    label: &str,
    pieces: &dyn Pieces,
    measure: &Measure,
    out: &mut dyn Write,
) -> Result<Packed> {
    let record = array(label, pieces, measure, Some(out))?;
    let size = record["size"].as_u64().unwrap_or(0) as usize;
    Ok(Packed {
        record,
        arrays: vec![(size, None)],
    })
}

/// The pieces a source holds.
enum Held<'a> {
    Whole(Whole<'a>),
    Spooled(Spooled<'a>),
}

impl Held<'_> {
    fn pieces(&self) -> &dyn Pieces {
        match self {
            Held::Whole(whole) => whole,
            Held::Spooled(spooled) => spooled,
        }
    }
}

/// Pack the `entry` array, which must hold one `i8` value per row, from its source; `files` are
/// the files' row bounds, within each of which the entries strictly increase.
pub(super) fn entry(source: Source, edges: &[usize], files: &[usize], out: &mut dyn Write) -> Result<Packed> {
    let held = match source {
        Source::Values(values) => Held::Whole(Whole::new(values, "entry")?),
        Source::Spooled(spooled) => Held::Spooled(spooled),
        Source::Copied(..) => return Err(invalid("the entry array cannot be copied from another file: give it as values or spooled pieces")),
    };
    let pieces = held.pieces();
    let total: usize = (0..pieces.pieces()).map(|p| pieces.values(p)).sum();
    if pieces.listed() || pieces.kind() != "i8" || total != edges[edges.len() - 1] {
        return Err(invalid(format!("the entry array holds {total} {} values{}, and it holds one i8 value per row, {}: give each row's entry as an i8", pieces.kind(), if pieces.listed() { " in lists" } else { "" }, edges[edges.len() - 1])));
    }
    plain("entry", pieces, &Measure::Entries(edges, files), out)
}

/// Pack one column of the rows bounded by `edges` from its source.
pub(super) fn column(
    name: &str,
    source: Source,
    edges: &[usize],
    out: &mut dyn Write,
    scratch: &Path,
) -> Result<Packed> {
    let held = match source {
        Source::Copied(reader, from) => {
            let copy = Copy {
                name,
                reader,
                from: &from,
                edges,
                scratch,
            };
            return copied(&copy, out);
        }
        Source::Spooled(spooled) => Held::Spooled(spooled),
        Source::Values(values) => Held::Whole(Whole::new(values, name)?),
    };
    laid(name, held.pieces(), edges, Some(out))
}

/// The record a column of these values would have, its checksums, `by_sample` and facts, with
/// no array written: a derived column's. `edges` are the samples' row bounds.
pub(super) fn measured(name: &str, column: &crate::rvr::Held, edges: &[usize]) -> Result<Map<String, Value>> {
    let w = width(&column.kind)?;
    if !column.bytes.len().is_multiple_of(w) {
        return Err(invalid(format!("{name}: bytes do not hold its values")));
    }
    let counts = column.offsets.as_ref().map(|o| Cow::Owned(o.windows(2).map(|w| w[1] - w[0]).collect()));
    let whole = Whole {
        kind: column.kind.clone(),
        w,
        bytes: Cow::Borrowed(&column.bytes),
        counts,
    };
    Ok(laid(name, &whole, edges, None)?.record)
}

/// Measure a column's pieces and, with `out`, write their arrays.
fn laid<'w>(
    name: &str,
    pieces: &dyn Pieces,
    edges: &[usize],
    mut out: Option<&mut (dyn Write + 'w)>,
) -> Result<Packed> {
    let rows = edges[edges.len() - 1];
    let total: usize = (0..pieces.pieces()).map(|p| pieces.values(p)).sum();
    let size = |record: &Map<String, Value>| record.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut record = Map::new();
    record.insert("name".into(), name.into());
    if !pieces.listed() {
        if total != rows {
            return Err(invalid(format!("{name}: it holds {total} values for {rows} rows: give it one value per row")));
        }
        record.insert("kind".into(), pieces.kind().into());
        let mut placed = array(name, pieces, &Measure::Column(edges), out)?;
        let arrays = vec![(size(&placed), None)];
        record.append(&mut placed);
        return Ok(Packed { record, arrays });
    }
    let mut bases = Vec::with_capacity(pieces.pieces() + 1);
    let mut held = Vec::with_capacity(pieces.pieces());
    let (mut row, mut offset, mut bounds) = (0_usize, 0_i64, Vec::with_capacity(edges.len()));
    let too_many = || invalid(format!("{name}: its lists hold more values than an offset counts (2^63 − 1): write fewer rows into one file"));
    for piece in 0..pieces.pieces() {
        bases.push(offset);
        let counts = pieces.counts(piece)?.unwrap_or_default();
        held.push(counts.len());
        for count in counts.iter() {
            while bounds.len() < edges.len() && edges[bounds.len()] == row {
                bounds.push(offset as usize);
            }
            let count = i64::try_from(*count).map_err(|_| too_many())?;
            offset = offset.checked_add(count).ok_or_else(too_many)?;
            row += 1;
        }
    }
    bases.push(offset);
    while bounds.len() < edges.len() && edges[bounds.len()] == row {
        bounds.push(offset as usize);
    }
    if row != rows || offset as usize != total || bounds.len() != edges.len() {
        return Err(invalid(format!("{name}: it holds {row} lists of {total} values for {rows} rows: give it one list per row, the lists together holding every value")));
    }
    let steps = Steps {
        column: pieces,
        bases,
        rows: held,
    };
    record.insert("kind".into(), "jagged".into());
    let offsets = array(
        &format!("{name} offsets"),
        &steps,
        &Measure::Steps(edges),
        out.as_deref_mut(),
    )?;
    let mut content = Map::new();
    content.insert("kind".into(), pieces.kind().into());
    content.insert("count".into(), total.into());
    let mut placed = array(name, pieces, &Measure::Bounds(&bounds), out)?;
    content.append(&mut placed);
    let arrays = vec![
        (size(&offsets), Some("offsets")),
        (size(&content), Some("content")),
    ];
    record.insert("offsets".into(), Value::Object(offsets));
    record.insert("content".into(), Value::Object(content));
    Ok(Packed { record, arrays })
}

/// A `Read` over a span of a file that copies every byte it gives to `out`.
/// A failure to write it is kept apart, so it is not taken for damage in what was read.
struct Tee<'a, R: Read> {
    from: R,
    out: &'a mut dyn Write,
    copied: usize,
    failed: Option<std::io::Error>,
}

impl<R: Read> Read for Tee<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.from.read(buffer)?;
        if let Err(error) = self.out.write_all(&buffer[..n]) {
            self.failed = Some(error);
            return Err(std::io::Error::other("the copy could not be written"));
        }
        self.copied += n;
        Ok(n)
    }
}

/// A column to copy as it lies in another file of the same rows, and where to check it.
struct Copy<'a> {
    name: &'a str,
    reader: &'a Reader,
    from: &'a str,
    edges: &'a [usize],
    /// A file to decode each array into while it is checked.
    scratch: &'a Path,
}

/// Copy one array to `out` as it lies, decoding it into the scratch file: its frame must
/// carry its own checksum and decode to `values` values of `kind`.
fn decoded(
    copy: &Copy,
    held: &Value,
    kind: &str,
    values: usize,
    out: &mut dyn Write,
) -> Result<Decoded> {
    let damaged = || {
        invalid(format!(
            "{}: its array in {} is damaged. Run rvr validate on that file, or press again with --full.",
            copy.name,
            copy.reader.path().display()
        ))
    };
    let (file, size) = copy.reader.span(held, copy.name)?;
    let mut start = file.clone();
    let expected = values.checked_mul(width(kind)?).ok_or_else(damaged)?;
    if expected > crate::codec::most(size as usize) {
        return Err(damaged());
    }
    let mut head = vec![0; (size as usize).min(18)];
    start.read_exact(&mut head)?;
    crate::codec::frame(&head, expected, copy.name).map_err(|_| damaged())?;
    let mut tee = Tee {
        from: file,
        out,
        copied: 0,
        failed: None,
    };
    let scratch = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(copy.scratch)?;
    // Decoded no further than one byte past what the record states, so a damaged frame cannot
    // fill the disk; reading and decoding fail as damage, writing as itself.
    let (mut into, mut decoded) = (std::io::BufWriter::new(&scratch), 0_usize);
    let sound = {
        let mut decoder = match StreamingDecoder::new(&mut tee) {
            Ok(decoder) => decoder,
            Err(_) => return Err(tee.failed.take().map_or_else(damaged, Into::into)),
        };
        let mut buffer = vec![0; 1 << 16];
        loop {
            let room = buffer.len().min(expected + 1 - decoded);
            let n = match decoder.read(&mut buffer[..room]) {
                Ok(n) => n,
                Err(_) => break false,
            };
            into.write_all(&buffer[..n])?;
            decoded += n;
            if n == 0 || decoded > expected {
                let frame = &decoder.decoder;
                break frame.get_checksum_from_data().is_some()
                    && frame.get_checksum_from_data() == frame.get_calculated_checksum();
            }
        }
    };
    if let Some(error) = tee.failed.take() {
        return Err(error.into());
    }
    into.flush()?;
    drop(into);
    let after = std::io::copy(&mut tee, &mut std::io::sink())?;
    if let Some(error) = tee.failed.take() {
        return Err(error.into());
    }
    if !sound || decoded != expected || after != 0 || tee.copied as u64 != size {
        return Err(damaged());
    }
    Ok(Decoded {
        file: scratch,
        kind: kind.to_owned(),
        count: values,
    })
}

/// Copy a column as it lies in another file of the same rows. Each array is decoded and measured
/// again, and must match its record there in `sha256`, `by_sample` and facts, as a full read of
/// that file checks it.
fn copied(copy: &Copy, out: &mut dyn Write) -> Result<Packed> {
    let name = copy.name;
    let Some(Value::Object(found)) = copy.reader.record(copy.from) else {
        return Err(invalid(format!(
            "{name}: the file it is copied from does not hold {}",
            copy.from
        )));
    };
    let mut record = found.clone();
    record.insert("name".into(), name.into());
    let rows = copy.edges[copy.edges.len() - 1];
    let unlike = || {
        invalid(format!(
            "{name}: its array in {} does not hold what its record says. Run rvr validate on that file, or press again with --full.",
            copy.reader.path().display()
        ))
    };
    let check = |held: &Value,
                 kind: &str,
                 values: usize,
                 measure: &Measure,
                 out: &mut dyn Write|
     -> Result<(Decoded, usize)> {
        let array = decoded(copy, held, kind, values, out)?;
        let mut meter = Meter::new(name, kind, measure)?;
        each_raw(&array, width(kind)?, |bytes| meter.add(bytes))?;
        let (sha256, after) = meter.finish()?;
        if held["sha256"] != sha256.as_str() || after.iter().any(|(key, value)| &held[key] != value)
        {
            return Err(unlike());
        }
        Ok((array, held["size"].as_u64().unwrap_or(0) as usize))
    };
    let text = |held: &Value, key: &str| held[key].as_str().unwrap_or("").to_owned();
    let result = (|| -> Result<Vec<(usize, Option<&'static str>)>> {
        if record["kind"] != "jagged" {
            let held = Value::Object(record.clone());
            let kind = text(&held, "kind");
            let (_, size) = check(
                &held,
                &kind,
                rows,
                &Measure::Column(copy.edges),
                out,
            )?;
            return Ok(vec![(size, None)]);
        }
        let offsets = record.get("offsets").cloned().ok_or_else(unlike)?;
        let content = record.get("content").cloned().ok_or_else(unlike)?;
        let (steps, size) = check(&offsets, "i8", rows + 1, &Measure::Steps(copy.edges), out)?;
        let bounds = copy
            .edges
            .iter()
            .map(|row| steps.signed(*row).map(|offset| offset as usize))
            .collect::<Result<Vec<_>>>()?;
        drop(steps);
        let count = content["count"].as_u64().unwrap_or(0) as usize;
        let ordered = bounds.windows(2).all(|pair| pair[0] <= pair[1]);
        if !ordered || bounds.first() != Some(&0) || bounds.last() != Some(&count) {
            return Err(unlike());
        }
        let (_, more) = check(
            &content,
            &text(&content, "kind"),
            count,
            &Measure::Bounds(&bounds),
            out,
        )?;
        Ok(vec![(size, Some("offsets")), (more, Some("content"))])
    })();
    let _ = std::fs::remove_file(copy.scratch);
    Ok(Packed {
        record,
        arrays: result?,
    })
}
