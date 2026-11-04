//! One array as a file holds it: little-endian values, byte-shuffled, then one zstd frame.

use crate::{invalid, Result};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use ruzstd::encoding::{compress_to_vec, CompressionLevel};
use std::io::Read;

/// The most bytes a zstd frame of `size` bytes is let to declare: a 128 KiB block can be one
/// byte repeated, so a damaged header cannot ask for more memory than its span could fill.
pub(crate) fn most(size: usize) -> usize {
    size.saturating_mul(1 << 15).saturating_add(1 << 17)
}

/// The largest window a frame may ask for: 2^27 bytes, which every zstd library reads by default.
const WINDOW: u64 = 1 << 27;

/// The refusal of a frame that breaks docs/FORMAT.md §7.
fn broken(place: &str, what: &str) -> crate::Error {
    invalid(format!("{place}: its zstd frame {what} (docs/FORMAT.md §7): the file is damaged, or was not written as the format says; copy it again, or press its study again"))
}

/// Check a frame's header (RFC 8878 §3.1.1.1), the first bytes of its array: the zstd magic, no
/// dictionary, a content checksum, a window of at most 2^27 bytes, and a declared content size,
/// when it has one, of `expected` bytes.
pub(crate) fn frame(head: &[u8], expected: usize, place: &str) -> Result<()> {
    if head.get(..4) != Some(&[0x28, 0xB5, 0x2F, 0xFD][..]) {
        return Err(broken(place, "does not begin with the zstd magic 28 b5 2f fd (no skippable frame or other data comes first)"));
    }
    let short = || broken(place, "is cut short in its header");
    let descriptor = *head.get(4).ok_or_else(short)?;
    let single = descriptor & 0x20 != 0;
    if descriptor & 0x08 != 0 {
        return Err(broken(place, "sets the reserved bit of its header"));
    }
    if descriptor & 0x03 != 0 {
        return Err(broken(place, "has a dictionary ID (its dictionary ID flag is not 0), and an array uses no dictionary"));
    }
    if descriptor & 0x04 == 0 {
        return Err(broken(place, "has no content checksum (its checksum flag is 0)"));
    }
    let mut at = 5;
    let mut window = None;
    if !single {
        let byte = *head.get(5).ok_or_else(short)?;
        let base = 1_u64 << (10 + (byte >> 3));
        window = Some(base + base / 8 * u64::from(byte & 7));
        at = 6;
    }
    let width = match (descriptor >> 6, single) {
        (0, false) => 0,
        (0, true) => 1,
        (1, _) => 2,
        (2, _) => 4,
        _ => 8,
    };
    let declared = match width {
        0 => None,
        _ => {
            let bytes = head.get(at..at + width).ok_or_else(short)?;
            let mut le = [0; 8];
            le[..width].copy_from_slice(bytes);
            Some(u64::from_le_bytes(le) + if width == 2 { 256 } else { 0 })
        }
    };
    let window = window.or(declared).unwrap_or(0);
    if window > WINDOW {
        return Err(broken(place, &format!("asks for a window of {window} bytes, more than 2^27")));
    }
    if declared.is_some_and(|size| size != expected as u64) {
        return Err(broken(place, &format!("declares {} bytes of content, and its record's values take {expected}", declared.unwrap_or_default())));
    }
    Ok(())
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

/// The little-endian bytes of a stored array: exactly one zstd frame (docs/FORMAT.md §7) that
/// fills it and gives exactly `expected` bytes, with the content checksum it states. The stored
/// bytes are let go before the shuffle is undone.
pub(crate) fn unpack(packed: Vec<u8>, w: usize, expected: usize, place: &str) -> Result<Vec<u8>> {
    if expected > most(packed.len()) {
        return Err(broken(place, "is too small to hold the values its record states"));
    }
    frame(&packed, expected, place)?;
    // One byte past the declared length, so a frame that holds more is caught.
    let mut shuffled = vec![0; expected + 1];
    let mut decoder = FrameDecoder::new();
    let mut input = &packed[..];
    let fails = |e: &dyn std::fmt::Display| broken(place, &format!("does not decode ({e})"));
    decoder.init(&mut input).map_err(|e| fails(&e))?;
    let mut written = 0;
    loop {
        decoder.decode_blocks(&mut input, BlockDecodingStrategy::UptoBytes(1 << 20)).map_err(|e| fails(&e))?;
        written += decoder.read(&mut shuffled[written..])?;
        if decoder.can_collect() != 0 {
            return Err(broken(place, &format!("decodes to more than the {expected} bytes its record's values take")));
        }
        if decoder.is_finished() {
            break;
        }
    }
    if !input.is_empty() {
        return Err(broken(place, &format!("ends {} bytes before its array's size: an array is exactly one frame", input.len())));
    }
    if decoder.get_checksum_from_data() != decoder.get_calculated_checksum() {
        return Err(broken(place, "has a content checksum that is not that of its content"));
    }
    if written != expected {
        return Err(broken(place, &format!("decodes to {written} bytes, and its record's values take {expected}")));
    }
    drop(packed);
    shuffled.truncate(expected);
    Ok(unshuffle(shuffled, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    // FORMAT 7
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

    // FORMAT 7
    #[test]
    fn a_frame_off_the_format_is_refused() {
        let raw: Vec<u8> = (0..300_u16).flat_map(u16::to_le_bytes).collect();
        let good = pack(&raw, 2);
        let said = |bytes: Vec<u8>| unpack(bytes, 2, raw.len(), "x").unwrap_err().to_string();
        assert!(said([good.clone(), good.clone()].concat()).contains("exactly one frame"));
        let skippable = [&[0x50, 0x2A, 0x4D, 0x18, 0, 0, 0, 0][..], &good].concat();
        assert!(said(skippable).contains("magic"));
        let mut sum = good.clone();
        *sum.last_mut().unwrap() ^= 1;
        assert!(said(sum).contains("checksum"));
        let mut unchecked = good.clone();
        unchecked[4] &= !0x04;
        assert!(said(unchecked).contains("no content checksum"));
        let mut dictionary = good.clone();
        dictionary[4] |= 0x01;
        assert!(said(dictionary).contains("dictionary"));
        let mut wide = good.clone();
        wide[5] = 18 << 3;
        assert!(said(wide).contains("window"));
    }

    // FORMAT 7
    #[test]
    fn a_frame_is_refused_before_decoding_when_its_values_would_take_more_than_it_can_give() {
        let zeros = vec![0; 1 << 22];
        let packed = pack(&zeros, 1);
        let bound = most(packed.len());
        assert_eq!(bound, packed.len() * 32_768 + 128 * 1024);
        let past = unpack(packed.clone(), 1, bound + 1, "x").unwrap_err().to_string();
        assert!(past.contains("too small to hold the values its record states"), "{past}");
        // At the bound it is decoded, and refused only for what it decodes to.
        let at = unpack(packed, 1, bound, "x").unwrap_err().to_string();
        assert!(!at.contains("too small"), "{at}");
    }
}
