//! A file's columns gathered a piece at a time, held compressed on disk until each is packed,
//! so what is held in memory does not grow with the rows.

use super::column::{read_at, write_at, Pieces};
use super::Values;
use crate::numeric::width;
use crate::{codec, invalid, Result};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where a piece goes among its column's pieces: which input, then which piece of it.
pub type Order = (usize, usize);

/// Where one run of bytes lies in the spool, how many bytes it holds, and whether it is
/// compressed.
#[derive(Clone, Debug)]
struct Span {
    at: u64,
    size: usize,
    bytes: usize,
    packed: bool,
}

/// How much of a run is compressed first to see whether the rest is worth compressing.
const PROBE: usize = 16 << 10;

/// One piece of a column: each byte lane of its values, and for a list column each row's count.
#[derive(Clone, Debug)]
struct Part {
    kind: String,
    values: usize,
    lanes: Vec<Span>,
    counts: Option<Span>,
}

/// Pieces of columns in a file of their own, added in any order and packed in the order of their `order`.
pub struct Spool {
    path: PathBuf,
    file: std::fs::File,
    /// How many bytes it holds.
    end: Mutex<u64>,
    parts: Mutex<BTreeMap<(usize, Order), Part>>,
}

impl Spool {
    /// An empty spool in a file at `path`, which stays when it is dropped.
    pub fn create(path: &Path) -> Result<Self> {
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            path: path.to_owned(),
            file,
            end: Mutex::new(0),
            parts: Mutex::new(BTreeMap::new()),
        })
    }

    /// The spool at `path` holding only the pieces `held` gave, cut after the last; kept when dropped.
    pub fn reopen(path: &Path, held: &[&Value]) -> Result<Self> {
        let file = std::fs::File::options().read(true).write(true).open(path)?;
        let end = file.metadata()?.len();
        let span = |value: &Value| -> Option<Span> {
            let [at, size, bytes, packed] = value.as_array()?.as_slice() else {
                return None;
            };
            let span = Span {
                at: at.as_u64()?,
                size: usize::try_from(size.as_u64()?).ok()?,
                bytes: usize::try_from(bytes.as_u64()?).ok()?,
                packed: packed.as_bool()?,
            };
            (span.at.checked_add(span.size as u64)? <= end).then_some(span)
        };
        let piece = |piece: &Value| -> Option<((usize, Order), Part)> {
            let [column, input, at, kind, values, lanes, counts] = piece.as_array()?.as_slice() else {
                return None;
            };
            let part = Part {
                kind: kind.as_str()?.to_owned(),
                values: usize::try_from(values.as_u64()?).ok()?,
                lanes: lanes.as_array()?.iter().map(span).collect::<Option<_>>()?,
                counts: match counts {
                    Value::Null => None,
                    counts => Some(span(counts)?),
                },
            };
            // One lane of every value per byte of its kind, and a count per row of a list.
            let fits = |span: &Span, bytes: usize| span.bytes == bytes;
            let lanes = width(&part.kind).ok()? == part.lanes.len();
            let lanes = lanes && part.lanes.iter().all(|lane| fits(lane, part.values));
            (lanes && part.counts.as_ref().is_none_or(|counts| counts.bytes % 8 == 0)).then_some(())?;
            let order = (input.as_u64()? as usize, at.as_u64()? as usize);
            Some(((column.as_u64()? as usize, order), part))
        };
        let mut parts = BTreeMap::new();
        let damaged = |why: &str| invalid(format!("{why}: what names the pieces and the spool do not agree; spool the pieces again"));
        let held = held.iter().map(|held| held.as_array().ok_or_else(|| damaged("a journal line names no spooled pieces")));
        for value in held.collect::<Result<Vec<_>>>()?.into_iter().flatten() {
            let (key, part) = piece(value).ok_or_else(|| damaged("a piece the journal names is not in the spool"))?;
            if parts.insert(key, part).is_some() {
                return Err(damaged("the journal names a spooled piece twice"));
            }
        }
        let spans = parts.values().flat_map(|part: &Part| part.lanes.iter().chain(&part.counts));
        let end = spans.map(|span| span.at + span.size as u64).max().unwrap_or(0);
        file.set_len(end)?;
        Ok(Self {
            path: path.to_owned(),
            file,
            end: Mutex::new(end),
            parts: Mutex::new(parts),
        })
    }

    /// Every piece of every column the spool holds of the inputs `inputs`, for `reopen`.
    pub fn held(&self, inputs: &[usize]) -> Value {
        let copied: Vec<((usize, Order), Part)> = {
            let parts = self.parts.lock().unwrap_or_else(|e| e.into_inner());
            let columns = parts.last_key_value().map_or(0, |((column, _), _)| column + 1);
            let ranges = (0..columns).flat_map(|column| {
                inputs.iter().map(move |&input| (column, (input, 0))..(column, (input + 1, 0)))
            });
            ranges.flat_map(|range| parts.range(range).map(|(key, part)| (*key, part.clone()))).collect()
        };
        let span = |s: &Span| json!([s.at, s.size, s.bytes, s.packed]);
        let held = copied.iter().map(|((column, (input, piece)), part)| {
            let lanes: Vec<Value> = part.lanes.iter().map(span).collect();
            json!([column, input, piece, part.kind, part.values, lanes, part.counts.as_ref().map(span)])
        });
        Value::Array(held.collect())
    }

    /// Make what the spool holds durable on disk.
    pub fn sync(&self) -> Result<()> {
        Ok(self.file.sync_data()?)
    }

    /// Add `bytes` at the end, compressed when `compress` and a sample of them shrinks by a fifth
    /// (the low bytes of measured values are mostly noise, which costs time to compress for little).
    fn put(&self, bytes: &[u8], compress: bool) -> Result<Span> {
        let probe = &bytes[..bytes.len().min(PROBE)];
        let worth = compress && codec::pack(probe, 1).len() * 5 < probe.len() * 4;
        let packed = match worth {
            true => Cow::Owned(codec::pack(bytes, 1)),
            false => Cow::Borrowed(bytes),
        };
        let at = {
            let mut end = self.end.lock().unwrap_or_else(|e| e.into_inner());
            *end += packed.len() as u64;
            *end - packed.len() as u64
        };
        write_at(&self.file, &packed, at)?;
        Ok(Span {
            at,
            size: packed.len(),
            bytes: bytes.len(),
            packed: worth,
        })
    }

    /// Read back what `put` added.
    fn get(&self, span: &Span) -> Result<Vec<u8>> {
        let mut packed = vec![0; span.size];
        read_at(&self.file, &mut packed, span.at).map_err(|e| crate::about(&self.path, e.into()))?;
        match span.packed {
            true => codec::unpack(packed, 1, span.bytes, "a spooled piece"),
            false => Ok(packed),
        }
    }

    /// Hold a piece of column `column`, placed among its pieces by `order`.
    pub fn add(&self, column: usize, order: Order, values: Values) -> Result<()> {
        let (buffer, counts) = values.parts();
        let w = width(&buffer.kind)?;
        if buffer.count.checked_mul(w) != Some(buffer.bytes.len()) {
            return Err(invalid(format!("a spooled piece's {} bytes are not {} values of {}, {w} bytes each: give it exactly its values' bytes", buffer.bytes.len(), buffer.count, buffer.kind)));
        }
        let shuffled = codec::shuffle(&buffer.bytes, w);
        drop(buffer.bytes);
        let lanes = (0..w)
            .map(|lane| {
                self.put(
                    &shuffled[lane * buffer.count..(lane + 1) * buffer.count],
                    true,
                )
            })
            .collect::<Result<_>>()?;
        drop(shuffled);
        let counts = counts
            .map(|counts| {
                let bytes: Vec<u8> = counts
                    .iter()
                    .flat_map(|n| (*n as u64).to_le_bytes())
                    .collect();
                // Read again for every lane of the offsets, so kept as they are.
                self.put(&bytes, false)
            })
            .transpose()?;
        let part = Part {
            kind: buffer.kind,
            values: buffer.count,
            lanes,
            counts,
        };
        let mut parts = self.parts.lock().unwrap_or_else(|e| e.into_inner());
        match parts.entry((column, order)) {
            std::collections::btree_map::Entry::Vacant(free) => {
                free.insert(part);
                Ok(())
            }
            _ => Err(invalid(format!("piece {order:?} of column {column} was spooled twice: spool each piece once"))),
        }
    }

    /// Every piece of column `column`, in order, or `None` when it has none; the spool lets them go.
    pub fn take(&self, column: usize) -> Result<Option<Spooled<'_>>> {
        let parts: Vec<Part> = {
            let mut held = self.parts.lock().unwrap_or_else(|e| e.into_inner());
            let mut rest = held.split_off(&(column, (0, 0)));
            let mut after = rest.split_off(&(column + 1, (0, 0)));
            held.append(&mut after);
            rest.into_values().collect()
        };
        let Some(first) = parts.first() else {
            return Ok(None);
        };
        let (kind, listed) = (first.kind.clone(), first.counts.is_some());
        if parts
            .iter()
            .any(|part| part.kind != kind || part.counts.is_some() != listed)
        {
            return Err(invalid(format!("the spooled pieces of column {column} differ in kind or shape: spool every piece of a column as one kind, flat or in lists alike")));
        }
        Ok(Some(Spooled {
            spool: self,
            parts,
            kind,
            listed,
        }))
    }
}

