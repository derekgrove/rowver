//! The rvr command on files written here: validate, header, dump, and diff naming what changed.

use rowver::study::lock::{Branch, Branches, File, Lock, Sample, Shape};
use rowver::write::{array, made_checksum, pack, sample_cols, text_checksum, write, Piece, Values};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A small pressed file: samples A (3 rows) and B (2 rows), a flat and a jagged column, and an
/// `xsec` sample column. `met_b` is MET's first value in sample B; `xsec_b` is B's cross-section.
fn pressed(path: &Path, met_b: f32, xsec_b: f64) {
    let piece = |name: &str, met: Vec<f32>, counts: Vec<usize>, pt: Vec<f32>| Piece {
        name: name.into(),
        label: format!("Sample {name}"),
        columns: vec![
            ("MET".into(), Values::Flat(array(&met))),
            (
                "Jet_pt".into(),
                Values::Jagged {
                    counts,
                    content: array(&pt),
                },
            ),
        ],
        entry: (0..met.len() as i64).collect(),
    };
    let pieces = vec![
        piece("A", vec![10., 20., 30.], vec![1, 0, 2], vec![40., 50., 60.]),
        piece("B", vec![met_b, 50.], vec![2, 1], vec![70., 80., 90.]),
    ];
    let files = vec![
        json!({"sample": "A", "path": "./a.root", "rows": 3}),
        json!({"sample": "B", "path": "./b.root", "rows": 2}),
    ];
    let (mut data, arrays) = pack(pieces, files).unwrap();
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"Events\"\n\n[eras]\n\n\
        [[sample]]\nname = \"A\"\nlabel = \"Sample A\"\nfiles = \"./a.root\"\nmetadata = {{ xsec = 1.5 }}\n\n\
        [[sample]]\nname = \"B\"\nlabel = \"Sample B\"\nfiles = \"./b.root\"\nmetadata = {{ xsec = {xsec_b:?} }}\n");
    let list = Branches::new(vec![
        Branch { name: "MET".into(), kind: "f4".into(), shape: Shape::Flat, why: None },
        Branch { name: "Jet_pt".into(), kind: "f4".into(), shape: Shape::Jagged, why: None },
    ]);
    let made_samples = rowver::study::samples::read(&samples).unwrap();
    let locked: Vec<Sample> = made_samples.iter().zip([3, 2]).enumerate().map(|(n, (s, entries))| Sample {
        name: s.name.clone(), label: s.label.clone(), asked: s.files.clone(), tree: s.tree.clone(),
        branches: list.sha256.clone(), events: s.events, events_total: 0, events_read: 0, metadata: s.metadata.clone(),
        files: vec![File { at: s.files[0].clone(), bytes: 4000, adler32: Some("0a1b2c3d".into()), uuid: format!("31e851e4-b994-11f1-ba5c-{n:012x}"), entries, stop: None, take: None }],
    }.planned(s.events)).collect();
    let lock = Lock { resolved: "2026-10-16T00:00:00Z".into(), samples_toml_sha256: text_checksum(&samples), samples: locked, branches: vec![list] }.write().unwrap();
    let lock = Lock::read(&lock).map(|read| (lock, read)).unwrap();
    let text = |text: &str| json!({"sha256": text_checksum(text), "text": text});
    let mut recipe = text("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"MET\", \"Jet_pt\"]\n\n[totals]\n\n[derived]\n");
    recipe["format"] = "rvr-recipe".into();
    recipe["columns"] = json!(["MET", "Jet_pt"]);
    let mut files = serde_json::Map::new();
    for sample in &lock.1.samples {
        let file = &sample.files[0];
        let mut metadata = sample.metadata.clone();
        metadata.insert("events_read".into(), sample.events_read.into());
        metadata.insert("events_total".into(), sample.events_total.into());
        files.insert(sample.name.clone(), json!({"events": {"choose": "all"}, "files": {file.at.clone(): {"tree": "Events", "uuid": file.uuid, "entries": file.entries}}, "metadata": metadata}));
    }
    let mut made = json!({
        "study": "cli-test", "built": "2026-10-16T00:00:00Z", "by": "rvr test",
        "toolchain": {"rvr": "15.0.0", "rowver": "15", "commit": "unknown", "rustc": "rustc 1.91.0", "target": "test"},
        "recipe": recipe, "samples": text(&samples), "lock": text(&lock.0), "files": files,
    });
    made["sha256"] = made_checksum(&made).unwrap().into();
    data["sample_cols"] = sample_cols(&["A", "B"], &made["files"], &["MET", "Jet_pt"])
        .unwrap()
        .into();
    write(
        path,
        &json!({"rowver": 15, "data": data, "made": made}),
        &arrays,
    )
    .unwrap();
}

