//! Row-aligned chunks (docs/FORMAT.md §7): every array cut at the same rows, each chunk's byte
//! lanes one zstd frame each, then the array's chunk table; the checksums from the chunks' (§8).

use crate::rvr::hex;
use crate::{invalid, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The smallest zstd frame with a content checksum: the magic (4 bytes), its descriptor (1), a
/// one-byte window or content size (1), an empty last block (3), the checksum (4).
pub(crate) const FRAME: usize = 13;

/// One chunk: its sample, first row and end row.
pub(crate) type Rows = (usize, usize, usize);

/// The chunks of rows `edges` bounds (each sample's first row, then the end): within each sample,
/// every `r` rows from its first; a sample of no rows has none.
pub(crate) fn rows(edges: &[usize], r: usize) -> Vec<Rows> {
    let mut out = Vec::new();
    for (sample, pair) in edges.windows(2).enumerate() {
        let mut at = pair[0];
        while at < pair[1] {
            let to = at.saturating_add(r.max(1)).min(pair[1]);
            out.push((sample, at, to));
            at = to;
        }
    }
    out
}

/// The rows of one chunk a `data` section gives, 65,536 when it gives none that reads.
fn chunk_rows(data: &Value) -> usize {
    data["chunk_rows"].as_u64().filter(|r| *r > 0).map_or(rvr_recipe::CHUNK_ROWS, |r| r as usize)
}

/// How many chunks a `data` section's samples make, counted without making them; `None` when
/// its rows add up past what a count holds.
pub(crate) fn count(data: &Value) -> Option<u64> {
    let r = chunk_rows(data) as u64;
    data["samples"].as_array().into_iter().flatten().try_fold(0_u64, |sum, sample| sum.checked_add(sample["rows"].as_u64().unwrap_or(0).div_ceil(r)))
}

/// The chunks of a `data` section: its samples' rows cut every `chunk_rows`; refused when its
/// rows add up past what a count holds.
pub(crate) fn of(data: &Value) -> Result<Vec<Rows>> {
    let mut edges = vec![0_usize];
    for sample in data["samples"].as_array().into_iter().flatten() {
        let end = usize::try_from(sample["rows"].as_u64().unwrap_or(0)).ok().and_then(|rows| edges[edges.len() - 1].checked_add(rows));
        edges.push(end.ok_or_else(|| invalid("data.samples' rows add up past what a count holds (2^64): the file is damaged; copy it again, or press its study again"))?);
    }
    Ok(rows(&edges, chunk_rows(data)))
}

/// The bytes of one chunk table entry, for values `w` bytes wide: each lane's frame size, then the SHA-256.
pub(crate) fn entry(w: usize) -> usize {
    8 * w + 32
}

/// The SHA-256 of these SHA-256s' hex texts joined: an array's `sha256`, or one sample's `by_sample`.
pub(crate) fn joined<'a>(sums: impl IntoIterator<Item = &'a [u8; 32]>) -> String {
    let mut hash = Sha256::new();
    for sum in sums {
        hash.update(hex(sum).as_bytes());
    }
    hex(&hash.finalize())
}

/// An array's `sha256` and its `by_sample` (one per sample of `samples`), from its chunks' SHA-256s.
pub(crate) fn sums(chunks: &[Rows], hashes: &[[u8; 32]], samples: usize) -> (String, Vec<String>) {
    let mut by_sample = vec![Sha256::new(); samples];
    for (chunk, hash) in chunks.iter().zip(hashes) {
        if let Some(sample) = by_sample.get_mut(chunk.0) {
            sample.update(hex(hash).as_bytes());
        }
    }
    (joined(hashes), by_sample.into_iter().map(|hash| hex(&hash.finalize())).collect())
}

/// One chunk's values stored: each byte lane compressed as one frame.
pub(crate) fn frames(values: &[u8], w: usize) -> Vec<Vec<u8>> {
    let n = values.len() / w;
    let lanes = crate::codec::shuffle(values, w);
    (0..w).map(|lane| crate::codec::pack(&lanes[lane * n..(lane + 1) * n])).collect()
}

/// An array's chunk table as read, `w` lanes a chunk: each lane's frame size, each chunk's
/// SHA-256, and where each chunk's frames begin in the array (then where the last ends).
#[derive(Debug)]
pub(crate) struct Table {
    w: usize,
    lanes: Vec<u64>,
    sums: Vec<[u8; 32]>,
    starts: Vec<u64>,
}

impl Table {
    /// How many chunks it has.
    pub fn len(&self) -> usize {
        self.sums.len()
    }

    /// Where chunk `k`'s frames begin and end in the array.
    pub fn span(&self, k: usize) -> (u64, u64) {
        (self.starts[k], self.starts[k + 1])
    }

    /// Chunk `k`'s SHA-256.
    pub fn sum(&self, k: usize) -> [u8; 32] {
        self.sums[k]
    }

    /// Chunk `k`'s frame sizes, one a lane.
    pub fn sizes(&self, k: usize) -> &[u64] {
        &self.lanes[k * self.w..(k + 1) * self.w]
    }
}

/// A chunk's frames, one a lane: apart, or together as an array holds them, with each one's size.
pub(crate) enum Frames {
    Apart(Vec<Vec<u8>>),
    Together(Vec<u8>, Vec<u64>),
}

