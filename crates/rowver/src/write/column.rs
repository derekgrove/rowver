//! One array packed a chunk at a time (docs/FORMAT.md §7): its values taken in row order, a piece
//! at a time, cut at the file's chunks, each chunk hashed and its lanes compressed, so no array is
//! held whole.

use super::facts::Facts;
use super::spool::Spooled;
use super::Values;
use crate::chunks::{self, Rows};
use crate::numeric::width;
use crate::rvr::Reader;
use crate::{invalid, Result};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::io::Write;
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

/// One piece's values as they are at hand: little-endian, or in byte lanes (every value's first
/// byte, then every value's second, and so on).
pub(super) enum Piece<'a> {
    Raw(Cow<'a, [u8]>),
    Lanes(Vec<Cow<'a, [u8]>>),
}

/// An array's values in row order, held in pieces.
pub(super) trait Pieces {
    /// The kind of the values: `f4`, `i8`, `b1`.
    fn kind(&self) -> &str;
    /// How many pieces hold them.
    fn pieces(&self) -> usize;
    /// How many values piece `piece` holds.
    fn values(&self, piece: usize) -> usize;
    /// The values of piece `piece`, as they are at hand.
    fn piece(&self, piece: usize) -> Result<Piece<'_>>;
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
    fn piece(&self, _piece: usize) -> Result<Piece<'_>> {
        Ok(Piece::Raw(Cow::Borrowed(&self.bytes)))
    }
    fn counts(&self, _piece: usize) -> Result<Option<Cow<'_, [usize]>>> {
        Ok(self.counts.as_deref().map(Cow::Borrowed))
    }
    fn listed(&self) -> bool {
        self.counts.is_some()
    }
}

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

/// Piece `piece` as it is at hand, its lanes refused unless each holds its values.
fn piece<'a>(pieces: &'a dyn Pieces, piece: usize, w: usize) -> Result<Piece<'a>> {
    let held = pieces.piece(piece)?;
    if let Piece::Lanes(lanes) = &held {
        if lanes.len() != w || lanes.iter().any(|lane| lane.len() != pieces.values(piece)) {
            return Err(invalid("A piece's lanes do not hold its values"));
        }
    }
    Ok(held)
}

/// One array being written a chunk at a time: each chunk's lanes a frame each to `out`, then, when
/// it is finished, its chunk table; with no `out`, each chunk only hashed (a derived column's).
struct Chunked<'o> {
    w: usize,
    out: Option<&'o mut dyn Write>,
    table: Vec<u8>,
    hashes: Vec<[u8; 32]>,
    size: usize,
}

impl<'o> Chunked<'o> {
    fn new(kind: &str, out: Option<&'o mut dyn Write>) -> Result<Self> {
        Ok(Self { w: width(kind)?, out, table: Vec::new(), hashes: Vec::new(), size: 0 })
    }

    /// The next chunk's values, little-endian, and its byte lanes when they are at hand.
    fn chunk(&mut self, values: &[u8], lanes: Option<&[Vec<u8>]>) -> Result<()> {
        let digest: [u8; 32] = Sha256::digest(values).into();
        self.hashes.push(digest);
        if let Some(out) = self.out.as_mut() {
            let frames = match lanes {
                Some(lanes) => lanes.iter().map(|lane| crate::codec::pack(lane)).collect(),
                None => chunks::frames(values, self.w),
            };
            for frame in frames {
                out.write_all(&frame)?;
                self.size += frame.len();
                self.table.extend((frame.len() as u64).to_le_bytes());
            }
            self.table.extend_from_slice(&digest);
        }
        Ok(())
    }

