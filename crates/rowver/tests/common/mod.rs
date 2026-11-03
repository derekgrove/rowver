//! A small file every test of this crate reads: the columns, samples and study it holds, built
//! with the writer, and the bytes of it committed as `fixtures/rowver-v15.hex`; and the pieces a
//! test builds a file of.
#![allow(dead_code)]

use rowver::study::lock::{Branch, Branches, File, Lock, Sample, Shape};
use rowver::study::samples::Events;
use rowver::write::{array, made_checksum, sample_cols, stream, text_checksum, write_laid, Rows, Source, Values};
use serde_json::{json, Value};

/// One sample's rows before they are joined with the other samples'.
pub struct Piece {
    pub name: String,
    pub label: String,
    pub columns: Vec<(String, Values)>,
    pub entry: Vec<i64>,
}

/// The bytes of one `.rvr` file: the magic, the header's length and text, then the arrays.
pub fn to_bytes(header: &Value, arrays: &[Vec<u8>]) -> Vec<u8> {
    let text = serde_json::to_string(header).unwrap();
    let mut bytes = b"\x89RVR\r\n\x1a\n".to_vec();
    bytes.extend((text.len() as u32).to_le_bytes());
    bytes.extend(text.as_bytes());
    bytes.extend(arrays.concat());
    bytes
}

/// The data section of these pieces joined, in their order, as the writer streams it, and each
/// array's compressed bytes, in file order.
pub fn pack(pieces: Vec<Piece>, files: Vec<Value>) -> rowver::Result<(Value, Vec<Vec<u8>>)> {
    let refused = |why: String| rowver::Error::Invalid(why);
    let samples = pieces.iter().map(|p| rowver::write::Sample { name: p.name.clone(), label: p.label.clone(), rows: p.entry.len() }).collect();
    let entry: Vec<i64> = pieces.iter().flat_map(|p| p.entry.clone()).collect();
    let mut columns: Vec<(String, Values)> = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if index > 0 && piece.columns.len() != columns.len() {
            return Err(refused(format!("sample {} does not hold every column", piece.name)));
        }
        for (at, (name, values)) in piece.columns.into_iter().enumerate() {
            let rows = match &values {
                Values::Flat(flat) => flat.count,
                Values::Jagged { counts, .. } => counts.len(),
            };
            if rows != piece.entry.len() {
                return Err(refused(format!("{name}: needs one value per row of sample {}", piece.name)));
            }
            if index == 0 {
                columns.push((name, values));
                continue;
            }
            match (&mut columns[at], values) {
                ((held, _), _) if *held != name => return Err(refused(format!("sample {} holds {name} where the first holds {held}", piece.name))),
                ((_, Values::Flat(all)), Values::Flat(more)) if all.kind == more.kind => {
                    all.count += more.count;
                    all.bytes.extend(more.bytes);
                }
                ((_, Values::Jagged { counts, content }), Values::Jagged { counts: more, content: values }) if content.kind == values.kind => {
                    counts.extend(more);
                    content.count += values.count;
                    content.bytes.extend(values.bytes);
                }
                _ => return Err(refused(format!("{name}: samples differ in its kind"))),
            }
        }
    }
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rowver-test-{}-{n}-packed.rvr", std::process::id()));
    let rows = Rows { samples, files, entry: Source::Values(Values::Flat(array(&entry))) };
    let columns = columns.into_iter().map(|(name, values)| (name, Source::Values(values))).collect();
    let (data, laid) = stream(rows, columns, 2, &path)?;
    write_laid(&path, &json!({}), laid)?;
    let bytes = std::fs::read(&path)?;
    std::fs::remove_file(&path)?;
    let (_, mut rest) = split(&bytes);
    let mut sizes = vec![&data["sample"]["size"], &data["entry"]["size"]];
    for column in data["cols"].as_array().unwrap() {
        match column["kind"] == "jagged" {
            true => sizes.extend([&column["offsets"]["size"], &column["content"]["size"]]),
            false => sizes.push(&column["size"]),
        }
    }
    let mut arrays = Vec::new();
    for size in sizes {
        let more = rest.split_off(size.as_u64().unwrap() as usize);
        arrays.push(std::mem::replace(&mut rest, more));
    }
    Ok((data, arrays))
}

