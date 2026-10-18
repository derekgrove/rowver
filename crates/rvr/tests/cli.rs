//! The rvr command on files written here: validate, header, dump, and diff naming what changed.

use rowver::write::{
    array, made_checksum, pack, sample_cols, text_checksum, write, Labels, Piece, Values,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A small pressed file: samples A (3 rows) and B (2 rows), a flat and a jagged column, and an
/// `xsec` sample column. `met_b` is MET's first value in sample B; `xsec_b` is B's cross-section.
fn pressed(path: &Path, met_b: f32, xsec_b: f64) {
    let piece = |name: &str, met: Vec<f32>, counts: Vec<usize>, pt: Vec<f32>| Piece {
        name: name.into(),
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
        json!({"sample": "A", "path": "a.root", "rows": 3}),
        json!({"sample": "B", "path": "b.root", "rows": 2}),
    ];
    let (mut data, arrays) = pack(pieces, Labels::new(), files).unwrap();
    let record = |entries: u64| json!({"tree": "Events", "uuid": "00000000-0000-0000-0000-000000000000", "entries": entries});
    let source = "format = \"renorm-definition\"\nversion = 1\n";
    let mut made = json!({
        "study": "cli-test", "built": "2026-10-16T00:00:00Z", "by": "rvr test",
        "cart": {"sha256": text_checksum(source), "source": source},
        "files": {
            "A": {"files": {"a.root": record(10)}, "metadata": {"xsec": 1.5}},
            "B": {"files": {"b.root": record(10)}, "metadata": {"xsec": xsec_b}},
        },
    });
    made["sha256"] = made_checksum(&made).unwrap().into();
    data["sample_cols"] = sample_cols(&["A", "B"], &made["files"], &["MET", "Jet_pt"])
        .unwrap()
        .into();
    write(
        path,
        &json!({"rowver": 13, "data": data, "made": made}),
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
        (0, format!("{file}: valid, 5 rows, 2 columns\n"))
    );
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