    /// Its table written: the record's `at` and `size` when it is written, its `sha256`, and, with
    /// `samples`, its `by_sample` from the chunks of rows `rows`.
    fn finish(mut self, rows: &[Rows], samples: Option<usize>) -> Result<Map<String, Value>> {
        let mut record = Map::new();
        if let Some(out) = self.out.as_mut() {
            out.write_all(&self.table)?;
            record.insert("at".into(), 0.into());
            record.insert("size".into(), (self.size + self.table.len()).into());
        }
        let (sha256, by_sample) = chunks::sums(rows, &self.hashes, samples.unwrap_or(0));
        record.insert("sha256".into(), sha256.into());
        if samples.is_some() {
            record.insert("by_sample".into(), by_sample.into());
        }
        Ok(record)
    }
}

/// A column's values taken in row order, any number of rows at a time, and cut at the file's
/// chunks: a flat column's array (with its facts), or a list column's offsets and content.
struct Cutting<'o> {
    name: String,
    kind: String,
    w: usize,
    rows: Vec<Rows>,
    /// The next chunk, the rows taken so far, and the chunk's values (and counts) gathered: in
    /// byte lanes when the pieces come in lanes, so they are written as they come.
    next: usize,
    taken: usize,
    values: Vec<u8>,
    lanes: Option<Vec<Vec<u8>>>,
    counts: Vec<usize>,
    members: usize,
    facts: Option<Facts>,
    array: Chunked<'o>,
    offsets: Option<Chunked<'o>>,
}

impl<'o> Cutting<'o> {
    /// A column `name` of `kind`, a list column when `listed`, over the chunks `rows`, its array
    /// (a list column's content) to `out` and a list column's offsets to `offsets`.
    fn new(name: &str, kind: &str, listed: bool, rows: &[Rows], out: Option<&'o mut dyn Write>, offsets: Option<&'o mut dyn Write>) -> Result<Self> {
        let measured = out.is_none();
        let offsets = match listed {
            true if measured => Some(Chunked::new("i8", None)?),
            true => Some(Chunked::new("i8", offsets)?),
            false => None,
        };
        Ok(Self {
            name: name.into(),
            kind: kind.into(),
            w: width(kind)?,
            rows: rows.to_vec(),
            next: 0,
            taken: 0,
            values: Vec::new(),
            lanes: None,
            counts: Vec::new(),
            members: 0,
            facts: (!listed).then(|| Facts::new(kind)),
            array: Chunked::new(kind, out)?,
            offsets,
        })
    }

    /// The next rows: their values, and for a list column how many each row holds.
    fn take(&mut self, values: &Piece, counts: Option<&[usize]>) -> Result<()> {
        let (n, truths) = match values {
            Piece::Raw(raw) if raw.len() % self.w != 0 => return Err(invalid(format!("{}: its {} bytes are not whole values of {}, {} bytes each: give it exactly its values' bytes", self.name, raw.len(), self.kind, self.w))),
            Piece::Raw(raw) => (raw.len() / self.w, &raw[..]),
            Piece::Lanes(lanes) => (lanes.first().map_or(0, |lane| lane.len()), lanes.first().map_or(&[][..], |lane| &lane[..])),
        };
        if let Some(byte) = crate::numeric::untrue(&self.kind, truths) {
            return Err(invalid(format!("{}: a b1 value is the byte 0 or 1, and one is {byte}: give false as 0 and true as 1", self.name)));
        }
        let (name, rows) = (self.name.clone(), self.rows.last().map_or(0, |last| last.2));
        let per = if counts.is_some() { "list" } else { "value" };
        let too_many = move || invalid(format!("{name}: it holds more rows than the file's {rows}: give it one {per} per row"));
        let Some(counts) = counts else {
            let mut at = 0;
            while at < n {
                let end = self.rows.get(self.next).ok_or_else(&too_many)?.2;
                let take = (end - self.taken).min(n - at);
                self.gather(values, at, at + take);
                (self.taken, at) = (self.taken + take, at + take);
                if self.taken == end {
                    self.close()?;
                }
            }
            return Ok(());
        };
        let (mut at, mut from) = (0_usize, 0_usize);
        for count in counts {
            let end = self.rows.get(self.next).ok_or_else(&too_many)?.2;
            at = at.checked_add(*count).filter(|to| *to <= n).ok_or_else(|| invalid(format!("{}: its lists hold more values than it has: give it one list per row, the lists together holding every value", self.name)))?;
            self.counts.push(*count);
            self.members = self.members.checked_add(*count).ok_or_else(|| invalid(format!("{}: its lists hold more values than an offset counts: write fewer rows into one file", self.name)))?;
            self.taken += 1;
            if self.taken == end {
                self.gather(values, from, at);
                from = at;
                self.close()?;
            }
        }
        if at != n {
            return Err(invalid(format!("{}: it holds values no row's list holds: give it one list per row, the lists together holding every value", self.name)));
        }
        self.gather(values, from, at);
        Ok(())
    }