/// The committed fixture's bytes; `ROWVER_BLESS=1` writes them again from `rebuilt(10.)`.
pub fn golden() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/rowver-v15.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("ROWVER_BLESS").is_some() {
        let (header, arrays) = rebuilt(10.);
        let bytes = to_bytes(&header, &arrays);
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

/// Each column's branch: its kind and shape in the lock.
const BRANCHES: [(&str, &str, Shape); 12] = [
    ("nlep", "i4", Shape::Flat),
    ("MET", "f4", Shape::Flat),
    ("Muon_pt", "f4", Shape::Jagged),
    ("signed_i1", "i1", Shape::Flat),
    ("signed_i2", "i2", Shape::Flat),
    ("signed_i8", "i8", Shape::Flat),
    ("unsigned_u1", "u1", Shape::Flat),
    ("unsigned_u2", "u2", Shape::Flat),
    ("unsigned_u4", "u4", Shape::Flat),
    ("unsigned_u8", "u8", Shape::Flat),
    ("flag", "b1", Shape::Flat),
    ("double", "f8", Shape::Flat),
];

/// The fixture's recipe: every column copied from its branch.
pub fn recipe() -> String {
    let names: Vec<String> = BRANCHES.iter().map(|(name, _, _)| format!("{name:?}")).collect();
    format!("format = \"rvr-recipe\"\nversion = 1\ntitle = \"Synthetic reader test: π and μ\"\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [{}]\n\n[totals]\n\n[derived]\n", names.join(", "))
}

/// The fixture's samples.toml.
pub const SAMPLES: &str = r#"format = "rvr-samples"
version = 1

[places]

[defaults]
tree = "Events"

[eras]

[[sample]]
name = "A"
label = "Sample A, π⁰"
files = ["./fixture-a.root", "./fixture-empty.root", "./fixture-a2.root"]
events = 25
metadata = { xsec = 87.3, kind = "background", floats = [1e-4, 1e-5, 1e15, 1e16, 1e20, 1.1e-5, 0.0, -0.0, 1.2345678901234567, 1e-308, 1345740272956748.25, -101568684769612.625] }

[[sample]]
name = "B"
files = "./fixture-b.root"
events = 4
choose = "random"
seed = 7
metadata = { seed_like = 9223372036854775807 }
"#;

/// A file of the lock, `n` its place, with `entries` entries.
fn file(at: &str, n: u64, entries: u64) -> File {
    File {
        at: at.into(),
        bytes: 5000 + n,
        adler32: Some(format!("0a1b2c{n:02x}")),
        uuid: format!("31e851e4-b994-11f1-ba5c-{n:012x}"),
        entries,
        stop: None,
        take: None,
    }
}

/// The fixture's samples.lock, as `rvr samples resolve` would write it. Sample A reads the first
/// 25 entries of three files (the third stops at 5); sample B chooses 4 of its 20 at random,
/// seed 7, which are entries 1, 8, 10 and 17.
pub fn lock() -> String {
    lock_with(|_| {})
}

/// The fixture's lock with `edit` made to it, written as `rvr samples resolve` writes one.
pub fn lock_with(edit: impl Fn(&mut Lock)) -> String {
    let list = Branches::new(BRANCHES.iter().map(|(name, kind, shape)| Branch { name: (*name).into(), kind: (*kind).into(), shape: *shape, why: None }).collect());
    let samples = rowver::study::samples::read(SAMPLES).unwrap();
    let locked = |at: usize, files: Vec<File>| {
        let made = &samples[at];
        let paths: Vec<&str> = files.iter().map(|f| f.at.as_str()).collect();
        Sample {
            name: made.name.clone(),
            label: made.label.clone(),
            asked: made.files.clone(),
            tree: made.tree.clone(),
            branches: list.sha256.clone(),
            events: made.events,
            events_total: 0,
            events_read: 0,
            metadata: rowver::study::samples::file_filled(&made.metadata, &paths).unwrap(),
            files,
        }
        .planned(made.events)
    };
    let a = locked(0, vec![file("./fixture-a.root", 1, 10), file("./fixture-empty.root", 2, 10), file("./fixture-a2.root", 3, 10)]);
    let b = locked(1, vec![file("./fixture-b.root", 4, 20)]);
    assert_eq!(b.events, Events::Random { n: 4, seed: 7 });
    let mut lock = Lock {
        resolved: "2026-10-18T00:00:00Z".into(),
        samples_toml_sha256: text_checksum(SAMPLES),
        samples: vec![a, b],
        branches: vec![list],
    };
    edit(&mut lock);
    lock.write().unwrap()
}

/// A text the study holds, as `made` records it.
pub fn text(text: &str) -> Value {
    json!({"sha256": text_checksum(text), "text": text})
}

/// The fixture's header and arrays, with a chosen first muon.
pub fn rebuilt(muon: f32) -> (Value, Vec<Vec<u8>>) {
    let pieces = vec![
        Piece {
            name: "A".into(),
            label: "Sample A, π⁰".into(),
            columns: columns(0, 3, muon),
            entry: vec![0, 2, 4],
        },
        Piece {
            name: "B".into(),
            label: "B".into(),
            columns: columns(3, 5, muon),
            entry: vec![1, 17],
        },
    ];
    let files = vec![
        json!({"sample": "A", "path": "./fixture-a.root", "rows": 2}),
        json!({"sample": "A", "path": "./fixture-empty.root", "rows": 0}),
        json!({"sample": "A", "path": "./fixture-a2.root", "rows": 1}),
        json!({"sample": "B", "path": "./fixture-b.root", "rows": 2}),
    ];
    let (mut data, arrays) = pack(pieces, files).unwrap();
    let lock_text = lock();
    let lock = Lock::read(&lock_text).unwrap();
    let mut made_files = serde_json::Map::new();
    for sample in &lock.samples {
        let mut records = serde_json::Map::new();
        for file in &sample.files {
            let mut record = json!({"tree": sample.tree, "uuid": file.uuid, "entries": file.entries});
            if let Some(stop) = file.stop {
                record["stop"] = stop.into();
            }
            if let Some(take) = file.take {
                record["take"] = take.into();
            }
            records.insert(file.at.clone(), record);
        }
        let mut metadata = sample.metadata.clone();
        metadata.insert("events_read".into(), sample.events_read.into());
        metadata.insert("events_total".into(), sample.events_total.into());
        made_files.insert(sample.name.clone(), json!({"events": sample.events.record(), "files": records, "metadata": metadata}));
    }
    let names: Vec<String> = columns(0, 5, muon)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut recipe = text(&recipe());
    recipe["format"] = "rvr-recipe".into();
    recipe["columns"] = names.clone().into();
    let mut made = json!({
        "study": "native-reader-test",
        "title": "Synthetic reader test: π and μ",
        "built": "2026-10-18T00:00:00Z",
        "by": "rowver tests",
        "toolchain": {"rvr": "15.0.0", "rowver": "15", "commit": "0123456789abcdef0123456789abcdef01234567",
                      "rustc": "rustc 1.91.0 (f8297e351 2025-10-28)", "target": "aarch64-apple-darwin"},
        "recipe": recipe,
        "samples": text(SAMPLES),
        "lock": text(&lock_text),
        "files": made_files,
    });
    let taken: Vec<&str> = names.iter().map(String::as_str).collect();
    data["sample_cols"] = sample_cols(&["A", "B"], &made["files"], &taken)
        .unwrap()
        .into();
    made["sha256"] = made_checksum(&made).unwrap().into();
    let header =
        json!({"rowver": 15, "data": data, "made": made, "fixture_tool": {"preserved": "μ"}});
    (header, arrays)
}

/// A header with its sample columns made again from its metadata, and `made` sealed with its new
/// checksum, so that only the change made to it is wrong.
pub fn sealed(mut header: Value) -> Value {
    let names: Vec<String> = header["made"]["recipe"]["columns"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(str::to_owned)).collect();
    let taken: Vec<&str> = names.iter().map(String::as_str).collect();
    let samples: Vec<String> = header["data"]["samples"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_owned)).collect();
    let samples: Vec<&str> = samples.iter().map(String::as_str).collect();
    if let Ok(cols) = sample_cols(&samples, &header["made"]["files"], &taken) {
        header["data"]["sample_cols"] = cols.into();
    }
    header["made"]["sha256"] = made_checksum(&header["made"]).unwrap().into();
    header
}

/// Write bytes to a fresh file in the temporary directory, deleted when dropped.
pub struct Written(pub std::path::PathBuf);

impl Drop for Written {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl AsRef<std::path::Path> for Written {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

/// Bytes written to a fresh file in the temporary directory, deleted when the result is dropped.
pub fn written(bytes: &[u8], name: &str) -> Written {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("rowver-test-{}-{n}-{name}.rvr", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    Written(path)
}

/// A file's header and the bytes of its arrays.
pub fn split(bytes: &[u8]) -> (Value, Vec<u8>) {
    let start = 12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&bytes[12..start]).unwrap(),
        bytes[start..].to_vec(),
    )
}