/// A fresh scratch folder in the temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rvr-cli-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run rvr; its exit status and standard output.
fn rvr(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .env("RVR_CONFIG", std::env::temp_dir().join("rvr-cli-no-config"))
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn diff_names_the_column_and_sample_that_changed() {
    let dir = Scratch::new("diff");
    let (a, b) = (dir.0.join("a.rvr"), dir.0.join("b.rvr"));
    pressed(&a, 40., 2.5);
    pressed(&b, 41., 3.5);
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
    assert_eq!(rvr(&["diff", a, a]).0, 0);
    let (code, said) = rvr(&["diff", a, b]);
    assert_eq!(code, 1);
    assert_eq!(
        said,
        format!("column MET: differs in sample B\nsample column xsec: differs in sample B: 2.5 in {a}, 3.5 in {b}\n")
    );
}

#[test]
fn validate_header_and_dump_read_a_pressed_file() {
    let dir = Scratch::new("read");
    let path = dir.0.join("one.rvr");
    pressed(&path, 40., 2.5);
    let file = path.to_str().unwrap();
    let (code, said) = rvr(&["validate", file]);
    assert_eq!(
        (code, said),
        (0, format!("{file}: valid, 5 rows, 2 columns, read all 5 entries, not signed\n"))
    );
    // A flag it does not know is refused (on stderr), not read past.
    assert_eq!(rvr(&["validate", file, "--parent", file]), (2, String::new()));
    let (code, said) = rvr(&["header", file]);
    let header: Value = serde_json::from_str(&said).unwrap();
    assert_eq!((code, header["data"]["rows"].as_u64()), (0, Some(5)));
    let (code, said) = rvr(&["dump", file, "--columns"]);
    assert_eq!(code, 0);
    assert!(
        said.contains("MET            f4[5]  [10, 20, 30, 40, 50]"),
        "{said}"
    );
    assert!(
        said.contains("Jet_pt         jagged f4[5]  [[40], [], [50, 60], ...]"),
        "{said}"
    );
    assert!(
        said.contains("xsec           f8 by sample[2]  A: 1.5, B: 2.5"),
        "{said}"
    );
    // A damaged header is still shown, after a line saying what is wrong.
    let bytes = std::fs::read(&path).unwrap();
    let start = 12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut header: Value = serde_json::from_slice(&bytes[12..start]).unwrap();
    header["derived"] = 5.into();
    header["made"]["recipe"] = "not a record".into();
    let damaged = dir.0.join("damaged.rvr");
    write(&damaged, &header, &[bytes[start..].to_vec()]).unwrap();
    let (code, said) = rvr(&["dump", damaged.to_str().unwrap()]);
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains("is not valid") && said.contains("\"derived\": 5"),
        "{said}"
    );
    std::fs::write(&path, b"not an rvr file").unwrap();
    let (code, said) = rvr(&["validate", file]);
    assert_eq!(code, 1);
    assert!(said.starts_with(&format!("{file}: INVALID: ")), "{said}");
}

#[test]
fn new_makes_a_study_that_expands_and_never_overwrites() {
    let dir = Scratch::new("new");
    let study = dir.0.join("higgs_tautau");
    let at = study.to_str().unwrap();
    let (code, said) = rvr(&["new", at]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("rvr samples resolve samples.toml"), "{said}");
    for name in ["samples.toml", "recipe.toml", ".gitignore"] {
        assert!(study.join(name).is_file(), "{name}");
    }
    let recipe = std::fs::read_to_string(study.join("recipe.toml")).unwrap();
    assert!(recipe.contains("title = \"higgs_tautau\""));
    let (code, said) = rvr(&[
        "samples",
        "expand",
        study.join("samples.toml").to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains("describes 2 samples")
            && said.contains("GluGluToHToTauTau")
            && said.contains("VBF_HToTauTau"),
        "{said}"
    );
    std::fs::write(study.join("recipe.toml"), "mine").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(["new", at])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("writes nothing over a file"));
    assert_eq!(
        std::fs::read_to_string(study.join("recipe.toml")).unwrap(),
        "mine"
    );
}

#[test]
fn samples_alone_says_its_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(["samples"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("rvr samples resolve samples.toml"));
}
