//! The answer a server gives when asked for several byte ranges at once: `multipart/byteranges`
//! (RFC 7233, appendix A). The ranges may come joined or reordered; far more bytes than asked are refused.

use std::io::{BufRead, BufReader, Read};

/// One part of an answer: where in the file its bytes begin, and the bytes.
#[derive(Debug, PartialEq)]
pub struct Part {
    pub at: u64,
    pub bytes: Vec<u8>,
}

/// The longest line of a part's headers that is read.
const LINE: u64 = 1024;
/// The most parts one answer may hold.
const PARTS: usize = 8192;

/// Find the boundary a `Content-Type: multipart/byteranges; boundary=...` header names.
pub fn boundary(content_type: &str) -> Option<String> {
    let named = content_type.split(';').find_map(|piece| {
        let (key, value) = piece.trim().split_once('=')?;
        key.eq_ignore_ascii_case("boundary").then_some(value)
    })?;
    let plain = named.trim().trim_matches('"');
    (!plain.is_empty() && plain.len() <= 200).then(|| plain.to_owned())
}

/// Read one line, without its ending; `None` at the end of the answer.
fn line(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut held = Vec::new();
    let read = reader.take(LINE).read_until(b'\n', &mut held);
    match read.map_err(|error| error.to_string())? {
        0 => Ok(None),
        _ if !held.ends_with(b"\n") && held.len() as u64 == LINE => {
            Err("the server sent a header line that is too long".into())
        }
        _ => Ok(Some(String::from_utf8_lossy(&held).trim_end().to_owned())),
    }
}

/// Read a `Content-Range` of `bytes first-last/whole` as the first byte and the length.
pub fn range(value: &str) -> Option<(u64, u64)> {
    let (span, _) = value.trim().strip_prefix("bytes")?.trim().split_once('/')?;
    let (first, last) = span.split_once('-')?;
    let (first, last) = (
        first.trim().parse::<u64>().ok()?,
        last.trim().parse::<u64>().ok()?,
    );
    let length = last.checked_sub(first)?.checked_add(1)?;
    Some((first, length))
}

/// Every part of an answer to a request for `asked` bytes in all, refused when it holds far more.
pub fn parts(reader: impl Read, boundary: &str, asked: u64) -> Result<Vec<Part>, String> {
    let allowed = asked.saturating_mul(2).saturating_add(1 << 20);
    // The whole answer is bounded, not only its parts: lines between them cost bytes too.
    let most = allowed.saturating_add(PARTS as u64 * 4 * LINE);
    let mut reader = BufReader::new(reader.take(most));
    let (open, close) = (format!("--{boundary}"), format!("--{boundary}--"));
    let (mut parts, mut held) = (Vec::new(), 0_u64);
    loop {
        // Blank lines stand between a part's bytes and the next boundary.
        let marker = loop {
            match line(&mut reader)? {
                None => break None,
                Some(found) if found.is_empty() => continue,
                Some(found) => break Some(found),
            }
        };
        match marker {
            None => return Ok(parts),
            Some(found) if found == close => return Ok(parts),
            Some(found) if found == open => {}
            Some(_) => return Err("the server's answer is not laid out as parts".into()),
        }
        let mut found = None;
        while let Some(header) = line(&mut reader)?.filter(|header| !header.is_empty()) {
            if let Some((name, value)) = header.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-range") {
                    found = range(value);
                }
            }
        }
        let (at, length) = found.ok_or("the server sent a part without its place")?;
        held = held.saturating_add(length);
        if held > allowed || parts.len() == PARTS {
            return Err("the server sent far more than was asked for".into());
        }
        let length = usize::try_from(length).map_err(|_| "a part is too large".to_owned())?;
        let mut bytes = vec![0; length];
        let read = reader.read_exact(&mut bytes);
        read.map_err(|error| format!("the server's answer was cut short ({})", error.kind()))?;
        parts.push(Part { at, bytes });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay parts out as a server does.
    fn answer(parts: &[(u64, &[u8])], end: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for (at, bytes) in parts {
            let last = at + bytes.len() as u64 - 1;
            let head = format!("\r\n--B\r\nContent-Type: application/octet-stream\r\nContent-range: bytes {at}-{last}/1000\r\n\r\n");
            out.extend(head.bytes().chain(bytes.iter().copied()));
        }
        if end {
            out.extend(b"\r\n--B--\r\n");
        }
        out
    }

    /// Parts are read with their places, including bytes that look like a boundary or a line.
    // SPEC 7.2.5
    #[test]
    fn parts_are_read_with_their_places() {
        assert_eq!(
            boundary("multipart/byteranges; boundary=EOS").as_deref(),
            Some("EOS")
        );
        assert_eq!(
            boundary("multipart/byteranges; Boundary=\"a b\"").as_deref(),
            Some("a b")
        );
        assert_eq!(boundary("application/octet-stream"), None);
        let tricky: &[u8] = b"\r\n--B\r\n\n\n--B--";
        let sent = answer(&[(10, b"abc"), (500, tricky), (0, &[0, 255, 10, 13])], true);
        let read = parts(&sent[..], "B", 30).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(
            (read[0].at, &read[0].bytes[..]),
            (10, &b"abc"[..])
        );
        assert_eq!((read[1].at, &read[1].bytes[..]), (500, tricky));
        assert_eq!(read[2].bytes, [0, 255, 10, 13]);
        assert_eq!(
            parts(&answer(&[(1, b"x")], false)[..], "B", 1)
                .unwrap()
                .len(),
            1
        );
        assert!(parts(&b""[..], "B", 0).unwrap().is_empty());
    }

    /// An answer that is cut short, misplaced, or far too large is refused.
    // SPEC 7.2.5
    #[test]
    fn broken_answers_are_refused() {
        let sent = answer(&[(10, b"abcdef")], true);
        assert!(parts(&sent[..sent.len() - 12], "B", 6)
            .unwrap_err()
            .contains("cut short"));
        assert!(parts(&sent[..], "other", 6)
            .unwrap_err()
            .contains("not laid out"));
        let unplaced = b"--B\r\nContent-Type: text/plain\r\n\r\nabc\r\n--B--\r\n";
        assert!(parts(&unplaced[..], "B", 3)
            .unwrap_err()
            .contains("without its place"));
        let huge = vec![7_u8; (3 << 20) + 5];
        let much = answer(&[(0, &huge)], true);
        assert!(parts(&much[..], "B", 4).unwrap_err().contains("far more"));
        let long = format!("--B\r\nX: {}\r\n\r\n", "y".repeat(3000));
        assert!(parts(long.as_bytes(), "B", 1)
            .unwrap_err()
            .contains("too long"));
        let backwards = b"--B\r\nContent-Range: bytes 9-3/10\r\n\r\n";
        assert!(parts(&backwards[..], "B", 1).is_err());
    }
}
