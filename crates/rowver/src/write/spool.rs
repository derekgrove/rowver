//! A file's columns gathered a piece at a time, held compressed on disk until each is packed,
//! so what is held in memory does not grow with the rows.

use super::column::{read_at, write_at, Pieces};
use super::Values;
use crate::numeric::width;
use crate::{codec, invalid, Result};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where a piece goes among its column's pieces: which input, then which piece of it.
pub type Order = (usize, usize);

/// Where one run of bytes lies in the spool, how many bytes it holds, and whether it is
/// compressed.
#[derive(Debug)]
struct Span {
    at: u64,
    size: usize,
    bytes: usize,
    packed: bool,
}

/// How much of a run is compressed first to see whether the rest is worth compressing.
const PROBE: usize = 16 << 10;

/// One piece of a column: each byte lane of its values, and for a list column each row's count.
#[derive(Debug)]
struct Part {
    kind: String,
    values: usize,
    lanes: Vec<Span>,
    counts: Option<Span>,
}

/// Pieces of columns in a file of their own beside the file they will be written into, deleted
/// when dropped. Pieces are added in any order and packed in the order of their `order`.
pub struct Spool {
    path: PathBuf,
    file: std::fs::File,
    /// How many bytes it holds.
    end: Mutex<u64>,
    parts: Mutex<BTreeMap<(usize, Order), Part>>,
}

impl Spool {
    /// An empty spool at `path` with `.spool` added to its name.
    pub fn beside(path: &Path) -> Result<Self> {
        let mut name = path.as_os_str().to_owned();
        name.push(".spool");
        let path = PathBuf::from(name);
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            end: Mutex::new(0),
            parts: Mutex::new(BTreeMap::new()),
        })
    }

    /// Add `bytes` at the end, compressed when `compress` and a sample of them shrinks by a
    /// fifth: the low bytes of measured values are mostly noise, which costs time to compress
    /// for little.
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
        read_at(&self.file, &mut packed, span.at)?;
        match span.packed {
            true => codec::unpack(packed, 1, span.bytes, "a spooled piece"),
            false => Ok(packed),
        }
    }

    /// Hold a piece of column `column`, placed among its pieces by `order`.
    pub fn add(&self, column: usize, order: Order, values: Values) -> Result<()> {
        let (buffer, counts) = match values {
            Values::Flat(buffer) => (buffer, None),
            Values::Jagged { counts, content } => (content, Some(counts)),
        };
        let w = width(&buffer.kind)?;
        if buffer.count.checked_mul(w) != Some(buffer.bytes.len()) {
            return Err(invalid("A spooled piece's bytes do not hold its values"));
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
            _ => Err(invalid("A piece was spooled twice")),
        }
    }

    /// Every piece of column `column`, in order, or `None` when it has none; the spool lets
    /// them go.
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
            return Err(invalid("A spooled column's pieces differ in kind"));
        }
        Ok(Some(Spooled {
            spool: self,
            parts,
            kind,
            listed,
        }))
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
    fn pieces_come_back_in_their_order_and_the_file_goes() {
        let dir = std::env::temp_dir().join(format!("rowver-spool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spool = Spool::beside(&dir.join("out.rvr")).unwrap();
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
        let path = spool.path.clone();
        assert!(path.exists());
        drop((flat, listed));
        drop(spool);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