/// The pieces a spool held of one column, read back as they are packed.
pub struct Spooled<'a> {
    spool: &'a Spool,
    parts: Vec<Part>,
    kind: String,
    listed: bool,
}

impl Spooled<'_> {
    /// How many bytes its pieces hold, unpacked.
    pub(super) fn weight(&self) -> usize {
        let bytes = |span: &Span| span.bytes;
        self.parts
            .iter()
            .map(|part| {
                part.lanes.iter().map(bytes).sum::<usize>() + part.counts.as_ref().map_or(0, bytes)
            })
            .sum()
    }
}

impl Pieces for Spooled<'_> {
    fn kind(&self) -> &str {
        &self.kind
    }
    fn pieces(&self) -> usize {
        self.parts.len()
    }
    fn values(&self, piece: usize) -> usize {
        self.parts[piece].values
    }
    fn lane(&self, piece: usize, lane: usize) -> Result<Cow<'_, [u8]>> {
        Ok(Cow::Owned(self.spool.get(&self.parts[piece].lanes[lane])?))
    }
    fn counts(&self, piece: usize) -> Result<Option<Cow<'_, [usize]>>> {
        let Some(span) = &self.parts[piece].counts else {
            return Ok(None);
        };
        let raw = self.spool.get(span)?;
        let (held, _) = raw.as_chunks::<8>();
        Ok(Some(Cow::Owned(
            held.iter()
                .map(|n| u64::from_le_bytes(*n) as usize)
                .collect(),
        )))
    }
    fn listed(&self) -> bool {
        self.listed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::array;

    /// Every value of a spooled column, little-endian, and its counts.
    fn joined(held: &Spooled) -> (Vec<u8>, Vec<usize>) {
        let w = width(held.kind()).unwrap();
        let (mut bytes, mut counts) = (Vec::new(), Vec::new());
        for piece in 0..held.pieces() {
            let lanes: Vec<_> = (0..w).map(|lane| held.lane(piece, lane).unwrap()).collect();
            for value in 0..held.values(piece) {
                bytes.extend(lanes.iter().map(|lane| lane[value]));
            }
            counts.extend(held.counts(piece).unwrap().unwrap_or_default().iter());
        }
        (bytes, counts)
    }

    #[test]
    fn pieces_come_back_in_their_order() {
        let dir = std::env::temp_dir().join(format!("rowver-spool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spool = Spool::create(&dir.join("spool")).unwrap();
        let jagged = |counts: Vec<usize>, values: &[f32]| Values::Jagged {
            counts,
            content: array(values),
        };
        spool
            .add(1, (1, 0), jagged(vec![2, 0], &[3.0, 4.0]))
            .unwrap();
        spool.add(0, (0, 1), Values::Flat(array(&[7_i16]))).unwrap();
        spool.add(1, (0, 3), jagged(vec![1], &[1.0])).unwrap();
        spool
            .add(0, (0, 0), Values::Flat(array(&[5_i16, 6])))
            .unwrap();
        spool
            .add(0, (2, 0), Values::Flat(array::<i16>(&[])))
            .unwrap();
        assert!(spool.add(0, (0, 0), Values::Flat(array(&[5_i16]))).is_err());
        let flat = spool.take(0).unwrap().unwrap();
        assert_eq!(joined(&flat).0, array(&[5_i16, 6, 7]).bytes);
        let listed = spool.take(1).unwrap().unwrap();
        assert!(listed.listed());
        assert_eq!(
            joined(&listed),
            (array(&[1.0_f32, 3.0, 4.0]).bytes, vec![1, 2, 0])
        );
        assert!(spool.take(1).unwrap().is_none() && spool.take(2).unwrap().is_none());
        spool.add(2, (0, 0), Values::Flat(array(&[1_u8]))).unwrap();
        spool.add(2, (1, 0), Values::Flat(array(&[1_i8]))).unwrap();
        assert!(spool.take(2).is_err(), "pieces of two kinds");
        drop((flat, listed));
        drop(spool);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_spool_reopened_holds_the_pieces_it_is_given_and_no_others() {
        let dir = std::env::temp_dir().join(format!("rowver-reopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spool");
        let jagged = |counts: Vec<usize>, values: &[f32]| Values::Jagged {
            counts,
            content: array(values),
        };
        // Inputs 0 and 2 finished; input 1 did not, and lies between them.
        let spool = Spool::create(&path).unwrap();
        spool.add(0, (0, 0), Values::Flat(array(&[5_i16, 6]))).unwrap();
        spool.add(1, (0, 0), jagged(vec![2, 0], &[3.0, 4.0])).unwrap();
        spool.add(0, (1, 0), Values::Flat(array(&[9_i16]))).unwrap();
        spool.add(0, (2, 0), Values::Flat(array(&[7_i16]))).unwrap();
        spool.add(0, (2, 1), Values::Flat(array(&[8_i16]))).unwrap();
        spool.add(0, (3, 0), Values::Flat(array(&[4_i16]))).unwrap();
        let (first, third) = (spool.held(&[0]), spool.held(&[2]));
        assert_eq!(first.as_array().unwrap().len(), 2, "both columns of input 0");
        spool.sync().unwrap();
        drop(spool);
        assert!(path.exists(), "a spool made with create stays");
        let length = std::fs::metadata(&path).unwrap().len();
        let again = Spool::reopen(&path, &[&first, &third]).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < length, "cut after input 2, input 3 gone");
        // Input 1 was not given: its piece is passed over, and can be spooled again.
        again.add(0, (1, 0), Values::Flat(array(&[1_i16]))).unwrap();
        assert!(again.add(0, (2, 0), Values::Flat(array(&[1_i16]))).is_err());
        assert_eq!(joined(&again.take(0).unwrap().unwrap()).0, array(&[5_i16, 6, 1, 7, 8]).bytes);
        assert_eq!(
            joined(&again.take(1).unwrap().unwrap()),
            (array(&[3.0_f32, 4.0]).bytes, vec![2, 0])
        );
        drop(again);
        let mut beyond = first.clone();
        beyond[0][5][0][0] = 1_000_000.into();
        assert!(Spool::reopen(&path, &[&beyond]).is_err(), "a piece past the spool's end");
        assert!(Spool::reopen(&path, &[&first, &first]).is_err(), "a piece named twice");
        let mut short = first.clone();
        short[0][5].as_array_mut().unwrap().pop();
        assert!(Spool::reopen(&path, &[&short]).is_err(), "an i2 piece with one lane");
        assert!(Spool::reopen(&path, &[&Value::Null]).is_err(), "a line naming no pieces");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
