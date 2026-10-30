//! A small file every test of this crate reads: the columns, samples and study it holds, built
//! with the writer, and the bytes of it committed as `fixtures/rowver-v15.hex`.
#![allow(dead_code)]

use rowver::write::{
    array, made_checksum, pack, sample_cols, text_checksum, Labels, Piece, Values,
};
use serde_json::{json, Value};

/// The committed fixture's bytes; `ROWVER_BLESS=1` writes them again from `rebuilt(10.)`.
pub fn golden() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/rowver-v15.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("ROWVER_BLESS").is_some() {
        let (header, arrays) = rebuilt(10.);
        let bytes = rowver::write::to_bytes(&header, &arrays).unwrap();
        let hex: Vec<String> = bytes
            .chunks(32)
            .map(|line| line.iter().map(|b| format!("{b:02x}")).collect())
            .collect();
        std::fs::write(&path, hex.join("\n") + "\n").unwrap();
    }
    let text: String = std::fs::read_to_string(path)
        .unwrap()
        .split_whitespace()
        .collect();
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

/// The columns over the rows `from..to`, with a chosen first muon.
pub fn columns(from: usize, to: usize, muon: f32) -> Vec<(String, Values)> {
    let muons = [muon, 20., 30., 40., 50., 60.];
    let counts = [2_usize, 0, 1, 3, 0];
    let first: usize = counts[..from].iter().sum();
    let last: usize = counts[..to].iter().sum();
    let flat = |name: &str, values| (name.to_owned(), Values::Flat(values));
    vec![
        flat("nlep", array(&[2_i32, 0, 1, 3, 2][from..to])),
        flat("MET", array(&[20_f32, 0., 40., 30., 10.][from..to])),
        (
            "Muon_pt".into(),
            Values::Jagged {
                counts: counts[from..to].to_vec(),
                content: array(&muons[first..last]),
            },
        ),
        flat("signed_i1", array(&[-128_i8, -1, 0, 1, 127][from..to])),
        flat("signed_i2", array(&[-32768_i16, -1, 0, 1, 32767][from..to])),
        flat(
            "signed_i8",
            array(&[i64::MIN, -1, 0, (1 << 53) + 1, i64::MAX][from..to]),
        ),
        flat("unsigned_u1", array(&[0_u8, 1, 127, 128, 255][from..to])),
        flat(
            "unsigned_u2",
            array(&[0_u16, 1, 32767, 32768, 65535][from..to]),
        ),
        flat(
            "unsigned_u4",
            array(&[0_u32, 1, 1 << 31, u32::MAX - 1, u32::MAX][from..to]),
        ),
        flat(
            "unsigned_u8",
            array(&[0_u64, 1, (1 << 53) + 1, u64::MAX - 1, u64::MAX][from..to]),
        ),
        flat("flag", array(&[false, true, false, true, true][from..to])),
        flat(
            "double",
            array(&[-1.25_f64, 0., 1e-5, 1e16, 1.2345678901234567][from..to]),
        ),
    ]
}

/// A text the study holds, as `made` records it.
pub fn text(text: &str) -> Value {
    json!({"sha256": text_checksum(text), "text": text})
}

/// The fixture's header and arrays, with a chosen first muon. Sample A reads the first 25
/// entries of three files (the third stops at 5); sample B chooses 4 of its 20 at random, seed
/// 7, which are entries 1, 8, 10 and 17.
pub fn rebuilt(muon: f32) -> (Value, Vec<Vec<u8>>) {
    let pieces = vec![
        Piece {
            name: "A".into(),
            label: "Sample A, π⁰".into(),
            columns: columns(0, 3, muon),
            entry: vec![2, 0, 4],
        },
        Piece {
            name: "B".into(),
            label: "B".into(),
            columns: columns(3, 5, muon),
            entry: vec![17, 1],
        },
    ];
    let files = vec![
        json!({"sample": "A", "path": "fixture-a.root", "rows": 2}),
        json!({"sample": "A", "path": "fixture-empty.root", "rows": 0}),
        json!({"sample": "A", "path": "fixture-a2.root", "rows": 1}),
        json!({"sample": "B", "path": "fixture-b.root", "rows": 2}),
    ];
    let mut labels = Labels::new();
    labels.insert("flag".into(), [(0, "off".into()), (1, "on".into())].into());
    let (mut data, arrays) = pack(pieces, labels, files).unwrap();
    let record = |path: &str, entries: u64| json!({"tree": "Events", "uuid": format!("{}-uuid", path.trim_end_matches(".root")), "entries": entries});
    let mut stopped = record("fixture-a2.root", 10);
    stopped["stop"] = 5.into();
    let mut chosen = record("fixture-b.root", 20);
    chosen["take"] = 4.into();
    let names: Vec<String> = columns(0, 5, muon)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut recipe = text("# Synthetic reader test: π and μ\n");
    recipe["format"] = "rvr-recipe".into();
    recipe["columns"] = names.clone().into();
    let mut made = json!({
        "study": "native-reader-test", "built": "2026-10-18T00:00:00Z", "by": "rowver tests",
        "toolchain": {"rvr": "0.1.0", "rowver": "14", "commit": "fixture", "rustc": "rustc", "target": "any"},
        "recipe": recipe,
        "samples": text("[[sample]]\nname = \"A\"\n"),
        "lock": text("# the lock\n"),
        "files": {
            "A": {
                "events": {"choose": "first", "n": 25},
                "files": {
                    "fixture-a.root": record("fixture-a.root", 10),
                    "fixture-empty.root": record("fixture-empty.root", 10),
                    "fixture-a2.root": stopped,
                },
                "metadata": {"xsec": 87.3, "kind": "background", "floats": null},
            },
            "B": {
                "events": {"choose": "random", "n": 4, "seed": 7, "algorithm": rowver::choice::ALGORITHM},
                "files": {"fixture-b.root": chosen},
                "metadata": {"seed_like": 18446744073709551615_u64},
            },
        },
    });
    let floats = "[1e-4, 1e-5, 1e15, 1e16, 1e20, 1.1e-5, 0.0, -0.0, 1.2345678901234567, 1e-308, 1345740272956748.25, -101568684769612.625]";
    made["files"]["A"]["metadata"]["floats"] = serde_json::from_str(floats).unwrap();
    let taken: Vec<&str> = names.iter().map(String::as_str).collect();
    data["sample_cols"] = sample_cols(&["A", "B"], &made["files"], &taken)
        .unwrap()
        .into();
    made["sha256"] = made_checksum(&made).unwrap().into();
    let header =
        json!({"rowver": 15, "data": data, "made": made, "fixture_tool": {"preserved": "μ"}});
    (header, arrays)
}

/// Write bytes to a fresh file in the temporary directory; the caller removes it.
pub fn written(bytes: &[u8], name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("rowver-test-{}-{n}-{name}.rvr", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A file's header and the bytes of its arrays.
pub fn split(bytes: &[u8]) -> (Value, Vec<u8>) {
    let start = 12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&bytes[12..start]).unwrap(),
        bytes[start..].to_vec(),
    )
}
