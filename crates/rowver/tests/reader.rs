//! The column reader against a file the Python RowVer 13.0.0 wrote, and against a damaged header.

use rowver::rvr::{from_bytes, Reader};
use serde_json::Value;

/// Decode a committed hexadecimal fixture.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text: String = std::fs::read_to_string(path)
        .unwrap()
        .split_whitespace()
        .collect();
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

/// A file's header and the bytes of its arrays.
fn split(bytes: &[u8]) -> (Value, Vec<u8>) {
    let start = 12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&bytes[12..start]).unwrap(),
        bytes[start..].to_vec(),
    )
}

/// Write bytes to a fresh file in the temporary directory; the caller removes it.
fn written(bytes: &[u8], name: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("rowver-reader-{}-{name}.rvr", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Little-endian values of a kind, each as an f64.
fn as_f64(kind: &str, bytes: &[u8]) -> Vec<f64> {
    let w = match kind {
        "f8" | "i8" | "u8" => 8,
        "f4" | "i4" | "u4" => 4,
        "i2" | "u2" => 2,
        _ => 1,
    };
    bytes
        .chunks_exact(w)
        .map(|b| match kind {
            "f8" => f64::from_le_bytes(b.try_into().unwrap()),
            "f4" => f32::from_le_bytes(b.try_into().unwrap()) as f64,
            "i8" => i64::from_le_bytes(b.try_into().unwrap()) as f64,
            "i4" => i32::from_le_bytes(b.try_into().unwrap()) as f64,
            "i2" => i16::from_le_bytes(b.try_into().unwrap()) as f64,
            "i1" => b[0] as i8 as f64,
            "u8" => u64::from_le_bytes(b.try_into().unwrap()) as f64,
            "u4" => u32::from_le_bytes(b.try_into().unwrap()) as f64,
            "u2" => u16::from_le_bytes(b.try_into().unwrap()) as f64,
            _ => b[0] as f64,
        })
        .collect()
}

fn names(header: &Value) -> Vec<String> {
    header["data"]["cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn reader_gives_the_columns_python_wrote() {
    let bytes = fixture("rowver-v13.hex");
    let table = from_bytes(&bytes).unwrap();
    let path = written(&bytes, "python");
    let reader = Reader::open(&path).unwrap();
    for name in names(table.header()) {
        let held = reader.column(&name).unwrap().unwrap();
        let column = table.column(&name).unwrap();
        let values = as_f64(&held.kind, &held.bytes);
        match &held.offsets {
            None => {
                let want: Vec<f64> = (0..table.len())
                    .map(|row| column.value(row).unwrap())
                    .collect();
                assert_eq!(values, want, "{name}");
            }
            Some(offsets) => {
                for row in 0..table.len() {
                    assert_eq!(
                        offsets[row + 1] - offsets[row],
                        column.object_count(row).unwrap(),
                        "{name}"
                    );
                }
                let want: Vec<f64> = (0..values.len())
                    .map(|i| column.content_value(i).unwrap())
                    .collect();
                assert_eq!(
                    values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "{name}"
                );
            }
        }
    }
    assert_eq!(reader.entry().unwrap().len(), table.len());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_header_declaring_more_rows_than_its_arrays_hold_is_refused_not_allocated() {
    let (mut header, arrays) = split(&fixture("rowver-v13.hex"));
    header["data"]["rows"] = (1_u64 << 40).into();
    let bytes = rowver::write::to_bytes(&header, &[arrays]).unwrap();
    let path = written(&bytes, "rows");
    // Opening may refuse the header; reading a column must refuse it rather than ask for 8 TB.
    if let Ok(reader) = Reader::open(&path) {
        assert!(reader.sample().is_err());
        for name in names(&header) {
            assert!(reader.column(&name).is_err(), "{name}");
        }
    }
    std::fs::remove_file(path).unwrap();
}