    /// Values `from..to` of a piece added to the chunk gathered: in lanes once a piece came in
    /// lanes, whole until then.
    fn gather(&mut self, values: &Piece, from: usize, to: usize) {
        let w = self.w;
        match (values, &mut self.lanes) {
            (Piece::Raw(raw), None) => self.values.extend_from_slice(&raw[from * w..to * w]),
            (Piece::Raw(raw), Some(gathered)) => {
                for (lane, into) in gathered.iter_mut().enumerate() {
                    into.extend(raw[from * w..to * w].iter().skip(lane).step_by(w));
                }
            }
            (Piece::Lanes(lanes), gathered) => {
                let gathered = gathered.get_or_insert_with(|| vec![Vec::new(); w]);
                if !self.values.is_empty() {
                    for (lane, into) in gathered.iter_mut().enumerate() {
                        into.extend(self.values.iter().skip(lane).step_by(w));
                    }
                    self.values.clear();
                }
                for (into, lane) in gathered.iter_mut().zip(lanes) {
                    into.extend_from_slice(&lane[from..to]);
                }
            }
        }
    }

    /// The chunk gathered, hashed and stored: its lanes put back together for its SHA-256 and
    /// facts, when it came in lanes, and written as they are.
    fn close(&mut self) -> Result<()> {
        if let Some(lanes) = &self.lanes {
            self.values = crate::codec::merge(lanes, self.w, lanes.first().map_or(0, Vec::len));
        }
        if let Some(facts) = &mut self.facts {
            facts.add(&self.values)?;
        }
        self.array.chunk(&self.values, self.lanes.as_deref())?;
        self.values.clear();
        for lane in self.lanes.iter_mut().flatten() {
            lane.clear();
        }
        if let Some(offsets) = &mut self.offsets {
            let mut steps = Vec::with_capacity((self.counts.len() + 1) * 8);
            let mut at = 0_i64;
            steps.extend(at.to_le_bytes());
            for count in self.counts.drain(..) {
                at = i64::try_from(count).ok().and_then(|count| at.checked_add(count)).ok_or_else(|| invalid(format!("{}: its lists hold more values than an offset counts (2^63 − 1): write fewer rows into one file", self.name)))?;
                steps.extend(at.to_le_bytes());
            }
            offsets.chunk(&steps, None)?;
        }
        self.next += 1;
        Ok(())
    }

    /// The column's record (after its name): its kind, then its array's, or its offsets' and content's.
    fn finish(self, samples: usize) -> Result<(Map<String, Value>, Vec<usize>)> {
        let rows = self.rows.last().map_or(0, |last| last.2);
        if self.next != self.rows.len() || !self.values.is_empty() || self.lanes.iter().flatten().any(|lane| !lane.is_empty()) {
            return Err(invalid(format!("{}: it holds {} rows, and the file {rows}: give it one value (or list) per row", self.name, self.taken)));
        }
        let mut record = Map::new();
        let Some(offsets) = self.offsets else {
            record.insert("kind".into(), self.kind.clone().into());
            let mut placed = self.array.finish(&self.rows, Some(samples))?;
            let size = placed.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
            record.append(&mut placed);
            if let Some(facts) = self.facts {
                record.append(&mut facts.finish());
            }
            return Ok((record, vec![size]));
        };
        record.insert("kind".into(), "jagged".into());
        let offsets = offsets.finish(&self.rows, Some(samples))?;
        let mut content = Map::new();
        content.insert("kind".into(), self.kind.clone().into());
        content.insert("count".into(), self.members.into());
        content.append(&mut self.array.finish(&self.rows, Some(samples))?);
        let size = |m: &Map<String, Value>| m.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
        let sizes = vec![size(&offsets), size(&content)];
        record.insert("offsets".into(), Value::Object(offsets));
        record.insert("content".into(), Value::Object(content));
        Ok((record, sizes))
    }
}