/// What a chunk table that does not fit its array is told.
fn misfit(place: &str, what: &str) -> crate::Error {
    invalid(format!("{place}: its chunk table {what} (docs/FORMAT.md §7): the file is damaged; copy it again, or press its study again"))
}

/// The table at the end of an array of `size` bytes, of `chunks` entries for values `w` bytes wide;
/// refused unless its lane sizes and its own bytes fill the array exactly.
pub(crate) fn table(bytes: &[u8], size: u64, w: usize, chunks: usize, place: &str) -> Result<Table> {
    if bytes.len() != chunks * entry(w) {
        return Err(misfit(place, &format!("is {} bytes, and its {chunks} chunks take {}", bytes.len(), chunks * entry(w))));
    }
    let mut table = Table { w, lanes: Vec::with_capacity(chunks * w), sums: Vec::with_capacity(chunks), starts: Vec::with_capacity(chunks + 1) };
    table.starts.push(0);
    for e in bytes.chunks_exact(entry(w)) {
        let lanes = e[..8 * w].chunks_exact(8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()));
        let size = lanes.clone().fold(0_u64, u64::saturating_add);
        table.lanes.extend(lanes);
        table.sums.push(e[8 * w..].try_into().unwrap_or_default());
        table.starts.push(table.starts[table.starts.len() - 1].saturating_add(size));
    }
    let frames = table.starts[chunks];
    if frames.checked_add(bytes.len() as u64) != Some(size) {
        return Err(misfit(place, &format!("gives its frames {frames} bytes, and with the table's own {} they do not fill the array's {size}", bytes.len())));
    }
    Ok(table)
}

/// Check an array's record against its table: its `sha256`, and its `by_sample` when it has one.
pub(crate) fn agree(record: &Value, table: &Table, chunks: &[Rows], samples: usize, place: &str) -> Result<()> {
    let (sha256, by_sample) = sums(chunks, &table.sums, samples);
    if record["sha256"].as_str() != Some(sha256.as_str()) {
        return Err(misfit(place, "does not give the array's sha256 (the SHA-256 of its chunks' SHA-256s joined, docs/FORMAT.md §8)"));
    }
    if let Some(recorded) = record.get("by_sample").and_then(Value::as_array) {
        if let Some(at) = recorded.iter().zip(&by_sample).position(|(have, want)| have.as_str() != Some(want.as_str())) {
            return Err(misfit(place, &format!("does not give by_sample[{at}] (the SHA-256 of that sample's chunks' SHA-256s joined, docs/FORMAT.md §8)")));
        }
    }
    Ok(())
}

/// Chunk `k`'s `n` values from its frames: each lane decoded to `n` bytes as §7 says, the lanes
/// put back together, and with `check` the values checked against the chunk's SHA-256.
pub(crate) fn decode(frames: &[u8], table: &Table, k: usize, n: usize, place: &str, check: bool) -> Result<Vec<u8>> {
    let w = table.w;
    let mut lanes = Vec::with_capacity(w);
    let mut at = 0_usize;
    for size in &table.lanes[k * w..(k + 1) * w] {
        let size = usize::try_from(*size).map_err(|_| misfit(place, "gives a frame more bytes than this computer holds"))?;
        let frame = frames.get(at..at.saturating_add(size)).ok_or_else(|| misfit(place, "gives its frames more bytes than the array holds"))?;
        lanes.push(crate::codec::unpack(frame, n, place, "its chunk's lane")?);
        at += size;
    }
    let values = match w {
        1 => lanes.pop().unwrap_or_default(),
        _ => crate::codec::merge(&lanes, w, n),
    };
    if check && Sha256::digest(&values)[..] != table.sums[k] {
        return Err(invalid(format!("{place}: a chunk's values are not the SHA-256 its chunk table states (docs/FORMAT.md §8): the file is damaged; copy it again, or press its study again")));
    }
    Ok(values)
}

/// The bytes an array's chunks decode to, `n(k)` values in chunk `k`: no more for a chunk than its
/// smallest frame could give, so a table that asks for more makes it refused, not held.
pub(crate) fn decoded(table: &Table, n: &dyn Fn(usize) -> usize) -> usize {
    let most = |k: usize| table.lanes[k * table.w..(k + 1) * table.w].iter().map(|lane| crate::codec::most(*lane as usize)).min().unwrap_or(0);
    (0..table.len()).fold(0_usize, |sum, k| sum.saturating_add(n(k).min(most(k)).saturating_mul(table.w)))
}

/// The values a chunk of a jagged column's offsets gives: `rows + 1` offsets, the first 0, never
/// decreasing; refused otherwise. The chunk's members: its last offset.
pub(crate) fn offsets(bytes: &[u8], place: &str) -> Result<(Vec<usize>, usize)> {
    let mut held = Vec::with_capacity(bytes.len() / 8);
    for (at, step) in bytes.chunks_exact(8).enumerate() {
        let offset = i64::from_le_bytes(step.try_into().unwrap_or_default());
        let fits = usize::try_from(offset).ok().filter(|offset| held.last().is_none_or(|last| last <= offset) && (at > 0 || *offset == 0));
        held.push(fits.ok_or_else(|| invalid(format!("{place}: a chunk's offsets must start at 0 and never decrease (docs/FORMAT.md §4, §7), and one gives {offset} at its value {at}: the file is damaged; copy it again, or press its study again")))?);
    }
    let members = *held.last().unwrap_or(&0);
    Ok((held, members))
}
