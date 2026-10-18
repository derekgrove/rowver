//! `rvr press` on small ROOT files written by uproot and ROOT: the rows its cuts keep, the
//! columns it copies and computes, what it records, and the presses it refuses.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A study folder of its own: the fixtures beside a samples.toml and a recipe.toml.
fn study(name: &str, samples: &str, recipe: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rvr-press-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["uproot_flat.root", "stl_vectors.root", "codecs-zlib.root"] {
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file);
        std::fs::copy(from, dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("samples.toml"), samples).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
    dir
}

/// Run rvr in `dir`: its exit status, and what it printed and complained.
fn rvr(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

/// Resolve and press a study, and read back the file's header.
fn pressed(dir: &Path, flags: &[&str]) -> Value {
    let (code, said) = rvr(dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    let (code, said) = rvr(dir, &[&["press"], flags].concat());
    assert_eq!(code, 0, "{said}");
    let name = dir.file_name().unwrap().to_str().unwrap();
    let (code, said) = rvr(dir, &["validate", &format!("{name}.rvr")]);
    assert_eq!(code, 0, "{said}");
    let (_, header) = rvr(dir, &["header", &format!("{name}.rvr")]);
    serde_json::from_str(&header).unwrap()
}

/// The first values of each column, as `rvr dump --columns` prints them.
fn columns(dir: &Path) -> String {
    let name = dir.file_name().unwrap().to_str().unwrap();
    rvr(dir, &["dump", &format!("{name}.rvr"), "--columns"]).1
}

const FLAT: &str = r#"format = "rvr-samples"
version = 1

[[sample]]
name = "flat"
label = "Flat ntuple"
files = "uproot_flat.root"
tree = "analysis"
kind = "test"
"#;

#[test]
fn press_keeps_copies_and_computes() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[keep]
jets = "nJet >= 1"

[copy]
branches = ["weight", "nJet", "Jet_pt", "Muon_charge"]

[column]
ht = "sum(Jet_pt)"
"#;
    let dir = study("keep", FLAT, recipe);
    let header = pressed(&dir, &[]);
    let data = &header["data"];
    // nJet is 3, 4, 3, 0, 1: every entry but the fourth passes.
    assert_eq!(data["rows"], 4);
    assert_eq!(data["files"][0]["path"], "./uproot_flat.root");
    let dump = columns(&dir);
    assert!(
        dump.contains("entry          i8[4]  [0, 1, 2, 4]"),
        "{dump}"
    );
    assert!(
        dump.contains("weight         f8[4]  [1, 0.5, 2.25, 0.75]"),
        "{dump}"
    );
    assert!(
        dump.contains("[[65.3125, 19.96875, 16.421875], [51.75, 49.3125, 31.90625, 21.078125]"),
        "{dump}"
    );
    assert!(
        dump.contains("[[-1], [], [1, -1], [-1]]") || dump.contains("[[-1], [], [1, -1]"),
        "{dump}"
    );
    let ht = &data["cols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "ht")
        .unwrap();
    assert_eq!(ht["kind"], "f4");
    let made = &header["made"];
    assert_eq!(made["by"], concat!("rvr ", env!("CARGO_PKG_VERSION")));
    assert_eq!(made["cart"]["source"], recipe);
    let sample = &made["files"]["flat"];
    assert_eq!(sample["files"]["./uproot_flat.root"]["entries"], 5);
    assert!(sample["files"]["./uproot_flat.root"].get("stop").is_none());
    assert_eq!(sample["metadata"]["label"], "Flat ntuple");
    assert_eq!(sample["metadata"]["kind"], "test");
    assert_eq!(
        (
            sample["metadata"]["events_read"].clone(),
            sample["metadata"]["events_total"].clone()
        ),
        (5.into(), 5.into())
    );
    let names: Vec<&str> = data["sample_cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["kind", "label", "events_read", "events_total"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn events_reads_the_first_entries_and_records_where_it_stopped() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
    let dir = study("first", FLAT, recipe);
    let header = pressed(&dir, &["--events", "2"]);
    assert_eq!(header["data"]["rows"], 2);
    let sample = &header["made"]["files"]["flat"];
    assert_eq!(sample["files"]["./uproot_flat.root"]["stop"], 2);
    assert_eq!(sample["metadata"]["events_read"], 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn std_vectors_in_a_folder_are_pressed() {
    let samples = FLAT
        .replace("uproot_flat.root", "stl_vectors.root")
        .replace("\"analysis\"", "\"ntuple/Events\"");
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[keep]\ntwo = \"count(jet_pt) >= 2\"\n\n[copy]\nbranches = [\"evt\", \"jet_pt\", \"u16\"]\n";
    let dir = study("vectors", &samples, recipe);
    let header = pressed(&dir, &[]);
    assert_eq!(header["data"]["rows"], 2);
    let dump = columns(&dir);
    assert!(
        dump.contains("[[30.5, 31.5], [30.5, 31.5, 32.5]]"),
        "{dump}"
    );
    assert!(dump.contains("[1002, 1003]"), "{dump}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn presses_that_cannot_be_trusted_are_refused() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
    let dir = study("refused", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);

    let (code, said) = rvr(&dir, &["press", "--random"]);
    assert_eq!(code, 1);
    assert!(said.contains("waits for rowver 14"), "{said}");

    let select = recipe.replace("version = 1\n", "version = 1\nselect = \"kept\"\n");
    std::fs::write(dir.join("recipe.toml"), select).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1);
    assert!(
        said.contains("has no setting named select; it takes format, version"),
        "{said}"
    );
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("uproot_flat.root"))
        .unwrap();
    std::io::Write::write_all(&mut file, b"x").unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1);
    assert!(
        said.contains("has changed since samples.lock was written"),
        "{said}"
    );
    assert!(!dir
        .join(format!(
            "{}.rvr",
            dir.file_name().unwrap().to_str().unwrap()
        ))
        .exists());

    std::fs::write(dir.join("samples.toml"), format!("{FLAT}\n# edited\n")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1);
    assert!(
        said.contains("samples.toml has changed since samples.lock was written"),
        "{said}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_sample_of_empty_trees_is_written_with_no_rows() {
    // The two trees share no branch names, so each sample names its own through its metadata.
    let samples = r#"format = "rvr-samples"
version = 1

[[sample]]
name = "full"
label = "Full"
files = "codecs-zlib.root"
tree = "Events"
flat = "f4"
listed = "pt"

[[sample]]
name = "empty"
label = "Empty"
files = "codecs-zlib.root"
tree = "Empty"
flat = "x"
listed = "list"
"#;
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nvalue = \"branch(sample.flat)\"\nvalues = \"branch(sample.listed)\"\n";
    let dir = study("empty", samples, recipe);
    let header = pressed(&dir, &[]);
    let rows: Vec<u64> = header["data"]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["rows"].as_u64().unwrap())
        .collect();
    assert_eq!(rows, [7, 0]);
    let kinds: Vec<&str> = header["data"]["cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["f4", "jagged"]);
    assert_eq!(
        header["made"]["files"]["empty"]["metadata"]["events_total"],
        0
    );
    std::fs::remove_dir_all(dir).unwrap();
}