/// Feed every value of `pieces` to `cutting`, a piece at a time, as each is at hand.
fn fed(pieces: &dyn Pieces, cutting: &mut Cutting) -> Result<()> {
    let w = width(pieces.kind())?;
    for at in 0..pieces.pieces() {
        let values = piece(pieces, at, w)?;
        let counts = pieces.counts(at)?;
        cutting.take(&values, counts.as_deref())?;
    }
    Ok(())
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

/// Pack the `sample` array: each chunk holds its sample's index, which no checksum but its own has.
pub(super) fn sample(rows: &[Rows], out: &mut dyn Write) -> Result<Packed> {
    let mut array = Chunked::new("i4", Some(out))?;
    for (sample, first, end) in rows {
        let index = i32::try_from(*sample).map_err(|_| invalid("Too many samples"))?;
        array.chunk(&index.to_le_bytes().repeat(end - first), None)?;
    }
    let record = array.finish(rows, None)?;
    let size = record["size"].as_u64().unwrap_or(0) as usize;
    Ok(Packed { record, arrays: vec![(size, None)] })
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
pub(super) fn entry(source: Source, rows: &[Rows], samples: usize, files: &[usize], out: &mut dyn Write) -> Result<Packed> {
    let held = match source {
        Source::Values(values) => Held::Whole(Whole::new(values, "entry")?),
        Source::Spooled(spooled) => Held::Spooled(spooled),
        Source::Copied(..) => return Err(invalid("the entry array cannot be copied from another file: give it as values or spooled pieces")),
    };
    let pieces = held.pieces();
    let total: usize = (0..pieces.pieces()).map(|p| pieces.values(p)).sum();
    let end = rows.last().map_or(0, |last| last.2);
    if pieces.listed() || pieces.kind() != "i8" || total != end {
        return Err(invalid(format!("the entry array holds {total} {} values{}, and it holds one i8 value per row, {end}: give each row's entry as an i8", pieces.kind(), if pieces.listed() { " in lists" } else { "" })));
    }
    let mut cutting = Cutting::new("entry", "i8", false, rows, Some(out), None)?;
    cutting.facts = None;
    let (mut row, mut file, mut last) = (0_usize, 0_usize, None::<i64>);
    for at in 0..pieces.pieces() {
        // The entries are checked as values; they are written as they came, in lanes or not.
        let held = piece(pieces, at, 8)?;
        let values = match &held {
            Piece::Raw(raw) => Cow::Borrowed(&raw[..]),
            Piece::Lanes(lanes) => Cow::Owned(crate::codec::merge(lanes, 8, pieces.values(at))),
        };
        for value in values.chunks_exact(8) {
            let entry = i64::from_le_bytes(value.try_into().unwrap_or_default());
            while file + 1 < files.len() && row >= files[file + 1] {
                (file, last) = (file + 1, None);
            }
            // Rows no file of `files` holds are not a file's.
            if file + 1 < files.len() {
                if last.is_some_and(|last| entry <= last) {
                    return Err(invalid(format!("the entries of a file's rows must be strictly increasing, and row {row} holds entry {entry} after a row of entry {}: keep each file's rows in the order of their entries", last.unwrap_or_default())));
                }
                last = Some(entry);
            }
            row += 1;
        }
        drop(values);
        cutting.take(&held, None)?;
    }
    let (mut record, sizes) = cutting.finish(samples)?;
    record.remove("kind");
    Ok(Packed { record, arrays: vec![(sizes[0], None)] })
}

/// Pack one column of the chunks `rows` from its source; a list column's content goes through
/// `scratch` while its offsets are written.
pub(super) fn column(name: &str, source: Source, rows: &[Rows], samples: usize, out: &mut dyn Write, scratch: &Path) -> Result<Packed> {
    let held = match source {
        Source::Copied(reader, from) => return copied(name, reader, &from, rows, out),
        Source::Spooled(spooled) => Held::Spooled(spooled),
        Source::Values(values) => Held::Whole(Whole::new(values, name)?),
    };
    let pieces = held.pieces();
    let mut record = Map::new();
    record.insert("name".into(), name.into());
    if !pieces.listed() {
        let mut cutting = Cutting::new(name, pieces.kind(), false, rows, Some(out), None)?;
        fed(pieces, &mut cutting)?;
        let (mut placed, sizes) = cutting.finish(samples)?;
        record.append(&mut placed);
        return Ok(Packed { record, arrays: vec![(sizes[0], None)] });
    }
    // The content's frames wait in `scratch` while the offsets, which come first, are written.
    let mut content = std::io::BufWriter::new(std::fs::File::options().read(true).write(true).create(true).truncate(true).open(scratch)?);
    let packed = {
        let mut cutting = Cutting::new(name, pieces.kind(), true, rows, Some(&mut content), Some(out))?;
        fed(pieces, &mut cutting)?;
        cutting.finish(samples)?
    };
    let mut file = content.into_inner().map_err(|e| e.into_error())?;
    std::io::Seek::rewind(&mut file)?;
    std::io::copy(&mut file, out)?;
    drop(file);
    let _ = std::fs::remove_file(scratch);
    let (mut placed, sizes) = packed;
    record.append(&mut placed);
    Ok(Packed { record, arrays: vec![(sizes[0], Some("offsets")), (sizes[1], Some("content"))] })
}

/// The record a column of these values would have, its checksums (cut at the chunks `rows`),
/// `by_sample` and facts, with no array written: a derived column's.
pub(super) fn measured(name: &str, column: &crate::rvr::Held, rows: &[Rows], samples: usize) -> Result<Map<String, Value>> {
    let mut measuring = Measuring::new(name, &column.kind, column.offsets.is_some(), rows)?;
    let counts: Option<Vec<usize>> = column.offsets.as_ref().map(|o| o.windows(2).map(|w| w[1] - w[0]).collect());
    measuring.take(&column.bytes, counts.as_deref())?;
    measuring.finish(samples)
}

/// A column measured a few rows at a time, as a derived column's record holds it: its kind, its
/// checksums (cut at the file's chunks) and facts, and no array.
pub(crate) struct Measuring(Cutting<'static>);

impl Measuring {
    /// A column `name` of `kind` (a list column's values' kind, when `listed`) over the chunks `rows`.
    pub(crate) fn new(name: &str, kind: &str, listed: bool, rows: &[Rows]) -> Result<Self> {
        Ok(Self(Cutting::new(name, kind, listed, rows, None, None)?))
    }

    /// The next rows: their values, and for a list column how many each row holds.
    pub(crate) fn take(&mut self, values: &[u8], counts: Option<&[usize]>) -> Result<()> {
        self.0.take(&Piece::Raw(Cow::Borrowed(values)), counts)
    }

    /// The record, from its kind on, every row taken.
    pub(crate) fn finish(self, samples: usize) -> Result<Map<String, Value>> {
        self.0.finish(samples).map(|(record, _)| record)
    }
}

/// An array as another file holds it, copied to `out` byte for byte a chunk at a time, read as
/// the reader reads it: its table checked against its record there, then each chunk decoded,
/// checked and handed to `each` before its frames are written; how many bytes it takes. A table
/// unlike its record is `unlike`, a chunk unlike its table `damaged`.
#[allow(clippy::too_many_arguments)]
fn checked_copy(reader: &Reader, record: &Value, kind: &str, place: &str, n: &dyn Fn(usize) -> usize, out: &mut dyn Write, refused: (&dyn Fn() -> crate::Error, &dyn Fn() -> crate::Error), each: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<usize> {
    let (unlike, damaged) = refused;
    let mapped = |e: crate::Error, to: &dyn Fn() -> crate::Error| match e {
        crate::Error::Invalid(_) => to(),
        e => e,
    };
    let table = reader.chunk_table(record, kind, place).map_err(|e| mapped(e, unlike))?;
    let (mut handed, mut size) = (false, 0);
    let read = reader.frames(record, kind, place, n, None, &mut |_, frames, values| {
        handed = true;
        each(&values)?;
        handed = false;
        out.write_all(frames)?;
        size += frames.len() as u64;
        Ok(())
    });
    read.map_err(|e| if handed { e } else { mapped(e, damaged) })?;
    let end = size + table.bytes();
    out.write_all(&reader.array_bytes(record, place, size, end)?)?;
    Ok(end as usize)
}

/// Copy a column as it lies in another file of the same rows and chunks: its arrays byte for byte,
/// each chunk decoded and checked; its facts made again, and equal to its record's there.
fn copied(name: &str, reader: &Reader, from: &str, rows: &[Rows], out: &mut dyn Write) -> Result<Packed> {
    let Some(held) = reader.record(from).filter(|held| held.is_object()) else {
        return Err(invalid(format!("{name}: the file it is copied from does not hold {from}")));
    };
    if reader.chunks()?.as_slice() != rows {
        return Err(invalid(format!("{name}: the file it is copied from, {}, is not cut into the same chunks: press again with --full", reader.path().display())));
    }
    let mut record = held.as_object().cloned().unwrap_or_default();
    record.insert("name".into(), name.into());
    let path = reader.path().display();
    let unlike = || invalid(format!("{name}: its array in {path} does not hold what its record says. Run rvr validate on that file, or press again with --full."));
    let damaged = || invalid(format!("{name}: its array in {path} is damaged. Run rvr validate on that file, or press again with --full."));
    let place = format!("data.cols[{from:?}]");
    let kind_of = |held: &Value| held["kind"].as_str().unwrap_or("").to_owned();
    let truths = |values: &[u8], kind: &str| match crate::numeric::untrue(kind, values) {
        Some(_) => Err(unlike()),
        None => Ok(()),
    };
    if held["kind"] != "jagged" {
        let kind = kind_of(held);
        let mut facts = Facts::new(&kind);
        let size = checked_copy(reader, held, &kind, &place, &|k| rows[k].2 - rows[k].1, out, (&unlike, &damaged), &mut |values| {
            truths(values, &kind)?;
            facts.add(values)
        })?;
        if facts.finish().iter().any(|(key, value)| &held[key] != value) {
            return Err(unlike());
        }
        return Ok(Packed { record, arrays: vec![(size, None)] });
    }
    let (offsets, content) = (&held["offsets"], &held["content"]);
    let mut members = Vec::with_capacity(rows.len());
    let size = checked_copy(reader, offsets, "i8", &format!("{place}.offsets"), &|k| rows[k].2 - rows[k].1 + 1, out, (&unlike, &damaged), &mut |values| {
        members.push(chunks::offsets(values, name).map_err(|_| unlike())?.1);
        Ok(())
    })?;
    let total = members.iter().try_fold(0_u64, |sum, m| sum.checked_add(*m as u64));
    if total.is_none() || total != content["count"].as_u64() {
        return Err(unlike());
    }
    let kind = kind_of(content);
    let more = checked_copy(reader, content, &kind, &format!("{place}.content"), &|k| members[k], out, (&unlike, &damaged), &mut |values| truths(values, &kind))?;
    Ok(Packed { record, arrays: vec![(size, Some("offsets")), (more, Some("content"))] })
}
