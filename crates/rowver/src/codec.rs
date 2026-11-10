//! One zstd frame as a file holds it (docs/FORMAT.md §7): a chunk's byte lane, or a spooled piece's;
//! and the byte lanes of values.

use crate::{invalid, Result};
use std::cell::RefCell;
use zstd::zstd_safe::{self, CCtx, CParameter, DCtx};

/// The zstd level every frame is written at.
const LEVEL: i32 = 1;

std::thread_local! {
    /// This thread's compressor, at `LEVEL` and with a content checksum: each frame is one call of it.
    static PACKER: RefCell<CCtx<'static>> = RefCell::new({
        let mut packer = CCtx::create();
        // A level and a flag zstd has: neither can fail.
        packer.set_parameter(CParameter::CompressionLevel(LEVEL)).expect("zstd has this level");
        packer.set_parameter(CParameter::ChecksumFlag(true)).expect("zstd has content checksums");
        packer
    });
    /// This thread's decompressor.
    static UNPACKER: RefCell<DCtx<'static>> = RefCell::new(DCtx::create());
}

/// Whether zstd's failure `code` is `kind`: a failure is its kind's number, negated.
fn is(code: zstd_safe::ErrorCode, kind: zstd_safe::zstd_sys::ZSTD_ErrorCode) -> bool {
    code == (kind as usize).wrapping_neg()
}

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
/// when it has one, of `expected` bytes, those of what it `holds`.
pub(crate) fn frame(head: &[u8], expected: usize, place: &str, holds: &str) -> Result<()> {
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
        return Err(broken(place, &format!("declares {} bytes of content, and {holds} holds {expected}", declared.unwrap_or_default())));
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

/// Values `w` bytes wide put back together from their byte lanes, `n` values each.
pub(crate) fn merge<L: AsRef<[u8]>>(lanes: &[L], w: usize, n: usize) -> Vec<u8> {
    let mut values = vec![0; n * w];
    // Each width its own loop of known bounds; lanes that are not `w` of `n` bytes, as they come.
    let whole = lanes.len() == w && lanes.iter().all(|lane| lane.as_ref().len() == n);
    match w {
        2 if whole => fill::<2, L>(&mut values, lanes),
        4 if whole => fill::<4, L>(&mut values, lanes),
        8 if whole => fill::<8, L>(&mut values, lanes),
        _ => {
            for (lane, bytes) in lanes.iter().enumerate() {
                for (to, byte) in values.iter_mut().skip(lane).step_by(w).zip(bytes.as_ref()) {
                    *to = *byte;
                }
            }
        }
    }
    values
}

/// `values`, `W` bytes each, filled from their `W` lanes of as many bytes as there are values.
fn fill<const W: usize, L: AsRef<[u8]>>(values: &mut [u8], lanes: &[L]) {
    let n = values.len() / W;
    let lanes: [&[u8]; W] = std::array::from_fn(|lane| &lanes[lane].as_ref()[..n]);
    for (at, value) in values.as_chunks_mut::<W>().0.iter_mut().enumerate() {
        for (byte, lane) in value.iter_mut().zip(&lanes) {
            *byte = lane[at];
        }
    }
}

/// Bytes stored as one frame.
pub(crate) fn pack(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(zstd_safe::compress_bound(raw.len()));
    let packed = PACKER.with_borrow_mut(|packer| packer.compress2(&mut out, raw));
    // A buffer of the bound's size: it cannot fail.
    packed.unwrap_or_else(|code| panic!("zstd could not compress {} bytes: {}", raw.len(), zstd_safe::get_error_name(code)));
    out.shrink_to_fit();
    out
}

/// The bytes of exactly one zstd frame (docs/FORMAT.md §7) that fills `packed` and gives exactly
/// `expected` bytes, those of what it `holds` (as "its chunk's lane"), with the content checksum it states.
pub(crate) fn unpack(packed: &[u8], expected: usize, place: &str, holds: &str) -> Result<Vec<u8>> {
    if expected > most(packed.len()) {
        return Err(broken(place, &format!("is too small to hold {holds}")));
    }
    frame(packed, expected, place, holds)?;
    let fails = |code| broken(place, &format!("does not decode ({})", zstd_safe::get_error_name(code)));
    let size = zstd_safe::find_frame_compressed_size(packed).map_err(fails)?;
    if size != packed.len() {
        return Err(broken(place, &format!("ends {} bytes before {holds} does: it is exactly one frame", packed.len() - size)));
    }
    // One byte past the declared length, so a frame that holds more is caught.
    let mut out = Vec::with_capacity(expected + 1);
    let written = UNPACKER.with_borrow_mut(|unpacker| unpacker.decompress(&mut out, packed));
    let written = match written {
        Ok(written) => written,
        Err(code) => {
            use zstd_safe::zstd_sys::ZSTD_ErrorCode as Kind;
            return Err(match code {
                code if is(code, Kind::ZSTD_error_dstSize_tooSmall) => broken(place, &format!("decodes to more than the {expected} bytes {holds} holds")),
                code if is(code, Kind::ZSTD_error_checksum_wrong) => broken(place, "has a content checksum that is not that of its content"),
                code => fails(code),
            });
        }
    };
    if written != expected {
        return Err(broken(place, &format!("decodes to {written} bytes, and {holds} holds {expected}")));
    }
    Ok(out)
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
        let packed = pack(&raw);
        assert_eq!(unpack(&packed, raw.len(), "x", "its chunk's lane").unwrap(), raw);
        assert!(unpack(&packed, raw.len() - 1, "x", "its chunk's lane").is_err());
        assert!(unpack(&packed, raw.len() + 1, "x", "its chunk's lane").is_err());
        assert!(unpack(&packed[..packed.len() - 1], raw.len(), "x", "its chunk's lane").is_err());
        assert_eq!(
            shuffle(&[1, 2, 3, 4, 5, 6, 7, 8], 4),
            [1, 5, 2, 6, 3, 7, 4, 8]
        );
        assert_eq!(unpack(&pack(&[]), 0, "x", "its chunk's lane").unwrap(), Vec::<u8>::new());
        let zeros = vec![0; 1 << 20];
        assert_eq!(unpack(&pack(&zeros), zeros.len(), "x", "its chunk's lane").unwrap(), zeros);
    }

    // FORMAT 7
    #[test]
    fn a_frame_off_the_format_is_refused() {
        let raw: Vec<u8> = (0..300_u16).flat_map(u16::to_le_bytes).collect();
        let good = pack(&raw);
        let said = |bytes: Vec<u8>| unpack(&bytes, raw.len(), "x", "its chunk's lane").unwrap_err().to_string();
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
        // A frame with a window descriptor, as another zstd writes one: ruzstd's.
        let mut wide = ruzstd::encoding::compress_to_vec(&raw[..], ruzstd::encoding::CompressionLevel::Fastest);
        assert_eq!(unpack(&wide, raw.len(), "x", "its chunk's lane").unwrap(), raw);
        wide[5] = 18 << 3;
        assert!(said(wide).contains("window"));
    }

    // FORMAT 7
    #[test]
    fn a_window_of_two_to_the_27_is_taken_and_the_next_one_up_refused() {
        // A frame header with a checksum: a window descriptor (exponent, mantissa), or a single
        // segment whose window is its declared content size (8 bytes of it).
        let windowed = |descriptor: u8| [0x28, 0xB5, 0x2F, 0xFD, 0x04, descriptor];
        let single = |size: u64| [&[0x28, 0xB5, 0x2F, 0xFD, 0xE4][..], &size.to_le_bytes()].concat();
        let said = |head: &[u8], n: usize| frame(head, n, "x", "its chunk's lane").err().map(|e| e.to_string());
        assert_eq!(said(&windowed(17 << 3), 5), None, "2^27 exactly");
        let past = said(&windowed((17 << 3) | 1), 5).unwrap();
        assert!(past.starts_with("x: its zstd frame asks for a window of 150994944 bytes, more than 2^27 (docs/FORMAT.md §7): the file is damaged"), "{past}");
        assert_eq!(said(&single(1 << 27), 1 << 27), None);
        let past = said(&single((1 << 27) + 1), (1 << 27) + 1).unwrap();
        assert!(past.contains("asks for a window of 134217729 bytes, more than 2^27"), "{past}");
    }

    // FORMAT 7
    #[test]
    fn a_frame_is_refused_before_decoding_when_its_values_would_take_more_than_it_can_give() {
        let zeros = vec![0; 1 << 22];
        let packed = pack(&zeros);
        let bound = most(packed.len());
        assert_eq!(bound, packed.len() * 32_768 + 128 * 1024);
        let past = unpack(&packed, bound + 1, "x", "its chunk's lane").unwrap_err().to_string();
        assert!(past.contains("too small to hold its chunk's lane"), "{past}");
        // At the bound it is decoded, and refused only for what it decodes to.
        let at = unpack(&packed, bound, "x", "its chunk's lane").unwrap_err().to_string();
        assert!(!at.contains("too small"), "{at}");
    }
}
