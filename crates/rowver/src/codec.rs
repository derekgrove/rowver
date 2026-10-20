//! One array as a file holds it: little-endian values, byte-shuffled, then one zstd frame.

use crate::{invalid, Result};
use ruzstd::decoding::FrameDecoder;
use ruzstd::encoding::{compress_to_vec, CompressionLevel};

/// The most bytes a zstd frame of `size` bytes is let to declare: a 128 KiB block can be one
/// byte repeated, so a damaged header cannot ask for more memory than its span could fill.
pub(crate) fn most(size: usize) -> usize {
    size.saturating_mul(1 << 15).saturating_add(1 << 17)
}

/// Every value's first byte, then every value's second byte, and so on.
pub(crate) fn shuffle(raw: &[u8], w: usize) -> Vec<u8> {
    let count = raw.len() / w;
    if w == 1 || count == 0 {
        return raw.to_vec();
    }
    let mut out = vec![0; raw.len()];
    for (lane, target) in out.chunks_exact_mut(count).enumerate() {
        for (byte, value) in target.iter_mut().zip(raw.chunks_exact(w)) {
            *byte = value[lane];
        }
    }
    out
}

/// Undo `shuffle`.
fn unshuffle(shuffled: Vec<u8>, w: usize) -> Vec<u8> {
    let count = shuffled.len() / w;
    if w == 1 || count == 0 {
        return shuffled;
    }
    let mut bytes = vec![0; shuffled.len()];
    for (lane, source) in shuffled.chunks_exact(count).enumerate() {
        for (value, byte) in bytes.chunks_exact_mut(w).zip(source) {
            value[lane] = *byte;
        }
    }
    bytes
}

/// The stored form of an array of values `w` bytes wide.
pub(crate) fn pack(raw: &[u8], w: usize) -> Vec<u8> {
    compress_to_vec(&shuffle(raw, w)[..], CompressionLevel::Fastest)
}

/// The little-endian bytes of a stored array, which must be zstd data giving exactly `expected`
/// bytes. The stored bytes are let go before the shuffle is undone.
pub(crate) fn unpack(packed: Vec<u8>, w: usize, expected: usize, label: &str) -> Result<Vec<u8>> {
    if expected > most(packed.len()) {
        return Err(invalid(format!(
            "{label}: declares more bytes than its compressed span can hold"
        )));
    }
    // One byte past the declared length, so data that holds more is caught.
    let mut shuffled = vec![0; expected + 1];
    let written = FrameDecoder::new()
        .decode_all(&packed, &mut shuffled)
        .map_err(|error| invalid(format!("{label}: invalid zstd data: {error}")))?;
    if written != expected {
        return Err(invalid(format!(
            "{label}: decoded length differs from the declared array"
        )));
    }
    drop(packed);
    shuffled.truncate(expected);
    Ok(unshuffle(shuffled, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrays_come_back_whole_and_a_wrong_length_is_refused() {
        let raw: Vec<u8> = (0..4000_u32)
            .flat_map(|v| (v * 7919).to_le_bytes())
            .collect();
        for w in [1, 2, 4, 8] {
            let packed = pack(&raw, w);
            assert_eq!(unpack(packed.clone(), w, raw.len(), "x").unwrap(), raw);
            assert!(unpack(packed.clone(), w, raw.len() - 1, "x").is_err());
            assert!(unpack(packed.clone(), w, raw.len() + 1, "x").is_err());
            assert!(unpack(packed[..packed.len() - 1].to_vec(), w, raw.len(), "x").is_err());
        }
        assert_eq!(
            shuffle(&[1, 2, 3, 4, 5, 6, 7, 8], 4),
            [1, 5, 2, 6, 3, 7, 4, 8]
        );
        assert_eq!(unpack(pack(&[], 8), 8, 0, "x").unwrap(), Vec::<u8>::new());
        let zeros = vec![0; 1 << 20];
        assert_eq!(unpack(pack(&zeros, 8), 8, zeros.len(), "x").unwrap(), zeros);
    }
}
