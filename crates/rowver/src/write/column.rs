//! One array packed a chunk at a time (docs/FORMAT.md §7): its values taken in row order, a piece
//! at a time, cut at the file's chunks, each chunk hashed and its lanes compressed, so no array is
//! held whole.

use super::facts::Facts;
use super::spool::Spooled;
use super::Values;
use crate::chunks::{self, Frames, Rows};
use crate::numeric::width;
use crate::rvr::Reader;
use crate::{invalid, Result};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use crate::parallel::{Pending, Pool};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// Where `stream` takes a column from.
pub enum Source<'a> {
    /// Values held whole.
    Values(Values),
    /// What a spool held of it.
    Spooled(Spooled<'a>),
    /// Copied as it lies in a file of the same rows: its arrays, record and checksums.
    Copied(&'a Reader, String),
}

/// One piece's values as they are at hand: little-endian, or in byte lanes (every value's first
/// byte, then every value's second, and so on).
pub(super) enum Piece<'a> {
    Raw(Cow<'a, [u8]>),
    Lanes(Vec<Cow<'a, [u8]>>),
}

/// An array's values in row order, held in pieces, read on any thread.
pub(super) trait Pieces: Sync {
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
pub(crate) fn read_at(file: &std::fs::File, buffer: &mut [u8], at: u64) -> std::io::Result<()> {
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
pub(crate) fn write_at(file: &std::fs::File, bytes: &[u8], at: u64) -> std::io::Result<()> {
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

/// Where arrays are written: a file from byte 8 + `at` on, `at` (docs/FORMAT.md §7) the place of
/// the next byte. Chunks are handed over as they are cut, packed on a pool's threads, and written
/// in the order they were handed over: so the next arrays are packed while earlier ones wait.
pub(crate) struct Out {
    file: std::io::BufWriter<Placed>,
    pub(crate) at: u64,
    arrays: Vec<Array>,
    coming: VecDeque<Coming>,
    /// How many chunks wait among what is coming, and their values' bytes.
    chunks: usize,
    waiting: usize,
    /// The offsets arrays written or known, by `sha256`: each one's record, for the next list
    /// column of the same offsets to share (docs/FORMAT.md §7).
    shared: HashMap<String, Map<String, Value>>,
}

/// An array written through `Out`: where it begins, its frames' bytes, its chunk table, its chunks'
/// SHA-256s, and, with its file's samples, how many.
#[derive(Default)]
struct Array {
    start: Option<u64>,
    size: usize,
    table: Vec<u8>,
    hashes: Vec<[u8; 32]>,
    samples: Option<usize>,
}

/// What waits to be written: a chunk of an array (its values' bytes, and its packing), the end of
/// one, its chunk table, or a check of what every chunk before it held.
enum Coming {
    Chunk(usize, usize, Pending<Stored>),
    End(usize),
    Check(Box<dyn FnOnce() -> Result<()>>),
}

/// A file written from a place on, through a descriptor others share and read at places of their own;
/// with none, the bytes only counted.
struct Placed {
    file: Option<Arc<std::fs::File>>,
    at: u64,
}

impl Write for Placed {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(file) = &self.file {
            write_at(file, bytes, self.at)?;
        }
        self.at += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Out {
    /// Arrays written into `file` from its byte 8 + `at`; with no file, only their bytes counted.
    pub(crate) fn new(file: Option<Arc<std::fs::File>>, at: u64) -> Self {
        let file = std::io::BufWriter::with_capacity(1 << 20, Placed { file, at: 8 + at });
        Self { file, at, arrays: Vec::new(), coming: VecDeque::new(), chunks: 0, waiting: 0, shared: HashMap::new() }
    }

    /// An offsets array a file already holds, by its record there: one a new list column may share.
    pub(crate) fn share(&mut self, record: &Map<String, Value>) {
        if let Some(sha256) = record.get("sha256").and_then(Value::as_str).filter(|_| record.get("size").and_then(Value::as_u64) > Some(0)) {
            self.shared.entry(sha256.to_owned()).or_insert_with(|| record.clone());
        }
    }

    /// A new array: its id, for the chunks handed over for it.
    fn array(&mut self) -> usize {
        self.arrays.push(Array::default());
        self.arrays.len() - 1
    }

    /// The end of array `id` handed over, with its file's `samples` when it has `by_sample`: its
    /// record's `at`, `size` and checksums to come, which `placed` gives once it is written.
    fn ended(&mut self, id: usize, samples: Option<usize>, workers: usize) -> Result<Map<String, Value>> {
        self.arrays[id].samples = samples;
        self.hand(Coming::End(id), workers)?;
        let mut record = Map::new();
        record.insert("at".into(), id.into());
        record.insert("size".into(), 0.into());
        record.insert("sha256".into(), Value::Null);
        if samples.is_some() {
            record.insert("by_sample".into(), Value::Null);
        }
        Ok(record)
    }

    /// Every array written into the file.
    pub(crate) fn flushed(self) -> Result<()> {
        self.file.into_inner().map(drop).map_err(|e| e.into_error().into())
    }

    /// Hand over what comes next, then write from the head until at most two chunks for each of
    /// `workers` wait, and at most `AHEAD` bytes of them beyond the one at the head.
    fn hand(&mut self, coming: Coming, workers: usize) -> Result<()> {
        if let Coming::Chunk(_, bytes, _) = &coming {
            (self.chunks, self.waiting) = (self.chunks + 1, self.waiting + bytes);
        }
        self.coming.push_back(coming);
        while self.chunks > 2 * workers || (self.chunks > 1 && self.waiting > AHEAD) {
            self.next()?;
        }
        Ok(())
    }

    /// What is at the head written, once it is packed.
    fn next(&mut self) -> Result<()> {
        let at = self.at;
        match self.coming.pop_front() {
            Some(Coming::Chunk(id, bytes, pending)) => {
                (self.chunks, self.waiting) = (self.chunks - 1, self.waiting - bytes);
                let (digest, frames) = pending.wait().ok_or_else(|| invalid("a bug in rowver: a thread packing a chunk stopped; please report it"))??;
                let sizes: Vec<u64> = match &frames {
                    Frames::Apart(frames) => {
                        for frame in frames {
                            self.write_all(frame)?;
                        }
                        frames.iter().map(|frame| frame.len() as u64).collect()
                    }
                    Frames::Together(bytes, sizes) => {
                        self.write_all(bytes)?;
                        sizes.clone()
                    }
                };
                let array = &mut self.arrays[id];
                array.start.get_or_insert(at);
                for size in sizes {
                    array.size += size as usize;
                    array.table.extend(size.to_le_bytes());
                }
                array.table.extend_from_slice(&digest);
                array.hashes.push(digest);
            }
            Some(Coming::End(id)) => {
                let table = std::mem::take(&mut self.arrays[id].table);
                self.write_all(&table)?;
                let array = &mut self.arrays[id];
                array.start.get_or_insert(at);
                array.size += table.len();
            }
            Some(Coming::Check(check)) => check()?,
            None => {}
        }
        Ok(())
    }

    /// Everything handed over written.
    pub(crate) fn settle(&mut self) -> Result<()> {
        while !self.coming.is_empty() {
            self.next()?;
        }
        Ok(())
    }

    /// `record`, and a list column's arrays in it, given the place, size and checksums of each
    /// array written here, once settled, over the chunks `rows`.
    pub(crate) fn placed(&self, record: &mut Map<String, Value>, rows: &[Rows]) -> Result<()> {
        for part in ["offsets", "content"] {
            if let Some(Value::Object(inner)) = record.get_mut(part) {
                self.placed(inner, rows)?;
            }
        }
        if record.get("sha256") != Some(&Value::Null) {
            return Ok(());
        }
        let bug = || invalid("a bug in rowver: an array's record was made before it was written; please report it");
        let id = record["at"].as_u64().ok_or_else(bug)? as usize;
        let array = self.arrays.get(id).filter(|_| self.coming.is_empty()).ok_or_else(bug)?;
        let (sha256, by_sample) = chunks::sums(rows, &array.hashes, array.samples.unwrap_or(0));
        record.insert("at".into(), array.start.ok_or_else(bug)?.into());
        record.insert("size".into(), array.size.into());
        record.insert("sha256".into(), sha256.into());
        if array.samples.is_some() {
            record.insert("by_sample".into(), by_sample.into());
        }
        Ok(())
    }
}

impl Write for Out {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.at += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// A chunk's SHA-256 and its lanes' frames; or why a chunk copied was refused.
type Stored = Result<([u8; 32], Frames)>;

/// What hands a chunk's hashing and compressing to the threads of a `Pool`, and how many it has.
#[derive(Clone, Copy)]
pub(super) struct Packer<'p> {
    pub(super) run: &'p (dyn Fn(Box<dyn FnOnce() -> Stored + Send>) -> Pending<Stored> + Sync),
    pub(super) workers: usize,
}

/// How many bytes of chunks may wait to be written, beyond the one at the head.
const AHEAD: usize = 256 << 20;

/// One array cut a chunk at a time: each chunk handed to `out`, hashed and its lanes framed on the
/// packer's threads and written in order, then its chunk table; with no `out`, each chunk only
/// hashed, here (a derived column's).
struct Chunked<'o> {
    w: usize,
    out: Option<(&'o mut Out, Packer<'o>, usize)>,
    hashes: Vec<[u8; 32]>,
}

impl<'o> Chunked<'o> {
    fn new(kind: &str, out: Option<(&'o mut Out, Packer<'o>)>) -> Result<Self> {
        let out = out.map(|(out, packer)| {
            let id = out.array();
            (out, packer, id)
        });
        Ok(Self { w: width(kind)?, out, hashes: Vec::new() })
    }

    /// The next chunk's values, little-endian, and its byte lanes when they are at hand.
    fn chunk(&mut self, values: Vec<u8>, lanes: Option<Vec<Vec<u8>>>) -> Result<()> {
        let Some((out, packer, id)) = &mut self.out else {
            self.hashes.push(Sha256::digest(&values).into());
            return Ok(());
        };
        let (w, bytes) = (self.w, values.len());
        let pending = (packer.run)(Box::new(move || {
            let frames = match lanes {
                Some(lanes) => lanes.iter().map(|lane| crate::codec::pack(lane)).collect(),
                None => chunks::frames(&values, w),
            };
            Ok((Sha256::digest(&values).into(), Frames::Apart(frames)))
        }));
        out.hand(Coming::Chunk(*id, bytes, pending), packer.workers)
    }

    /// Its record from the chunks of rows `rows`: its `sha256`, and, with `samples`, its
    /// `by_sample`; written, its table handed over after its chunks, and its `at`, `size` and
    /// checksums kept for `Out::placed` to give once it is written.
    fn finish(self, rows: &[Rows], samples: Option<usize>) -> Result<Map<String, Value>> {
        if let Some((out, packer, id)) = self.out {
            return out.ended(id, samples, packer.workers);
        }
        let mut record = Map::new();
        let (sha256, by_sample) = chunks::sums(rows, &self.hashes, samples.unwrap_or(0));
        record.insert("sha256".into(), sha256.into());
        if samples.is_some() {
            record.insert("by_sample".into(), by_sample.into());
        }
        Ok(record)
    }
}

/// What a cutting makes of a column: written, a flat column's array, or a list column's offsets
/// (from its counts alone) or its content, a list column being written one array at a time; or
/// only measured, its array's checksums and, of a list column, its offsets' too.
enum Cut<'o> {
    Flat(Chunked<'o>),
    Offsets(Chunked<'o>),
    Content(Chunked<'o>),
    Measured(Option<Chunked<'o>>, Chunked<'o>),
}

/// A column's values taken in row order, any number of rows at a time, and cut at the file's
/// chunks: a flat column's array (with its facts), or a list column's offsets or content.
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
    cut: Cut<'o>,
}

impl<'o> Cutting<'o> {
    /// A flat column `name` of `kind` over the chunks `rows`, its array written to `out`.
    fn flat(name: &str, kind: &str, rows: &[Rows], out: (&'o mut Out, Packer<'o>)) -> Result<Self> {
        Self::made(name, kind, rows, Cut::Flat(Chunked::new(kind, Some(out))?))
    }

    /// A list column `name` of values of `kind` over the chunks `rows`, its offsets written to `out`.
    fn offsets(name: &str, kind: &str, rows: &[Rows], out: (&'o mut Out, Packer<'o>)) -> Result<Self> {
        Self::made(name, kind, rows, Cut::Offsets(Chunked::new("i8", Some(out))?))
    }

    /// A list column `name` of values of `kind` over the chunks `rows`, its content written to `out`.
    fn content(name: &str, kind: &str, rows: &[Rows], out: (&'o mut Out, Packer<'o>)) -> Result<Self> {
        Self::made(name, kind, rows, Cut::Content(Chunked::new(kind, Some(out))?))
    }

    /// A column `name` of `kind`, a list column when `listed`, over the chunks `rows`, only
    /// measured: its records' checksums and facts, no array written.
    fn measuring(name: &str, kind: &str, listed: bool, rows: &[Rows]) -> Result<Self> {
        let offsets = listed.then(|| Chunked::new("i8", None)).transpose()?;
        Self::made(name, kind, rows, Cut::Measured(offsets, Chunked::new(kind, None)?))
    }

    /// A cutting of a column that makes `cut` of it.
    fn made(name: &str, kind: &str, rows: &[Rows], cut: Cut<'o>) -> Result<Self> {
        let flat = matches!(cut, Cut::Flat(_) | Cut::Measured(None, _));
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
            facts: flat.then(|| Facts::new(kind)),
            cut,
        })
    }

    /// The next rows: their `n` values (none at hand when only a list column's offsets are
    /// written), and for a list column how many each row holds.
    fn take(&mut self, values: Option<&Piece>, n: usize, counts: Option<&[usize]>) -> Result<()> {
        if let Some(values) = values {
            let (held, truths) = match values {
                Piece::Raw(raw) if raw.len() % self.w != 0 => return Err(invalid(format!("{}: its {} bytes are not whole values of {}, {} bytes each: give it exactly its values' bytes", self.name, raw.len(), self.kind, self.w))),
                Piece::Raw(raw) => (raw.len() / self.w, &raw[..]),
                Piece::Lanes(lanes) => (lanes.first().map_or(0, |lane| lane.len()), lanes.first().map_or(&[][..], |lane| &lane[..])),
            };
            if held != n {
                return Err(invalid(format!("{}: a piece of it holds {held} values where it says {n}: a bug in rowver; please report it", self.name)));
            }
            if let Some(byte) = crate::numeric::untrue(&self.kind, truths) {
                return Err(invalid(format!("{}: a b1 value is the byte 0 or 1, and one is {byte}: give false as 0 and true as 1", self.name)));
            }
        }
        let (name, rows) = (self.name.clone(), self.rows.last().map_or(0, |last| last.2));
        let per = if counts.is_some() { "list" } else { "value" };
        let too_many = move || invalid(format!("{name}: it holds more rows than the file's {rows}: give it one {per} per row"));
        let Some(counts) = counts else {
            let values = values.ok_or_else(|| invalid("a bug in rowver: a flat column's values were not read; please report it"))?;
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
                if let Some(values) = values {
                    self.gather(values, from, at);
                }
                from = at;
                self.close()?;
            }
        }
        if at != n {
            return Err(invalid(format!("{}: it holds values no row's list holds: give it one list per row, the lists together holding every value", self.name)));
        }
        if let Some(values) = values {
            self.gather(values, from, at);
        }
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
        let (array, offsets) = match &mut self.cut {
            Cut::Flat(array) | Cut::Content(array) => (Some(array), None),
            Cut::Offsets(offsets) => (None, Some(offsets)),
            Cut::Measured(offsets, array) => (Some(array), offsets.as_mut()),
        };
        if let Some(array) = array {
            let lanes = self.lanes.as_mut().map(|lanes| std::mem::replace(lanes, vec![Vec::new(); self.w]));
            let values = match &lanes {
                Some(lanes) => crate::codec::merge(lanes, self.w, lanes.first().map_or(0, Vec::len)),
                None => std::mem::take(&mut self.values),
            };
            if let Some(facts) = &mut self.facts {
                facts.add(&values)?;
            }
            array.chunk(values, lanes)?;
        }
        if let Some(offsets) = offsets {
            let mut steps = Vec::with_capacity((self.counts.len() + 1) * 8);
            let mut at = 0_i64;
            steps.extend(at.to_le_bytes());
            for count in self.counts.drain(..) {
                at = i64::try_from(count).ok().and_then(|count| at.checked_add(count)).ok_or_else(|| invalid(format!("{}: its lists hold more values than an offset counts (2^63 − 1): write fewer rows into one file", self.name)))?;
                steps.extend(at.to_le_bytes());
            }
            offsets.chunk(steps, None)?;
        }
        self.counts.clear();
        self.next += 1;
        Ok(())
    }

    /// The column's record, or of a list column what this cutting made of it: a flat column's
    /// kind, array and facts; a list column's kind and both arrays' records, or `offsets` alone, or
    /// `content` alone (with its kind and count).
    fn finish(self, samples: usize) -> Result<Map<String, Value>> {
        let rows = self.rows.last().map_or(0, |last| last.2);
        if self.next != self.rows.len() || !self.values.is_empty() || self.lanes.iter().flatten().any(|lane| !lane.is_empty()) {
            return Err(invalid(format!("{}: it holds {} rows, and the file {rows}: give it one value (or list) per row", self.name, self.taken)));
        }
        let content = |array: Chunked| -> Result<Map<String, Value>> {
            let mut content = Map::new();
            content.insert("kind".into(), self.kind.clone().into());
            content.insert("count".into(), self.members.into());
            content.append(&mut array.finish(&self.rows, Some(samples))?);
            Ok(content)
        };
        let mut record = Map::new();
        match self.cut {
            Cut::Flat(array) | Cut::Measured(None, array) => {
                record.insert("kind".into(), self.kind.clone().into());
                record.append(&mut array.finish(&self.rows, Some(samples))?);
                if let Some(facts) = self.facts {
                    record.append(&mut facts.finish());
                }
                Ok(record)
            }
            Cut::Measured(Some(offsets), array) => {
                record.insert("kind".into(), "jagged".into());
                record.insert("offsets".into(), Value::Object(offsets.finish(&self.rows, Some(samples))?));
                record.insert("content".into(), Value::Object(content(array)?));
                Ok(record)
            }
            Cut::Offsets(offsets) => offsets.finish(&self.rows, Some(samples)),
            Cut::Content(array) => content(array),
        }
    }
}

/// A piece read: its values (none when only its counts are wanted), and for a list column its counts.
type Read<'e> = (Option<Piece<'e>>, Option<Cow<'e, [usize]>>);

/// Feed every value of `pieces` to `cutting` a piece at a time, the next few read on `pool`
/// while one is cut, each first handed to `check`; for a list column's offsets, only each piece's
/// counts are read.
fn fed<'e>(pieces: &'e dyn Pieces, cutting: &mut Cutting, pool: &Pool<'e>, check: &mut dyn FnMut(usize, &Piece) -> Result<()>) -> Result<()> {
    let w = width(pieces.kind())?;
    let counts_only = matches!(cutting.cut, Cut::Offsets(_));
    let read = move |at: usize| -> Result<Read<'e>> {
        let values = match counts_only {
            true => None,
            false => Some(piece(pieces, at, w)?),
        };
        Ok((values, pieces.counts(at)?))
    };
    // A piece read ahead for each of the pool's workers, while one is cut.
    let ahead = pool.workers();
    let mut coming = VecDeque::new();
    for at in 0..pieces.pieces() {
        while coming.len() < ahead && at + coming.len() < pieces.pieces() {
            let next = at + coming.len();
            coming.push_back(pool.run(move || read(next)));
        }
        let read = coming.pop_front().and_then(Pending::wait).ok_or_else(|| invalid("a bug in rowver: a thread reading a column stopped; please report it"))?;
        let (values, counts) = read?;
        if let Some(values) = &values {
            check(at, values)?;
        }
        cutting.take(values.as_ref(), pieces.values(at), counts.as_deref())?;
    }
    Ok(())
}

/// Pack the `sample` array: each chunk holds its sample's index, which no checksum but its own has.
pub(super) fn sample(rows: &[Rows], out: &mut Out, pool: &Pool) -> Result<Map<String, Value>> {
    let pack = |job| pool.run(job);
    let mut array = Chunked::new("i4", Some((out, Packer { run: &pack, workers: pool.workers() })))?;
    for (sample, first, end) in rows {
        let index = i32::try_from(*sample).map_err(|_| invalid("Too many samples"))?;
        array.chunk(index.to_le_bytes().repeat(end - first), None)?;
    }
    array.finish(rows, None)
}

/// The `sha256` of the `sample` array of rows of samples that end at `edges`, cut at the chunks
/// `rows`, no array written: each stretch of a chunk within one sample its index repeated.
pub(super) fn sample_sha256(edges: &[usize], rows: &[Rows]) -> Result<String> {
    let mut array = Chunked::new("i4", None)?;
    // The sample of the row at hand: edges[sample] <= row < edges[sample + 1], past samples of no rows.
    let mut sample = 0;
    for (_, first, end) in rows {
        let mut values = Vec::with_capacity((end - first) * 4);
        let mut row = *first;
        while row < *end {
            while edges.get(sample + 1).is_some_and(|edge| *edge <= row) {
                sample += 1;
            }
            let to = edges.get(sample + 1).map_or(*end, |edge| (*edge).min(*end));
            let index = i32::try_from(sample).map_err(|_| invalid("Too many samples"))?;
            values.extend(index.to_le_bytes().repeat(to - row));
            row = to;
        }
        array.chunk(values, None)?;
    }
    Ok(array.finish(rows, None)?["sha256"].as_str().unwrap_or_default().to_owned())
}

/// Where entries first differ: the row, the entry given there, and the file's (none past the end of either).
pub(super) type Differ = (usize, Option<i64>, Option<u64>);

/// Where the `entry` values of `pieces` (one i8 each, in order) first differ from `read`, a file's
/// entries of each of its files as they were read and checked; none when they agree.
pub(super) fn entries_differ(pieces: &dyn Pieces, read: &[&[u64]]) -> Result<Option<Differ>> {
    let mut was = read.iter().flat_map(|file| file.iter().copied());
    let mut row = 0;
    for at in 0..pieces.pieces() {
        let n = pieces.values(at);
        let values: Vec<i64> = match pieces.piece(at)? {
            Piece::Raw(bytes) if bytes.len() == n * 8 => bytes.as_chunks::<8>().0.iter().map(|v| i64::from_le_bytes(*v)).collect(),
            Piece::Lanes(lanes) if lanes.len() == 8 && lanes.iter().all(|lane| lane.len() == n) => (0..n).map(|v| i64::from_le_bytes(std::array::from_fn(|lane| lanes[lane][v]))).collect(),
            _ => return Err(invalid("a bug in rowver: a piece of entries is not its count of i8 values; please report it")),
        };
        for value in values {
            match was.next() {
                Some(entry) if entry as i64 == value => row += 1,
                entry => return Ok(Some((row, Some(value), entry))),
            }
        }
    }
    Ok(was.next().map(|entry| (row, None, Some(entry))))
}

/// A source made ready to pack: the pieces it holds, or the file it is copied from.
pub(super) enum Ready<'a> {
    Whole(Whole<'a>),
    Spooled(Spooled<'a>),
    Copied(&'a Reader, String),
}

impl<'a> Ready<'a> {
    /// `source`, of the column (or array) `name`, ready to pack.
    pub(super) fn of(source: Source<'a>, name: &str) -> Result<Self> {
        Ok(match source {
            Source::Values(values) => Ready::Whole(Whole::new(values, name)?),
            Source::Spooled(spooled) => Ready::Spooled(spooled),
            Source::Copied(reader, from) => Ready::Copied(reader, from),
        })
    }

    /// The pieces it holds; none, when it is copied.
    pub(super) fn pieces(&self) -> Option<&dyn Pieces> {
        match self {
            Ready::Whole(whole) => Some(whole),
            Ready::Spooled(spooled) => Some(spooled),
            Ready::Copied(..) => None,
        }
    }
}

/// Pack the column `name` from what `ready` holds: copied from another file, or from its pieces.
pub(super) fn packed<'e>(name: &str, ready: &'e Ready, rows: &[Rows], samples: usize, out: &mut Out, pool: &Pool<'e>) -> Result<Map<String, Value>> {
    match ready {
        Ready::Copied(reader, from) => copied(name, reader, from, rows, samples, out, pool),
        Ready::Whole(whole) => column(name, whole, rows, samples, out, pool),
        Ready::Spooled(spooled) => column(name, spooled, rows, samples, out, pool),
    }
}

/// Pack the `entry` array, which must hold one `i8` value per row, from its pieces; `files` are
/// the files' row bounds, within each of which the entries strictly increase.
pub(super) fn entry<'e>(pieces: &'e dyn Pieces, rows: &[Rows], samples: usize, files: &[usize], out: &mut Out, pool: &Pool<'e>) -> Result<Map<String, Value>> {
    let total: usize = (0..pieces.pieces()).map(|p| pieces.values(p)).sum();
    let end = rows.last().map_or(0, |last| last.2);
    if pieces.listed() || pieces.kind() != "i8" || total != end {
        return Err(invalid(format!("the entry array holds {total} {} values{}, and it holds one i8 value per row, {end}: give each row's entry as an i8", pieces.kind(), if pieces.listed() { " in lists" } else { "" })));
    }
    let pack = |job| pool.run(job);
    let mut cutting = Cutting::flat("entry", "i8", rows, (out, Packer { run: &pack, workers: pool.workers() }))?;
    cutting.facts = None;
    let (mut row, mut file, mut last) = (0_usize, 0_usize, None::<i64>);
    // The entries are checked as values; they are written as they came, in lanes or not.
    let mut check = |at: usize, held: &Piece| -> Result<()> {
        let values = match held {
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
        Ok(())
    };
    fed(pieces, &mut cutting, pool, &mut check)?;
    let mut record = cutting.finish(samples)?;
    record.remove("kind");
    Ok(record)
}

/// Pack one column of the chunks `rows` from its pieces: a list column's offsets, from its counts
/// alone, then its content.
pub(super) fn column<'e>(name: &str, pieces: &'e dyn Pieces, rows: &[Rows], samples: usize, out: &mut Out, pool: &Pool<'e>) -> Result<Map<String, Value>> {
    let pack = |job| pool.run(job);
    let pack = Packer { run: &pack, workers: pool.workers() };
    let mut record = Map::new();
    record.insert("name".into(), name.into());
    let none = &mut |_: usize, _: &Piece| Ok(());
    if !pieces.listed() {
        let mut cutting = Cutting::flat(name, pieces.kind(), rows, (out, pack))?;
        fed(pieces, &mut cutting, pool, none)?;
        record.append(&mut cutting.finish(samples)?);
        return Ok(record);
    }
    // Its offsets measured first, from its counts: those of an earlier list column are that one's
    // array, shared, and written once (docs/FORMAT.md §7).
    let mut measured = Cutting::made(name, pieces.kind(), rows, Cut::Offsets(Chunked::new("i8", None)?))?;
    fed(pieces, &mut measured, pool, none)?;
    let sha256 = measured.finish(samples)?.get("sha256").and_then(Value::as_str).unwrap_or_default().to_owned();
    let offsets = match out.shared.get(&sha256).filter(|_| !rows.is_empty()) {
        Some(shared) => shared.clone(),
        None => {
            let mut offsets = Cutting::offsets(name, pieces.kind(), rows, (&mut *out, pack))?;
            fed(pieces, &mut offsets, pool, none)?;
            let offsets = offsets.finish(samples)?;
            out.shared.insert(sha256, offsets.clone());
            offsets
        }
    };
    let mut content = Cutting::content(name, pieces.kind(), rows, (out, pack))?;
    fed(pieces, &mut content, pool, none)?;
    record.insert("kind".into(), "jagged".into());
    record.insert("offsets".into(), Value::Object(offsets));
    record.insert("content".into(), Value::Object(content.finish(samples)?));
    Ok(record)
}

/// A column measured a few rows at a time, as a derived column's record holds it: its kind, its
/// checksums (cut at the file's chunks) and facts, and no array.
pub(crate) struct Measuring(Cutting<'static>);

impl Measuring {
    /// A column `name` of `kind` (a list column's values' kind, when `listed`) over the chunks `rows`.
    pub(crate) fn new(name: &str, kind: &str, listed: bool, rows: &[Rows]) -> Result<Self> {
        Ok(Self(Cutting::measuring(name, kind, listed, rows)?))
    }

    /// The next rows: their values, and for a list column how many each row holds.
    pub(crate) fn take(&mut self, values: &[u8], counts: Option<&[usize]>) -> Result<()> {
        let n = values.len() / self.0.w;
        self.0.take(Some(&Piece::Raw(Cow::Borrowed(values))), n, counts)
    }

    /// The record, from its kind on, every row taken.
    pub(crate) fn finish(self, samples: usize) -> Result<Map<String, Value>> {
        self.0.finish(samples)
    }
}

/// Copy a column as it lies in another file of the same rows and chunks: its arrays byte for byte,
/// each chunk decoded and checked on `pool` and written in order through `out`, as packed chunks
/// are; its facts made again, and equal to its record's there. Its record is that file's, its
/// arrays where `out` writes them.
pub(super) fn copied<'e>(name: &str, reader: &'e Reader, from: &str, rows: &[Rows], samples: usize, out: &mut Out, pool: &Pool<'e>) -> Result<Map<String, Value>> {
    let Some(held) = reader.record(from).filter(|held| held.is_object()) else {
        return Err(invalid(format!("{name}: the file it is copied from does not hold {from}")));
    };
    if reader.chunks()?.as_slice() != rows {
        return Err(invalid(format!("{name}: the file it is copied from, {}, is not cut into the same chunks: press again with --full", reader.path().display())));
    }
    let mut record = held.as_object().cloned().unwrap_or_default();
    record.insert("name".into(), name.into());
    let path = reader.path().display();
    let unlike: Arc<str> = format!("{name}: its array in {path} does not hold what its record says. Run rvr validate on that file, or press again with --full.").into();
    let damaged: Arc<str> = format!("{name}: its array in {path} is damaged. Run rvr validate on that file, or press again with --full.").into();
    let place = format!("data.cols[{from:?}]");
    let mut copying = Copying { reader, out, pool, chunks: rows.len(), samples, unlike: unlike.clone(), damaged: damaged.clone() };
    let kind = |held: &Value| held["kind"].as_str().unwrap_or("").to_owned();
    if held["kind"] != "jagged" {
        let kind = kind(held);
        let facts = Arc::new(Mutex::new((0..rows.len()).map(|_| Facts::new(&kind)).collect()));
        record.extend(copying.array(held, &kind, &place, &|k| rows[k].2 - rows[k].1, Some(&facts))?);
        // Once every chunk is written, its facts merged in row order, as a press takes them.
        let expected = held.clone();
        let check = move || {
            let mut chunks = std::mem::take(&mut *facts.lock().unwrap_or_else(|e| e.into_inner())).into_iter();
            let mut made = chunks.next().unwrap_or_else(|| Facts::new(&kind));
            chunks.for_each(|chunk| made.merge(chunk));
            crate::rvr::facts::agree(&expected, &made.finish(), &kind, "").map_err(|_| invalid(&*unlike))
        };
        out.hand(Coming::Check(Box::new(check)), pool.workers())?;
        return Ok(record);
    }
    let (offsets, content) = (&held["offsets"], &held["content"]);
    // The offsets first, a few chunks ahead on the pool: they give each content chunk's values.
    let chunks = reader.array_chunks(offsets, "i8", &format!("{place}.offsets")).map_err(|e| refused(e, &unlike))?;
    // Offsets an earlier list column wrote or the file holds are that array, shared: read here
    // only for each content chunk's values.
    let sha256 = offsets["sha256"].as_str().unwrap_or_default().to_owned();
    let shared = copying.out.shared.get(&sha256).filter(|_| !rows.is_empty()).cloned();
    let id = shared.is_none().then(|| copying.out.array());
    let (mut members, mut coming) = (Vec::with_capacity(rows.len()), VecDeque::new());
    for k in 0..rows.len() {
        while coming.len() < pool.workers() && k + coming.len() < rows.len() {
            let (next, chunks) = (k + coming.len(), chunks.clone());
            let n = rows[next].2 - rows[next].1 + 1;
            coming.push_back(pool.run(move || chunks.read(next, n)));
        }
        let read = coming.pop_front().and_then(Pending::wait).ok_or_else(|| invalid("a bug in rowver: a thread reading a column stopped; please report it"))?;
        let (digest, frames, values) = read.map_err(|e| refused(e, &damaged))?;
        members.push(crate::chunks::offsets(&values, name).map_err(|_| invalid(&*unlike))?.1);
        if let Some(id) = id {
            copying.out.hand(Coming::Chunk(id, values.len(), Pending::Done(Ok((digest, frames)))), pool.workers())?;
        }
    }
    let placed = match id {
        None => shared.unwrap_or_default(),
        Some(id) => {
            let mut placed = offsets.as_object().cloned().unwrap_or_default();
            placed.extend(copying.out.ended(id, Some(samples), pool.workers())?);
            copying.out.shared.insert(sha256, placed.clone());
            placed
        }
    };
    record.insert("offsets".into(), Value::Object(placed));
    let total = members.iter().try_fold(0_u64, |sum, m| sum.checked_add(*m as u64));
    if total.is_none() || total != content["count"].as_u64() {
        return Err(invalid(&*unlike));
    }
    let mut placed = content.as_object().cloned().unwrap_or_default();
    placed.extend(copying.array(content, &kind(content), &format!("{place}.content"), &|k| members[k], None)?);
    record.insert("content".into(), Value::Object(placed));
    Ok(record)
}

/// A column copied from another file: the file, where it is written, the pool its chunks are
/// decoded on, how many chunks and samples the file has, and what an array unlike its record and a
/// damaged one are told.
struct Copying<'c, 'e> {
    reader: &'e Reader,
    out: &'c mut Out,
    pool: &'c Pool<'e>,
    chunks: usize,
    samples: usize,
    unlike: Arc<str>,
    damaged: Arc<str>,
}

impl Copying<'_, '_> {
    /// The array of `kind` the file holds under `record` (at `label`), `n(k)` values in chunk `k`:
    /// each chunk read, decoded and checked on the pool, its values' facts put in `facts[k]`, and
    /// written in order; its record's `at`, `size` and checksums, which `Out::placed` gives once
    /// it is written.
    fn array(&mut self, record: &Value, kind: &str, label: &str, n: &dyn Fn(usize) -> usize, facts: Option<&Arc<Mutex<Vec<Facts>>>>) -> Result<Map<String, Value>> {
        let chunks = self.reader.array_chunks(record, kind, label).map_err(|e| refused(e, &self.unlike))?;
        let (id, w) = (self.out.array(), width(kind)?);
        for k in 0..self.chunks {
            let (chunks, facts, kind, n) = (chunks.clone(), facts.cloned(), kind.to_owned(), n(k));
            let (unlike, damaged) = (self.unlike.clone(), self.damaged.clone());
            let job = move || -> Stored {
                let (digest, frames, values) = chunks.read(k, n).map_err(|e| refused(e, &damaged))?;
                if crate::numeric::untrue(&kind, &values).is_some() {
                    return Err(invalid(&*unlike));
                }
                if let Some(facts) = facts {
                    let mut seen = Facts::new(&kind);
                    seen.add(&values)?;
                    facts.lock().unwrap_or_else(|e| e.into_inner())[k] = seen;
                }
                Ok((digest, frames))
            };
            self.out.hand(Coming::Chunk(id, n * w, self.pool.run(job)), self.pool.workers())?;
        }
        self.out.ended(id, Some(self.samples), self.pool.workers())
    }
}

/// `e`, met copying an array: what the file holds, refused as it is read, is told `said`.
fn refused(e: crate::Error, said: &str) -> crate::Error {
    match e {
        crate::Error::Invalid(_) => invalid(said.to_owned()),
        e => e,
    }
}

#[cfg(test)]
mod tests {
    use super::{Chunked, Out, Packer};

    // FORMAT 7, 8
    #[test]
    fn the_sample_array_hashed_by_stretches_is_the_one_of_each_row_s_sample() {
        // Each row's sample found by a search of the edges, row by row.
        let by_row = |edges: &[usize], rows: &[crate::chunks::Rows]| {
            let mut array = Chunked::new("i4", None).unwrap();
            for (_, first, end) in rows {
                let values: Vec<u8> = (*first..*end).flat_map(|row| (edges.partition_point(|edge| *edge <= row).saturating_sub(1) as i32).to_le_bytes()).collect();
                array.chunk(values, None).unwrap();
            }
            array.finish(rows, None).unwrap()["sha256"].as_str().unwrap().to_owned()
        };
        let cases: [(Vec<usize>, Vec<crate::chunks::Rows>); 5] = [
            (vec![0, 4, 7], vec![(0, 0, 2), (0, 2, 4), (1, 4, 7)]),
            // Samples of no rows, first, between and last.
            (vec![0, 0, 3, 3, 7, 7], vec![(1, 0, 3), (3, 3, 5), (3, 5, 7)]),
            // A chunk across two samples' rows, as rows given otherwise than the file's lay them.
            (vec![0, 2, 7], vec![(0, 0, 3), (1, 3, 7)]),
            (vec![0, 1, 2, 3], vec![(0, 0, 3)]),
            (vec![0, 0], vec![]),
        ];
        for (edges, rows) in cases {
            assert_eq!(super::sample_sha256(&edges, &rows).unwrap(), by_row(&edges, &rows), "{edges:?} {rows:?}");
        }
    }

    // SPEC 8.12
    #[test]
    fn arrays_written_with_no_file_are_only_counted() {
        let mut placed = super::Placed { file: None, at: 8 };
        assert_eq!(std::io::Write::write(&mut placed, b"bytes").unwrap(), 5);
        assert_eq!(placed.at, 13);
    }

    // SPEC 7.6.1
    #[test]
    fn the_next_arrays_are_packed_while_earlier_ones_wait_to_be_written_two_chunks_a_worker() {
        let path = std::env::temp_dir().join(format!("rowver-unit-{}-ahead", std::process::id()));
        let mut out = Out::new(Some(std::fs::File::create(&path).unwrap().into()), 0);
        let rows = vec![(0, 0, 1000)];
        let records = std::thread::scope(|scope| {
            let pool = crate::parallel::Pool::new(scope, 2);
            assert_eq!(pool.workers(), 2);
            let pack = |job| pool.run(job);
            let packer = Packer { run: &pack, workers: pool.workers() };
            let mut records = Vec::new();
            for n in 0..20_u64 {
                let mut array = Chunked::new("i8", Some((&mut out, packer))).unwrap();
                array.chunk(n.to_le_bytes().repeat(1000), None).unwrap();
                records.push(array.finish(&rows, None).unwrap());
                assert!(out.chunks <= 4, "{} chunks wait", out.chunks);
                if n == 2 {
                    assert_eq!(out.chunks, 3, "no array waits for the one before it to be written");
                }
            }
            out.settle().unwrap();
            records
        });
        // Written in the order they were handed over, each where its record says.
        let mut end = 0;
        for mut record in records {
            out.placed(&mut record, &rows).unwrap();
            assert_eq!(record["at"], end);
            end += record["size"].as_u64().unwrap();
            assert!(record["sha256"].is_string());
        }
        assert_eq!(out.at, end);
        std::fs::remove_file(&path).unwrap();
    }
}
