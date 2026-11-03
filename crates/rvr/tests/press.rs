//! `rvr press` on small ROOT files written by uproot and ROOT: the rows its cuts keep, the
//! columns it copies and computes, what it records, and the presses it refuses.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A study folder of its own, deleted when the test ends, passed or failed.
struct Study(PathBuf);

impl std::ops::Deref for Study {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Study {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A study folder of its own: the fixtures beside a samples.toml and a recipe.toml.
fn study(name: &str, samples: &str, recipe: &str) -> Study {
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
    Study(dir)
}

/// Run rvr in `dir`: its exit status, and what it printed and complained.
fn rvr(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .current_dir(dir)
        .env("RVR_CONFIG", dir.join(".rvr-config"))
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

/// Run rvr in `dir` with `home` as the home folder.
fn rvr_at_home(dir: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .current_dir(dir)
        .env("RVR_CONFIG", dir.join(".rvr-config"))
        .env("HOME", home)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
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

[places]

[defaults]

[eras]

[[sample]]
name = "flat"
label = "Flat ntuple"
files = "uproot_flat.root"
tree = "analysis"
metadata = { kind = "test" }
"#;

#[test]
fn press_keeps_copies_and_computes() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["weight", "nJet", "Jet_pt", "Muon_charge"]
ht = "sum(Jet_pt)"

[totals]

[derived]
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
    assert_eq!(made["recipe"]["text"], recipe);
    assert_eq!(
        made["recipe"]["columns"],
        serde_json::json!(["weight", "nJet", "Jet_pt", "Muon_charge", "ht"])
    );
    assert_eq!(made["samples"]["text"], FLAT);
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    assert_eq!(made["lock"]["text"], lock);
    assert_eq!(made["toolchain"]["rvr"], env!("CARGO_PKG_VERSION"));
    assert_eq!(data["samples"][0]["label"], "Flat ntuple");
    let sample = &made["files"]["flat"];
    assert_eq!(sample["events"], serde_json::json!({"choose": "all"}));
    assert_eq!(sample["files"]["./uproot_flat.root"]["entries"], 5);
    assert!(sample["files"]["./uproot_flat.root"].get("stop").is_none());
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
    assert_eq!(names, ["kind", "events_read", "events_total"]);
}

#[test]
fn events_reads_the_first_entries_and_records_where_it_stopped() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("first", FLAT, recipe);
    let header = pressed(&dir, &["--events", "2"]);
    assert_eq!(header["data"]["rows"], 2);
    let sample = &header["made"]["files"]["flat"];
    assert_eq!(sample["files"]["./uproot_flat.root"]["stop"], 2);
    assert_eq!(sample["metadata"]["events_read"], 2);
    assert_eq!(
        sample["events"],
        serde_json::json!({"choose": "first", "n": 2})
    );
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (_, said) = rvr(&dir, &["validate", &file]);
    assert!(said.contains("read 2 of "), "{said}");
    // A second file the first N events never reach is not read, and validate counts it all the same.
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("again.root")).unwrap();
    let two = FLAT.replace(
        r#"files = "uproot_flat.root""#,
        r#"files = ["uproot_flat.root", "again.root"]"#,
    );
    std::fs::write(dir.join("samples.toml"), two).unwrap();
    let header = pressed(&dir, &["--events", "2"]);
    let entries = header["made"]["files"]["flat"]["metadata"]["events_total"]
        .as_u64()
        .unwrap();
    let (_, said) = rvr(&dir, &["validate", &file]);
    assert!(
        said.contains(&format!("read 2 of {entries} entries")),
        "{said}"
    );
}

#[test]
fn a_random_choice_is_recorded_checked_and_reproduced() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("random", FLAT, recipe);
    let header = pressed(&dir, &["--events", "3", "--random", "--seed", "5"]);
    let sample = &header["made"]["files"]["flat"];
    let algorithm = rowver::choice::ALGORITHM;
    assert_eq!(
        sample["events"],
        serde_json::json!({"choose": "random", "n": 3, "seed": 5, "algorithm": algorithm})
    );
    assert_eq!(sample["files"]["./uproot_flat.root"]["take"], 3);
    let chosen = rowver::choice::per_file(&[5], 3, 5).remove(0);
    let dump = columns(&dir);
    let entries = format!("entry          i8[3]  {chosen:?}");
    assert!(dump.contains(&entries), "{dump}");

    let name = dir.file_name().unwrap().to_str().unwrap();
    let (code, said) = rvr(&dir, &["reproduce", &format!("{name}.rvr")]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("The headers show no difference"), "{said}");
    let again = dir.join(format!("{name}.reproduced"));
    for name in ["recipe.toml", "samples.toml", "samples.lock"] {
        let (old, new) = (dir.join(name), again.join(name));
        assert_eq!(
            std::fs::read(old).unwrap(),
            std::fs::read(new).unwrap(),
            "{name}"
        );
    }

    // Moved away from its study, it says where to find the inputs, and finds them when told.
    let moved = dir.join("elsewhere");
    std::fs::create_dir(&moved).unwrap();
    std::fs::rename(dir.join(format!("{name}.rvr")), moved.join("moved.rvr")).unwrap();
    let (code, said) = rvr(&moved, &["reproduce", "moved.rvr"]);
    assert_eq!(code, 1);
    assert!(said.contains("name it with --study FOLDER"), "{said}");
    let (code, said) = rvr(&moved, &["reproduce", "moved.rvr", "--study", ".."]);
    assert_eq!(code, 0, "{said}");
}

#[test]
fn a_file_is_signed_once_there_is_a_key() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("signed", FLAT, recipe);
    let (code, said) = rvr(&dir, &["key", "new", "Tester"]);
    assert_eq!(code, 0, "{said}");
    let header = pressed(&dir, &[]);
    let (_, key) = rvr(&dir, &["key", "show"]);
    assert_eq!(header["signature"]["key"], key.trim());
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert_eq!(code, 0);
    assert!(said.ends_with("signed by you\n"), "{said}");
    let (code, said) = rvr(&dir, &["key", "new"]);
    assert_eq!(code, 1);
    assert!(said.contains("You already have a key"), "{said}");
    std::fs::remove_dir_all(dir.join(".rvr-config")).unwrap();
    let (_, said) = rvr(&dir, &["validate", &file]);
    assert!(said.contains("which you do not trust yet"), "{said}");
    let (code, said) = rvr(&dir, &["key", "trust", key.trim(), "Tester"]);
    assert_eq!(code, 0, "{said}");
    let (_, said) = rvr(&dir, &["validate", &file]);
    assert!(said.ends_with("signed by Tester (trusted)\n"), "{said}");
}

#[test]
fn std_vectors_in_a_folder_are_pressed() {
    let samples = FLAT
        .replace("uproot_flat.root", "stl_vectors.root")
        .replace("\"analysis\"", "\"ntuple/Events\"");
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\ntwo = \"count(jet_pt) >= 2\"\n\n[columns]\nbranches = [\"evt\", \"jet_pt\", \"u16\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("vectors", &samples, recipe);
    let header = pressed(&dir, &[]);
    assert_eq!(header["data"]["rows"], 2);
    let dump = columns(&dir);
    assert!(
        dump.contains("[[30.5, 31.5], [30.5, 31.5, 32.5]]"),
        "{dump}"
    );
    assert!(dump.contains("[1002, 1003]"), "{dump}");
}

#[test]
fn presses_that_cannot_be_trusted_are_refused() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("refused", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);

    let (code, said) = rvr(&dir, &["press", "--random"]);
    assert_eq!(code, 1);
    assert!(said.contains("--random needs a number of events"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--seed", "3"]);
    assert_eq!(code, 1);
    assert!(said.contains("--seed is for a random choice"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--events", "all", "--random"]);
    assert_eq!(code, 2);
    assert!(said.contains("not --events all"), "{said}");
    // A number of events from 1 to 2^63 − 1, refused at once as wrong use, before the study is read.
    for n in ["0", "9223372036854775808", "5__0"] {
        let (code, said) = rvr(&dir, &["press", "--events", n]);
        assert!(code == 2 && said.contains(&format!("--events is a number of events from 1 to 2^63 − 1, or all, not \"{n}\"")) && !said.contains("Pressing"), "{said}");
    }

    std::fs::rename(dir.join("samples.toml"), dir.join("aside.toml")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1);
    assert!(said.contains("no samples.toml"), "{said}");
    std::fs::rename(dir.join("aside.toml"), dir.join("samples.toml")).unwrap();

    let select = recipe.replace("version = 1\n", "version = 1\nselect = \"kept\"\n");
    std::fs::write(dir.join("recipe.toml"), select).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1);
    assert!(
        said.contains("recipe.toml: A recipe has no key \"select\"; it takes format, version"),
        "{said}"
    );
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("uproot_flat.root"))
        .unwrap();
    std::io::Write::write_all(&mut file, b"x").unwrap();
    // A file changed since the lock is a hole: the file is written, with it.
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("has changed since samples.lock was written") && said.contains("  changed     flat  "), "{said}");

    std::fs::write(dir.join("samples.toml"), format!("{FLAT}\n# edited\n")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1);
    assert!(
        said.contains("samples.toml has changed since samples.lock was written"),
        "{said}"
    );
}

#[test]
fn a_sample_of_empty_trees_is_written_with_no_rows() {
    // The two trees share no branch names, so each sample names its own through its metadata.
    let samples = r#"format = "rvr-samples"
version = 1

[places]

[defaults]

[eras]

[[sample]]
name = "full"
label = "Full"
files = "codecs-zlib.root"
tree = "Events"
metadata = { flat = "f4", listed = "pt" }

[[sample]]
name = "empty"
label = "Empty"
files = "codecs-zlib.root"
tree = "Empty"
metadata = { flat = "x", listed = "list" }
"#;
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nvalue = \"branch(sample.flat)\"\nvalues = \"branch(sample.listed)\"\n\n[totals]\n\n[derived]\n";
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
}

#[test]
fn a_file_of_no_entries_that_cannot_be_read_for_its_totals_stops_the_press() {
    let samples = r#"format = "rvr-samples"
version = 1

[places]

[defaults]

[eras]

[[sample]]
name = "full"
files = "codecs-zlib.root"
tree = "Events"
metadata = { flat = "f4" }

[[sample]]
name = "empty"
files = "nothing.root"
tree = "Empty"
metadata = { flat = "x" }
"#;
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nvalue = \"branch(sample.flat)\"\n\n[totals]\nn = \"1\"\n\n[derived]\n";
    let dir = study("nothing", samples, recipe);
    std::fs::copy(dir.join("codecs-zlib.root"), dir.join("nothing.root")).unwrap();
    // Read, it holds its totals: 0.0 over its no events.
    let header = pressed(&dir, &[]);
    assert_eq!(header["made"]["files"]["empty"]["files"]["./nothing.root"]["totals"]["n"], 0.0);
    // Gone, it has no entry to make a hole of: the press stops and writes nothing.
    std::fs::remove_file(dir.join("nothing.root")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    let needed = "The press reads it for the recipe's [totals]";
    assert!(code == 1 && said.contains("./nothing.root no longer exists: put it back") && said.contains(needed) && said.contains("was not written"), "{said}");
    // Another file in its place is said as it is, with what mends it.
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("nothing.root")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains("It has changed since samples.lock was written") && said.contains("run rvr samples resolve samples.toml") && said.contains(needed), "{said}");
}

#[test]
fn samples_whose_branch_lists_give_a_column_two_kinds_are_refused_before_any_file_is_read() {
    // The empty sample's tree has no entries, so no read would ever compare its kind.
    let samples = r#"format = "rvr-samples"
version = 1

[places]

[defaults]

[eras]

[[sample]]
name = "full"
files = "codecs-zlib.root"
tree = "Events"
metadata = { flat = "f8" }

[[sample]]
name = "empty"
files = "codecs-zlib.root"
tree = "Empty"
metadata = { flat = "x" }
"#;
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nvalue = \"branch(sample.flat)\"\n\n[totals]\n\n[derived]\n";
    let dir = study("kinds", samples, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("[columns] value is f8 in sample full and f4 in sample empty") && said.contains("stored = \"f8\"") && !said.contains("Pressing"), "{said}");
    assert!(!dir.join("kinds.rvr").exists());
    // Converted in the recipe, the column has one kind.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("value = \"branch(sample.flat)\"", "value = { value = \"branch(sample.flat)\", stored = \"f8\" }")).unwrap();
    assert_eq!(pressed(&dir, &[])["data"]["cols"][0]["kind"], "f8");
}

#[test]
fn derived_columns_are_kept_as_expressions_and_changed_by_rvr_derive() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["nJet", "Jet_pt"]
ht = "sum(Jet_pt)"

[totals]

[derived]
per_jet = "ht / nJet"
"#;
    let dir = study("derived", FLAT, recipe);
    let header = pressed(&dir, &[]);
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let record = &header["derived"]["columns"][0];
    assert_eq!((record["name"].as_str(), record["expression"].as_str()), (Some("per_jet"), Some("ht / nJet")));
    assert!(record.get("at").is_none() && record["lim"].is_array(), "{record}");
    let (_, said) = rvr(&dir, &["dump", &file, "--columns"]);
    assert!(said.contains("per_jet        f8[4]  derived = \"ht / nJet\"  ["), "{said}");
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && said.contains("4 rows, 3 columns, 1 derived,"), "{said}");
    // Added to the file alone: the recipe it carries gains the line.
    let (code, said) = rvr(&dir, &["derive", &file, "lead = first(Jet_pt, -1)", "twice = per_jet * 2"]);
    assert!(code == 0 && said.contains("Added lead, twice to") && said.contains("it has 3 derived columns"), "{said}");
    assert!(said.contains("Its study's recipe.toml is as it was"), "{said}");
    let (_, header) = rvr(&dir, &["header", &file]);
    assert!(header.contains("lead = \\\"first(Jet_pt, -1)\\\""), "{header}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // An expression is not changed in place; a derived column another uses goes with it, or after it.
    let (code, said) = rvr(&dir, &["derive", &file, "lead = 2 * ht"]);
    assert!(code == 1 && said.contains("drop it, then add it again"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--drop", "per_jet"]);
    assert!(code == 1 && said.contains("twice uses per_jet: drop twice first, or with it"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--rename", "per_jet", "mean"]);
    assert!(code == 1 && said.contains("twice uses per_jet"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--rename", "lead", "first_pt"]);
    assert!(code == 0 && said.contains("Renamed lead to first_pt in"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--drop", "per_jet", "twice"]);
    assert!(code == 0 && said.contains("it has 1 derived column;"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "bad = weight * 2"]);
    assert!(code == 1 && said.contains("the file does not store weight"), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // The study's recipe still asks for per_jet alone: status says so, and a press reads nothing.
    let (_, said) = rvr(&dir, &["status"]);
    assert!(said.contains("new derived: per_jet") && said.contains("dropped derived: first_pt"), "{said}");
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Nothing to read") && said.contains("3 columns and 1 derived"), "{said}");
    let (_, header) = rvr(&dir, &["header", &file]);
    let header: serde_json::Value = serde_json::from_str(&header).unwrap();
    assert_eq!(header["derived"]["columns"][0]["name"], "per_jet");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // A name a sample column has is refused, before a press reads anything, and by rvr derive.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("per_jet = ", "kind = ")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains("[derived] kind has the name of a key of samples.toml") && !said.contains("Pressing"), "{said}");
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
    let (code, said) = rvr(&dir, &["derive", &file, "--rename", "per_jet", "kind"]);
    assert!(code == 1 && said.contains("kind is already the name of a sample column or metadata key"), "{said}");
    // So is one a press gives every sample, though this file, with no holes, has no such column.
    let (code, said) = rvr(&dir, &["derive", &file, "events_missing = nJet"]);
    assert!(code == 1 && said.contains("or of one rvr press gives every sample"), "{said}");
    // A name given twice is dropped once; an expression written as recipe.toml writes it is that string.
    assert_eq!(rvr(&dir, &["derive", &file, "x = \"ht * 2\""]).0, 0);
    let (_, said) = rvr(&dir, &["dump", &file, "--columns"]);
    assert!(said.contains("derived = \"ht * 2\""), "{said}");
    // Single quotes are the recipe's own: 'nJet' is text, not the column nJet.
    let (code, said) = rvr(&dir, &["derive", &file, "t = 'nJet'"]);
    assert!(code == 1 && !said.contains("Added t"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--drop", "x", "x"]);
    assert!(code == 0 && said.contains("Dropped x from"), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // A derived column that names another through branch('…') uses it too.
    assert_eq!(rvr(&dir, &["derive", &file, "z = branch('per_jet') + 1"]).0, 0);
    let (code, said) = rvr(&dir, &["derive", &file, "--drop", "per_jet"]);
    assert!(code == 1 && said.contains("z uses per_jet"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--rename", "z", "ht"]);
    assert!(code == 1 && said.contains("ht is already the name of a stored column"), "{said}");
    let (code, said) = rvr(&dir, &["derive", &file, "--rename", "z", "a b"]);
    assert!(code == 1 && said.contains("\"a b\" is not a name a derived column can have"), "{said}");
    assert_eq!(rvr(&dir, &["derive", &file, "--drop", "z"]).0, 0);
    // What a press adds to every sample's metadata is no expression's to read: refused as it is read.
    let (code, said) = rvr(&dir, &["derive", &file, "n = sample.events_total"]);
    assert!(code == 1 && said.contains("[derived] n: sample.events_total is a count rvr press records of each sample, which no expression can read"), "{said}");
    // A key the sample lacks is named, with where to give it one.
    let (code, said) = rvr(&dir, &["derive", &file, "n = nJet * sample.xsec"]);
    assert!(code == 1 && said.contains("sample flat: derived column n: sample.xsec is used as a number, but this sample has no xsec: give it one in samples.toml, or give every sample one under [defaults]"), "{said}");
    // A record that is not what its expression gives is refused when it is worked out.
    let mut changed = header.clone();
    changed["derived"]["columns"][0]["sha256"] = "0".repeat(64).into();
    rowver::write::rewrite(dir.join(&file), dir.join("changed.rvr"), &changed).unwrap();
    let (code, said) = rvr(&dir, &["validate", "changed.rvr"]);
    assert!(code == 1 && said.contains("derived.columns[\"per_jet\"], worked out here, is not what"), "{said}");
    // Nor is it changed, and signed again, by rvr derive.
    let (code, said) = rvr(&dir, &["derive", "changed.rvr", "--rename", "per_jet", "mean"]);
    assert!(code == 1 && said.contains("is not what the file records"), "{said}");
    let (_, said) = rvr(&dir, &["dump", "changed.rvr", "--columns"]);
    assert!(said.contains("per_jet        derived = \"ht / nJet\"  (not worked out: "), "{said}");
    // Two derived columns that use each other, as an edited header could hold, are refused.
    let mut looped = header.clone();
    looped["derived"]["columns"][0]["expression"] = "per_jet + 1".into();
    rowver::write::rewrite(dir.join(&file), dir.join("looped.rvr"), &looped).unwrap();
    let (_, said) = rvr(&dir, &["dump", "looped.rvr", "--columns"]);
    assert!(said.contains("uses itself, directly or through other derived columns"), "{said}");
    // A header without the derived columns its recipe lists is refused.
    let mut stripped = header.clone();
    stripped.as_object_mut().unwrap().shift_remove("derived");
    rowver::write::rewrite(dir.join(&file), dir.join("stripped.rvr"), &stripped).unwrap();
    let (code, said) = rvr(&dir, &["validate", "stripped.rvr"]);
    assert!(code == 1 && said.contains("does not list the derived columns its header records"), "{said}");
    // A derived column that cannot be worked out is refused before anything is read.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("per_jet = \"ht / nJet\"", "per_jet = \"ht / sample.file\"")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains("[derived] per_jet: Text cannot be combined with a number: / takes numbers. (sample flat, over its stored columns and metadata)") && !said.contains("Pressing"), "{said}");
    // A float may be NaN or infinite: a derived column dividing by zero is written as it is.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("per_jet = \"ht / nJet\"", "per_jet = \"ht / (nJet - nJet)\"")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 0, "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
}

#[test]
fn a_column_added_to_the_recipe_is_read_alone_and_the_file_is_a_full_press() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["weight", "nJet"]

[totals]

[derived]
"#;
    let dir = study("add", FLAT, recipe);
    let first = pressed(&dir, &[]);
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let added = recipe.replace(
        r#"branches = ["weight", "nJet"]"#,
        "branches = [\"weight\", \"nJet\", \"Jet_pt\"]\nht = \"sum(Jet_pt)\"",
    );
    std::fs::write(dir.join("recipe.toml"), &added).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("and 2 of its 4 columns"), "{said}");
    assert!(
        said.contains("Reading only Jet_pt, ht for those events"),
        "{said}"
    );
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert_eq!(code, 0, "{said}");
    let built_on: Value = serde_json::from_str(&rvr(&dir, &["header", &file]).1).unwrap();
    // A full press of the same recipe writes the same rows, columns and records.
    let (code, said) = rvr(&dir, &["press", "--full", "--out", "full.rvr"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("kept after each cut"), "{said}");
    let full: Value = serde_json::from_str(&rvr(&dir, &["header", "full.rvr"]).1).unwrap();
    assert_eq!(built_on["data"], full["data"]);
    assert_eq!(built_on["made"]["files"], full["made"]["files"]);
    assert_eq!(built_on["made"]["recipe"], full["made"]["recipe"]);
    assert_eq!(built_on["data"]["rows"], first["data"]["rows"]);
    let (code, said) = rvr(&dir, &["diff", &file, "full.rvr"]);
    assert_eq!(code, 0, "{said}");

    // Nothing new: the file is current, left byte for byte (its time too), and no input is opened.
    let input = dir.join("uproot_flat.root");
    let bytes = std::fs::read(&input).unwrap();
    std::fs::remove_file(&input).unwrap();
    let (was, when) = (std::fs::read(dir.join(&file)).unwrap(), std::fs::metadata(dir.join(&file)).unwrap().modified().unwrap());
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!((code, said.trim()), (0, format!("{file} is current: nothing to press.").as_str()));
    assert_eq!((std::fs::read(dir.join(&file)).unwrap(), std::fs::metadata(dir.join(&file)).unwrap().modified().unwrap()), (was, when));
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 0 && said.contains("It is current"), "{said}");
    // Only [derived] changed: nothing is read, every input still gone.
    std::fs::write(dir.join("recipe.toml"), added.replace("[derived]\n", "[derived]\ntwice = \"ht * 2\"\n")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Nothing to read: every column is copied from it.") && said.contains("4 columns and 1 derived"), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    std::fs::write(&input, &bytes).unwrap();
    std::fs::write(
        dir.join("recipe.toml"),
        added.replace("nJet >= 1", "nJet >= 2"),
    )
    .unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains("the cuts under [keep], or what they use, have changed"),
        "{said}"
    );
    assert!(said.contains("kept after each cut"), "{said}");
}

/// A file's arrays: every byte after its header.
fn arrays(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let header = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    bytes[12 + header..].to_vec()
}

/// Press `dir`'s study in full into full.rvr with `flags`, and check the study's file is that file
/// but for when it was built and signed.
fn same_as_full(dir: &Path, flags: &[&str]) {
    let (code, said) = rvr(dir, &[&["press", "--full", "--out", "full.rvr"], flags].concat());
    assert!(code == 0 || code == 3, "{said}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(dir, &["validate", &name]);
    assert_eq!(code, 0, "{said}");
    let header = |file: &str| -> Value { serde_json::from_str(&rvr(dir, &["header", file]).1).unwrap() };
    let (built_on, full) = (header(&name), header("full.rvr"));
    assert_eq!(built_on["data"], full["data"]);
    assert_eq!(built_on["derived"], full["derived"]);
    let made = |header: &Value| {
        let mut made = header["made"].clone();
        for key in ["built", "sha256"] {
            made.as_object_mut().unwrap().remove(key);
        }
        made
    };
    assert_eq!(made(&built_on), made(&full));
    assert_eq!(arrays(&dir.join(&name)), arrays(&dir.join("full.rvr")));
    let (code, said) = rvr(dir, &["diff", &name, "full.rvr"]);
    assert_eq!(code, 0, "{said}");
}

/// Samples of FLAT's file, one per name, each with `kind` its name.
fn samples_of(names: &[&str]) -> String {
    let mut text = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n".to_string();
    for name in names {
        text += &format!("\n[[sample]]\nname = \"{name}\"\nlabel = \"{}\"\nfiles = \"flat_{name}.root\"\nmetadata = {{ kind = \"{name}\" }}\n", name.to_uppercase());
    }
    text
}

#[test]
fn a_sample_added_is_read_alone_and_the_file_is_a_full_press() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["weight", "nJet", "Jet_pt"]
ht = "sum(Jet_pt)"

[totals]

[derived]
double = "weight * 2"
"#;
    let dir = study("samples", &samples_of(&["a", "b"]), recipe);
    for copy in ["flat_a.root", "flat_b.root", "flat_c.root", "flat_d.root"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(copy)).unwrap();
    }
    pressed(&dir, &[]);
    // A sample added and resolved: rvr status and rvr press read only its files.
    std::fs::write(dir.join("samples.toml"), samples_of(&["a", "b", "c"])).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("rvr press would read only the new sample c (1 file), and copy the rows of every other sample from it."), "{said}");
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("holds the rows of 2 of the 3 samples as samples.lock has them."), "{said}");
    assert!(said.contains("Reading only the new sample c, 1 file, 5 entries, 1 at a time:"), "{said}");
    assert!(said.contains("  1/1            5 entries          4 kept   flat_c.root") && !said.contains("/2 "), "{said}");
    assert!(said.contains("Read 5 entries of the new sample c; kept after each cut:"), "{said}");
    same_as_full(&dir, &[]);
    // Taken out and reordered, the samples left are copied and nothing is read.
    std::fs::write(dir.join("samples.toml"), samples_of(&["c", "a"])).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("holds the rows of every sample as samples.lock has them.\nNothing to read: every row is copied from it."), "{said}");
    same_as_full(&dir, &[]);
    // A sample whose metadata changed is another sample: a full press, which says why.
    std::fs::write(dir.join("samples.toml"), samples_of(&["c", "a", "d"]).replace("kind = \"a\"", "kind = \"A\"")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("cannot be built on, as a is another sample in samples.lock now (its files, tree or metadata)."), "{said}");
    assert!(said.contains("Pressing 3 samples, 3 files"), "{said}");
    // So with a column added as well as a sample.
    std::fs::write(dir.join("samples.toml"), samples_of(&["c", "a", "d", "b"])).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe.replace("ht = ", "n = \"nJet * 2\"\nht = ")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("as samples.lock has changed since, and so have the columns (n new or changed): a press builds on a file for a change of either, not of both."), "{said}");
    // A random choice is built on when it is the same, and the derived columns changed with it.
    let (code, said) = rvr(&dir, &["press", "--events", "3", "--random", "--seed", "7"]);
    assert_eq!(code, 0, "{said}");
    std::fs::write(dir.join("samples.toml"), samples_of(&["c", "a", "d", "b", "e"]).replace("flat_e", "flat_*")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe.replace("ht = ", "n = \"nJet * 2\"\nht = ").replace("weight * 2", "weight * 3")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press", "--events", "3", "--random", "--seed", "7"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("Reading only the new sample e, 4 files, 3 entries"), "{said}");
    same_as_full(&dir, &["--events", "3", "--random", "--seed", "7"]);
}

#[test]
fn status_says_whether_the_file_is_current_and_what_press_would_read() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["weight", "nJet"]

[totals]

[derived]
"#;
    let dir = study("status", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("is not there: rvr press makes it"), "{said}");
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains("It is current: rvr press has nothing to read."),
        "{said}"
    );

    let added = recipe.replace(r#""nJet"]"#, r#""nJet", "Jet_pt"]"#);
    std::fs::write(dir.join("recipe.toml"), &added).unwrap();
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("recipe.toml   changed since: new: Jet_pt"),
        "{said}"
    );
    assert!(said.contains("rvr press would read only Jet_pt"), "{said}");
    let (code, said) = rvr(&dir, &["status", "--events", "10"]);
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("would press every column, as it chose other events of flat"),
        "{said}"
    );
    assert!(
        said.contains("events        the file read others"),
        "{said}"
    );

    std::fs::write(
        dir.join("recipe.toml"),
        added.replace("nJet >= 1", "nJet >= 2"),
    )
    .unwrap();
    let (_, said) = rvr(&dir, &["status"]);
    assert!(said.contains("the cuts under [keep] changed"), "{said}");
    std::fs::write(
        dir.join("samples.toml"),
        FLAT.replace("Flat ntuple", "Flat"),
    )
    .unwrap();
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("samples.toml has changed since samples.lock was written"),
        "{said}"
    );
    assert!(said.contains("rvr press would refuse:\n  samples.toml has changed since samples.lock was written"), "{said}");
    // A lock that does not read is said as its validator says it, not as samples.toml changed.
    std::fs::write(dir.join("samples.toml"), FLAT).unwrap();
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    std::fs::write(dir.join("samples.lock"), &lock[..lock.len() / 2]).unwrap();
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 1 && said.contains("rvr press would refuse:\n  samples.lock does not check out: ") && !said.contains("samples.toml has changed"), "{said}");
}

/// FLAT's file twice, as one sample: flat_a.root and flat_b.root.
const TWICE: &str = r#"format = "rvr-samples"
version = 1

[places]

[defaults]

[eras]

[[sample]]
name = "flat"
label = "Flat ntuple"
files = "flat_*.root"
tree = "analysis"
"#;

/// A study of TWICE.
fn twice(name: &str, recipe: &str) -> Study {
    let dir = study(name, TWICE, recipe);
    for copy in ["flat_a.root", "flat_b.root"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(copy)).unwrap();
    }
    dir
}

/// Damage the first basket of `branch` in a fixture file.
fn damage(path: &Path, branch: &str) {
    let (seek, nbytes) = {
        let file = hep_root::RootFile::open(hep_root::LocalFile::open(path).unwrap()).unwrap();
        let tree = file.tree_at("analysis").unwrap();
        let branch = tree.branch(branch).unwrap();
        (
            branch.basket_seek[0] as usize,
            branch.basket_bytes[0] as usize,
        )
    };
    let mut bytes = std::fs::read(path).unwrap();
    for b in &mut bytes[seek + nbytes - 12..seek + nbytes] {
        *b ^= 0x5a;
    }
    std::fs::write(path, &bytes).unwrap();
}

#[test]
fn a_derived_column_that_fails_past_its_trial_leaves_the_file_without_derived_columns() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\", \"nJet\"]\n\n[totals]\n\n[derived]\nr = \"if(sample.file == './flat_b.root', stored(weight * 1000, 'u1'), stored(weight, 'u1'))\"\n";
    let dir = twice("late", recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["key", "new", "Tester"]).0, 0);
    // One worker reads flat_a.root first, where the trial passes; flat_b.root's values fail at the end.
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert!(code == 1 && said.contains("derived column r, row 5: 1000.0 does not fit in u1") && said.contains("is written without its derived columns"), "{said}");
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && said.contains("10 rows, 2 columns, read all") && said.contains("signed by you"), "{said}");
    // Fixed, the next press reads no input and adds it.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("weight * 1000", "weight * 2")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Nothing to read") && said.contains("2 columns and 1 derived"), "{said}");
    // So with derived columns written as an inline table, which cannot be edited a line at a time.
    let inline = "format = \"rvr-recipe\"\nversion = 1\nderived = { r = \"if(sample.file == './flat_b.root', stored(weight * 1000, 'u1'), stored(weight, 'u1'))\" }\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\", \"nJet\"]\n\n[totals]\n";
    let dir = twice("late-inline", inline);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert!(code == 1 && said.contains("is written without its derived columns"), "{said}");
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // The recipe it carries keeps an empty [derived], so the next press builds on the file.
    std::fs::write(dir.join("recipe.toml"), inline.replace("weight * 1000", "weight * 2")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Nothing to read") && said.contains("2 columns and 1 derived"), "{said}");
    // rvr derive changes a [derived] table a line at a time, and says why it cannot change this one.
    let (code, said) = rvr(&dir, &["derive", &file, "y = weight * 3"]);
    assert!(code != 0 && said.contains("this one is written otherwise (inline"), "{said}");
}

#[test]
fn a_derived_column_of_another_kind_in_another_sample_is_refused_before_reading() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\nscaled = \"sample.w * nJet\"\n";
    let dir = twice("kinds", recipe);
    let samples = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n[[sample]]\nname = \"a\"\nlabel = \"A\"\nfiles = \"flat_a.root\"\nmetadata = { w = 1 }\n\n[[sample]]\nname = \"b\"\nlabel = \"B\"\nfiles = \"flat_b.root\"\nmetadata = { w = 0.5 }\n";
    std::fs::write(dir.join("samples.toml"), samples).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("[derived] scaled is i4 in sample a and f8 in sample b, and a derived column has one kind in every sample") && !said.contains("Pressing"), "{said}");
}

#[test]
fn a_damaged_basket_is_a_hole_named_with_its_file_tree_and_entries() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = twice("damaged", recipe);
    // Damaged before the lock is written: press checks size and adler32 first.
    damage(&dir.join("flat_b.root"), "weight");
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("flat_b.root, tree analysis, entries 0 to 4: "), "{said}");
    assert!(said.contains("in basket 0 of branch weight (entries 0 to "), "{said}");
    assert!(said.contains("The file is damaged there"), "{said}");
    assert!(said.contains("Read 5 of 10 entries"), "{said}");
    assert!(said.contains("with 1 hole:\n  damaged     flat  "), "{said}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (_, header) = rvr(&dir, &["header", &name]);
    let header: Value = serde_json::from_str(&header).unwrap();
    let file = &header["made"]["files"]["flat"];
    let hole = &file["files"]["./flat_b.root"]["holes"][0];
    assert_eq!((&hole["entries"], &hole["kind"]), (&serde_json::json!([0, 5]), &Value::from("damaged")));
    assert!(file["files"]["./flat_a.root"].get("holes").is_none());
    assert_eq!((&file["metadata"]["events_read"], &file["metadata"]["events_missing"]), (&5.into(), &5.into()));
    assert_eq!(header["data"]["rows"], 5);
    let (code, said) = rvr(&dir, &["validate", &name]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("read 5 of 10 entries, 1 hole (damaged), not signed"), "{said}");
    let (code, said) = rvr(&dir, &["validate", &name, "--complete"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(": INVALID: it has holes, and --complete asks for none"), "{said}");
    assert_eq!(said.lines().count(), 1, "{said}");
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(" 50%   2 files, 1 hole"), "{said}");
    assert!(said.contains("  damaged      1 file, 5 entries   flat"), "{said}");
    assert!(said.contains("rvr press --fill would read 5 entries of 1 file."), "{said}");
    // Pressed again as it is, it is filled, not pressed: press says so.
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("with 1 hole: rvr press --fill reads them again"), "{said}");
    // A column added is read for the events kept; the hole stays a hole.
    let more = recipe.replace("\n\n[totals]", "\ndouble = \"weight * 2\"\n\n[totals]");
    std::fs::write(dir.join("recipe.toml"), more).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("Reading only double for those events"), "{said}");
    let (code, said) = rvr(&dir, &["validate", &name]);
    assert!(code == 0 && said.contains("1 hole (damaged)"), "{said}");

    // The good file put back and resolved again, a fill reads only the hole, and the file is
    // the one a full press makes.
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("flat_b.root")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press", "--fill"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("reading 1 hole of 1 file, 5 entries"), "{said}");
    let filled: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    assert_eq!(rvr(&dir, &["validate", &name, "--complete"]).0, 0);
    std::fs::remove_file(dir.join(&name)).unwrap();
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let full: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    assert_eq!(filled["data"]["sha256"], full["data"]["sha256"]);
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 0 && said.contains("100%"), "{said}");
    assert_eq!(filled["made"]["files"], full["made"]["files"]);
    let (code, said) = rvr(&dir, &["press", "--fill"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("has no holes: there is nothing to fill"), "{said}");
}

#[test]
fn a_file_gone_or_changed_since_the_lock_is_a_hole_of_its_own() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = twice("gone", recipe);
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("flat_c.root")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::remove_file(dir.join("flat_b.root")).unwrap();
    std::fs::copy(dir.join("codecs-zlib.root"), dir.join("flat_c.root")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("with 2 holes:\n  gone        flat  ./flat_b.root no longer exists"), "{said}");
    assert!(said.contains("\n  changed     flat  "), "{said}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(&dir, &["validate", &name]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("read 5 of 15 entries, 2 holes (1 gone, 1 changed)"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--fill", "refused,unreachabel"]);
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("not \"unreachabel\" (did you mean --fill refused,unreachable?)"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--fill", "refused"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("has no holes of kind refused: its holes are changed, gone"), "{said}");
    // --fill gone reads only that kind: still gone, it fills nothing and leaves the file.
    let before = std::fs::read(dir.join(&name)).unwrap();
    let (code, said) = rvr(&dir, &["press", "--fill", "gone"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("reading 1 hole of 1 file"), "{said}");
    assert!(said.contains("None of the 1 hole could be read"), "{said}");
    assert_eq!(std::fs::read(dir.join(&name)).unwrap(), before);
}

#[test]
fn a_file_without_a_branch_the_recipe_reads_through_sample_metadata_is_a_hole() {
    let samples = FLAT
        .replace("files = \"uproot_flat.root\"", "files = [\"uproot_flat.root\", \"stl_vectors.root\"]")
        .replace("tree = \"analysis\"", "tree = \"ntuple/Events\"")
        .replace("{ kind = \"test\" }", "{ which = \"nJet\", note = \"run\" }");
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nn = \"branch(sample.which)\"\n\n[totals]\n\n[derived]\n";
    let dir = study("bysample", &samples, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("changed") && said.contains("stl_vectors.root") && said.contains("nJet missing"), "{said}");
    // A text of the sample's that the recipe does not read as a branch's name is not checked.
    assert!(!said.contains("run missing"), "{said}");
}

#[test]
fn a_fill_takes_the_choice_it_was_pressed_with_and_the_file_put_back() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = twice("refill", recipe);
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::rename(dir.join("flat_b.root"), dir.join("away.root")).unwrap();
    // The first 8 entries: all of flat_a, 3 of flat_b, which is gone.
    assert_eq!(rvr(&dir, &["press", "--events", "8"]).0, 3);
    // A recipe edited in a comment only is not a fill: the file is written again, holes and all.
    std::fs::write(dir.join("recipe.toml"), format!("{recipe}# weights only\n")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--events", "8"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("Nothing to read: every column is copied from it."), "{said}");
    // Put back as another ROOT file (its UUID changed) and resolved again: its UUID is the new one.
    let mut bytes = std::fs::read(dir.join("away.root")).unwrap();
    bytes[50] ^= 0x01;
    std::fs::write(dir.join("flat_b.root"), &bytes).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press", "--fill"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("fill it with the --events, --random and --seed it was pressed with"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--fill", "--events", "8"]);
    assert_eq!(code, 0, "{said}");
    let filled: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    std::fs::remove_file(dir.join(&name)).unwrap();
    assert_eq!(rvr(&dir, &["press", "--events", "8"]).0, 0);
    let full: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    assert_eq!(filled["made"]["files"], full["made"]["files"]);
    assert_eq!(filled["data"]["sha256"], full["data"]["sha256"]);
    let uuid = |at: &str| filled["made"]["files"]["flat"]["files"][at]["uuid"].clone();
    assert_ne!(uuid("./flat_b.root"), uuid("./flat_a.root"), "flat_b is another ROOT file now");
}

#[test]
fn samples_added_keep_the_holes_of_the_others_and_are_picked_up_when_stopped() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\", \"Jet_pt\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("added-holes", &samples_of(&["a", "b"]), recipe);
    for name in ["a", "b", "c", "d", "e", "f"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(format!("flat_{name}.root"))).unwrap();
    }
    damage(&dir.join("flat_a.root"), "weight");
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["press"]).0, 3);
    // Two samples added, one of whose files is gone: a's hole is the file's, d's is new.
    std::fs::write(dir.join("samples.toml"), samples_of(&["a", "b", "c", "d"])).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::rename(dir.join("flat_d.root"), dir.join("away.root")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("Reading only the new samples c, d, 2 files, 10 entries"), "{said}");
    assert!(said.contains("Read 5 of 10 entries of the new samples c, d;") && said.contains("with 2 holes:"), "{said}");
    same_as_full(&dir, &[]);
    // A press of samples added (and d taken out) that stopped part way is picked up where it stopped.
    if cfg!(debug_assertions) {
        std::fs::write(dir.join("samples.toml"), samples_of(&["a", "b", "c", "e", "f"])).unwrap();
        assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
        stopped_after(&dir, 1, &[]);
        let (code, said) = rvr(&dir, &["press"]);
        assert_eq!(code, 3, "{said}");
        assert!(said.contains("where it stopped: 1 of 2 files done") && said.contains("  2/2 ") && !said.contains("  1/2 "), "{said}");
        same_as_full(&dir, &[]);
    }
}

/// `rvr press` with `flags`, one file at a time, stopped as a crash would once `n` files are done.
fn stopped_after(dir: &Path, n: usize, flags: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args([&["press", "--workers", "1"], flags].concat())
        .current_dir(dir)
        .env("RVR_CONFIG", dir.join(".rvr-config"))
        .env("RVR_STOP_AFTER", n.to_string())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains(&format!("Stopped with {n} files done")), "{said}");
    said
}

/// The data checksum of a study's file.
fn data_sha256(dir: &Path) -> Value {
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let header: Value = serde_json::from_str(&rvr(dir, &["header", &name]).1).unwrap();
    header["data"]["sha256"].clone()
}

#[test]
fn a_press_stopped_part_way_is_picked_up_where_it_stopped() {
    // RVR_STOP_AFTER stops a debug build only.
    if !cfg!(debug_assertions) {
        return;
    }
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\njets = \"nJet >= 1\"\n\n[columns]\nbranches = [\"weight\", \"Jet_pt\"]\n\n[totals]\n\n[derived]\n";
    let dir = twice("resume", recipe);
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("flat_c.root")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let staged = dir.join(format!("{name}.partial"));
    let said = stopped_after(&dir, 1, &[]);
    assert!(!said.contains("Starting over") && !said.contains("Picking up"), "a first press is new: {said}");
    assert!(staged.join("journal.jsonl").exists() && !dir.join(&name).exists());
    let (code, said) = rvr(&dir, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(".rvr.partial: a press stopped there with 1 of 3 files done (journal from "), "{said}");
    // While a press holds the journal, another press of it, or a restart, is refused.
    let held = std::fs::File::open(staged.join("journal.jsonl")).unwrap();
    held.lock().unwrap();
    for flags in [&["press"][..], &["press", "--restart"]] {
        let (code, said) = rvr(&dir, flags);
        assert_eq!(code, 1, "{said}");
        assert!(said.contains(" is under way (it holds "), "{said}");
    }
    assert!(rvr(&dir, &["status"]).1.contains(".rvr.partial: a press is under way there now."));
    drop(held);
    // Another recipe is another press: it starts over.
    std::fs::write(dir.join("recipe.toml"), format!("{recipe}# the same columns\n")).unwrap();
    let said = stopped_after(&dir, 2, &[]);
    assert!(said.contains("Starting over: the press that stopped in "), "{said}");
    assert!(said.contains("was another press: recipe.toml changed."), "{said}");
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains(&format!("Picking up {name} where it stopped: 2 of 3 files done (journal from ")), "{said}");
    assert!(said.contains("  3/3 ") && !said.contains("  1/3 "), "{said}");
    assert!(!staged.exists(), "the folder goes once the file is in place");
    let picked = data_sha256(&dir);
    assert_eq!(rvr(&dir, &["validate", &name]).0, 0);
    std::fs::remove_file(dir.join(&name)).unwrap();
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    assert_eq!(picked, data_sha256(&dir), "picked up, the file is the one a press makes at once");

    // A target in a folder that is not there is refused, and the folder is not made.
    let (code, said) = rvr(&dir, &["press", "--out", "nowhere/x.rvr"]);
    assert!(code == 1 && said.contains("nowhere/x.rvr cannot be written: the folder nowhere is not there."), "{said}");
    assert!(!dir.join("nowhere").exists());

    // --restart starts over.
    std::fs::remove_file(dir.join(&name)).unwrap();
    stopped_after(&dir, 1, &[]);
    let (code, said) = rvr(&dir, &["press", "--restart"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("Starting over: removed the press that stopped in ") && !said.contains("Picking up"), "{said}");
    assert!(said.contains("  1/3 "), "{said}");

    // A fill stopped part way is picked up too: two files gone are two holes, read again.
    std::fs::remove_file(dir.join(&name)).unwrap();
    for gone in ["flat_b.root", "flat_c.root"] {
        std::fs::rename(dir.join(gone), dir.join(format!("{gone}.away"))).unwrap();
    }
    // A file gone is a hole that lasts: a press picked up keeps it (one holed for a passing
    // reason, such as a storage down, is read again).
    stopped_after(&dir, 2, &[]);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("where it stopped: 2 of 3 files done") && said.contains("  3/3 "), "{said}");
    for gone in ["flat_b.root", "flat_c.root"] {
        std::fs::rename(dir.join(format!("{gone}.away")), dir.join(gone)).unwrap();
    }
    stopped_after(&dir, 1, &["--fill"]);
    let (code, said) = rvr(&dir, &["press", "--fill"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("where it stopped: 1 of 2 files done"), "{said}");
    assert_eq!(picked, data_sha256(&dir), "filled, the file is the one a press makes at once");
    assert!(!staged.exists());
}

#[test]
fn a_press_stopped_by_what_is_not_a_files_fault_keeps_what_it_read() {
    let samples = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nlabel = \"Flat\"\nfiles = [\"a/flat.root\", \"b/flat.root\"]\ntree = \"analysis\"\n";
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("unmounted", samples, recipe);
    for folder in ["a", "b"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(folder).join("flat.root")).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // b's folder gone, as a disk not mounted: not the file's fault, so no hole, and no file.
    std::fs::rename(dir.join("b"), dir.join("away")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("its folder is not there either (a disk not mounted?)"), "{said}");
    assert!(said.contains("then press again, which carries on from the 1 of 2 files done"), "{said}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    assert!(!dir.join(&name).exists());
    std::fs::rename(dir.join("away"), dir.join("b")).unwrap();
    // Every file read, the file cannot be put in place (a folder stands there): what was read is
    // kept, and the next press reads nothing again.
    std::fs::create_dir_all(dir.join(&name).join("in the way")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("where it stopped: 1 of 2 files done") && said.contains("  2/2 "), "{said}");
    assert!(said.contains("could not be written: "), "{said}");
    assert!(said.contains("Mend it, then press again, which carries on from the 2 of 2 files done"), "{said}");
    std::fs::remove_dir_all(dir.join(&name)).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("where it stopped: 2 of 2 files done") && !said.contains("/2 "), "{said}");
    assert!(rvr(&dir, &["validate", &name, "--complete"]).1.contains("read all 10 entries"));
}

#[test]
fn a_flag_some_samples_lack_is_compared_or_given_to_all_in_defaults() {
    let samples = |defaults: &str| format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n{defaults}\n[eras]\n\n[[sample]]\nname = \"data\"\nfiles = \"data.root\"\nmetadata = {{ isData = true }}\n\n[[sample]]\nname = \"mc\"\nfiles = \"mc.root\"\nmetadata = {{ xsec = 88.29 }}\n");
    let recipe = |condition: &str| format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\nw = \"if({condition}, 0 * weight + 1, weight * sample.xsec)\"\n\n[totals]\n\n[derived]\n");
    let dir = study("flag", &samples(""), &recipe("sample.isData"));
    for name in ["data.root", "mc.root"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(name)).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // A flag on the data sample alone is missing for mc: a condition it cannot be.
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("(sample mc, ") && said.contains(": [columns] w: sample.isData is used as the condition of if, but this sample has no isData: give it one in samples.toml, or give every sample one under [defaults]"), "{said}");
    // Compared, a sample without it is not data; it is in the metadata, but no sample column.
    std::fs::write(dir.join("recipe.toml"), recipe("sample.isData == true")).unwrap();
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let dump = columns(&dir);
    assert!(dump.contains("xsec           f8 by sample[2]  data: null, mc: 88.29") && !dump.contains("isData"), "{dump}");
    // Given to every sample in [defaults], it is a sample column, and a condition for every sample.
    std::fs::write(dir.join("samples.toml"), samples("metadata = { isData = false }\n")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe("sample.isData")).unwrap();
    let header = pressed(&dir, &[]);
    assert_eq!(header["made"]["files"]["mc"]["metadata"]["isData"], false);
    let dump = columns(&dir);
    assert!(dump.contains("isData         b1 by sample[2]  data: true, mc: false"), "{dump}");
}

#[test]
fn a_mistake_in_the_recipe_stops_the_press_with_no_file_and_no_holes() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nht = \"sum(Jet_ptt)\"\n\n[totals]\n\n[derived]\n";
    let dir = twice("mistake", recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named Jet_ptt. Did you mean: Jet_pt"), "{said}");
    assert!(!said.contains("hole"), "{said}");
    assert!(!dir.join(format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())).exists());
    // A column naming a branch the file does not have says which column, and which branch.
    let renamed = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nnpv = \"PV_npvs\"\n\n[totals]\n\n[derived]\n";
    std::fs::write(dir.join("recipe.toml"), renamed).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("[columns] npv: There is no column named PV_npvs."), "{said}");
    // A defined name that is also a branch of a sample's list is refused before anything is read,
    // used or not: a name means one thing.
    let both = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\nweight = \"nJet > 1\"\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\nheavy = \"weight\"\n\n[totals]\n\n[derived]\n";
    for recipe in [both.to_owned(), both.replace("heavy = \"weight\"\n", "")] {
        std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
        let (code, said) = rvr(&dir, &["press"]);
        assert_eq!(code, 1, "{said}");
        assert!(said.contains("[define] weight is also the name of a branch of sample flat in samples.lock, and a name means one thing: rename the defined name, as weight_sel, where the recipe uses it"), "{said}");
        assert!(!said.contains("hole") && !said.contains("Pressing"), "{said}");
    }
    // Every such name at once, on one line.
    std::fs::write(dir.join("recipe.toml"), both.replace("[define]\n", "[define]\nrun = \"1\"\n")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("[define] run is also the name of a branch of sample flat in samples.lock, and a name means one thing: rename the defined name, as run_sel, where the recipe uses it\n[define] weight is also"), "{said}");
    assert!(!said.contains("[columns] heavy"), "the clash is said once: {said}");
    let both = both.replace("weight", "heavy_event");
    std::fs::write(dir.join("recipe.toml"), &both).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    // A recipe.toml writes its five tables, even empty; one left out is refused by press and try.
    std::fs::write(dir.join("recipe.toml"), both.replace("\n[keep]\n", "")).unwrap();
    for args in [&["press"][..], &["try", "nJet"]] {
        let (code, said) = rvr(&dir, args);
        assert!(code == 1 && said.contains("recipe.toml has no [keep] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty. Add the line [keep]."), "{said}");
    }
    std::fs::write(dir.join("recipe.toml"), "version = 1\nformat = \"rvr-recipe\"\n\n[columns]\nbranches = [\"nJet\"]\n").unwrap();
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 1 && said.contains("recipe.toml has no [define], [keep], [totals] or [derived] table") && said.contains("Add [define], [keep], [totals] and [derived], each on a line of its own."), "{said}");
}

#[test]
fn a_press_whose_every_entry_is_in_a_hole_writes_the_file_and_its_holes() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("damaged-only", FLAT, recipe);
    damage(&dir.join("uproot_flat.root"), "weight");
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("Read 0 of 5 entries") && said.contains("(0 rows, 1 column, ") && said.contains("with 1 hole:"), "{said}");
    assert!(said.contains("in basket 0 of branch weight (entries 0 to "), "{said}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(&dir, &["validate", &name]);
    assert!(code == 0 && said.contains("0 rows, 1 columns, read 0 of 5 entries, 1 hole (damaged)"), "{said}");
}

#[test]
fn try_shows_what_an_expression_gives_on_the_first_events() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[define]
hard_jet = "Jet_pt > 30"

[keep]
jets = "nJet >= 1"

[columns]
branches = ["nJet"]

[totals]

[derived]
"#;
    let dir = study("try", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // nJet is 3, 4, 3, 0, 1.
    let (code, said) = rvr(&dir, &["try", "nJet", "--events", "4"]);
    assert_eq!(code, 0, "{said}");
    let values: Vec<&str> = said.lines().skip(1).map(|l| l.split_whitespace().last().unwrap()).collect();
    assert_eq!(values, ["3", "4", "3", "0"], "{said}");
    assert!(said.starts_with("flat: uproot_flat.root, tree analysis ("), "{said}");
    // The recipe's defined names can be used, and --kept passes over the event the cut drops.
    let (code, said) = rvr(&dir, &["try", "Jet_pt[hard_jet]", "--kept"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("  entry        0   [65.3125]\n"), "{said}");
    assert!(!said.contains("entry        3"), "{said}");
    assert!(said.contains("4 shown of its 5 entries"), "{said}");
    let (code, said) = rvr(&dir, &["try", "nJett"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named nJett. Did you mean: nJet"), "{said}");
    let (code, said) = rvr(&dir, &["try", "count(Jet_pt"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("the expression: Expected )"), "{said}");
    // A dotted name says how to name a branch whose name holds a dot.
    let (code, said) = rvr(&dir, &["try", "Jet.pt"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("Only sample has a field after a dot, as in sample.era"), "{said}");
    let (code, said) = rvr(&dir, &["try", "hard_jet()"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("hard_jet is a defined name, not a function: write it without brackets"), "{said}");
    // A flag it does not know, or one without its value, is refused; a word that is also a
    // flag's value is still a word.
    let (code, said) = rvr(&dir, &["try", "nJet", "--keep"]);
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("--keep is not one of its flags"), "{said}");
    assert_eq!(rvr(&dir, &["try", "nJet", "--events"]).0, 2);
    let (code, said) = rvr(&dir, &["try", "flat", "--sample", "flat", "--events", "1"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named flat"), "{said}");
}

#[test]
fn branches_lists_every_branch_with_its_kind_and_shape_from_the_lock() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("branches", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["branches"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.starts_with("flat: uproot_flat.root, tree analysis, 17 branches\n"), "{said}");
    // In alphabetical order whatever the case, each with the kind rvr reads it as and whether it is a list.
    let jet = "\n  isGood           b1  flat\n  Jet_eta          f4  jagged\n  Jet_pt           f4  jagged\n";
    assert!(said.contains(jet), "{said}");
    assert!(said.contains("\n  Muon_tight       b1  jagged\n  nJet             i4  flat\n"), "{said}");
    assert!(said.contains("\n  pv               f4  3 per event\n"), "{said}");
    // It reads the lock alone: with the file gone, it lists the same.
    std::fs::rename(dir.join("uproot_flat.root"), dir.join("aside.root")).unwrap();
    assert_eq!(rvr(&dir, &["branches"]).1, said);
    std::fs::rename(dir.join("aside.root"), dir.join("uproot_flat.root")).unwrap();
    let (code, said) = rvr(&dir, &["branches", "--find", "JET"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("3 of its 17 branches hold \"jet\""), "{said}");
    assert!(!said.contains("Muon"), "{said}");
    // A file with branches rvr cannot read lists them apart, with why.
    let samples = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"other\"\nfiles = \"stl_vectors.root\"\ntree = \"ntuple/Events\"\n";
    std::fs::write(dir.join("samples.toml"), samples).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["branches"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("\nNot readable by rvr:\n"), "{said}");
    assert!(said.contains("\n  label  (string: strings are not read)\n"), "{said}");
    std::fs::write(dir.join("samples.toml"), FLAT).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // A misspelt branch suggests the nearest, and says where to look.
    let (code, said) = rvr(&dir, &["try", "count(Jets_pt > 30)"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named Jets_pt. Did you mean: Jet_pt"), "{said}");
    assert!(said.contains("rvr branches lists every branch the file has."), "{said}");
    // rvr branches needs only the lock: a study with no recipe yet can look.
    std::fs::remove_file(dir.join("recipe.toml")).unwrap();
    let (code, said) = rvr(&dir, &["branches", "--find", "jet"]);
    assert!(code == 0 && said.contains("Jet_pt"), "{said}");
}

#[test]
fn the_lock_writes_each_number_as_a_number() {
    let samples = FLAT.replace("metadata = { kind = \"test\" }\n", "events = 4\nmetadata = { kind = \"test\", xsec = 2.5, era = 3, scale = 3.0 }\n");
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("numbers", &samples, recipe);
    let header = pressed(&dir, &[]);
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    for line in ["events = 4\n", "metadata = { kind = \"test\", xsec = 2.5, era = 3, scale = 3.0 }\n"] {
        assert!(lock.contains(line) && !lock.contains("serde_json"), "{line}{lock}");
    }
    let metadata = &header["made"]["files"]["flat"]["metadata"];
    let given = ["xsec", "era", "scale"].map(|key| metadata[key].to_string());
    assert_eq!((given, &header["data"]["rows"]), (["2.5", "3", "3.0"].map(String::from), &4.into()));
    // A value no lock can hold is refused in samples.toml, before anything is asked.
    std::fs::write(dir.join("samples.toml"), samples.replace("xsec = 2.5", "xsec = nan")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code != 0 && said.contains("xsec: NaN is not a finite number"), "{said}");
}

#[test]
fn an_emptied_file_keeps_its_header_and_is_refilled_byte_for_byte() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\njets = \"nJet >= 1\"\n\n[columns]\nbranches = [\"weight\", \"nJet\", \"Jet_pt\"]\nht = \"sum(Jet_pt)\"\n\n[totals]\n\n[derived]\nper_jet = \"ht / nJet\"\n";
    let dir = study("emptied", FLAT, recipe);
    assert_eq!(rvr(&dir, &["key", "new", "Tester"]).0, 0);
    pressed(&dir, &[]);
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let path = dir.join(&name);
    let whole = std::fs::read(&path).unwrap();
    // Not at a terminal, it is not emptied without --force (or -f): no one can be asked.
    let (code, said) = rvr(&dir, &["empty", &name]);
    assert!(code == 1 && said.contains("nothing was emptied: rvr empty asks before it lets a file's columns go, and no one is at a terminal to answer. Give --force (or -f) to empty without asking"), "{said}");
    let (code, said) = rvr(&dir, &["empty", &name, "-y"]);
    assert!(code == 2 && said.contains("-y is not one of its flags"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), whole);
    let (code, said) = rvr(&dir, &["empty", &name, "-f"]);
    assert!(code == 0 && said.contains(&format!("Emptied {name}")) && said.contains("keeps its study, every checksum and its signature.") && said.contains("rvr refill"), "{said}");
    let shell = std::fs::read(&path).unwrap();
    let end = 12 + u32::from_le_bytes(whole[8..12].try_into().unwrap()) as usize;
    assert_eq!(shell, whole[..end], "the header, byte for byte, and nothing after it");
    // Every command reads it as the empty file it is.
    let (code, said) = rvr(&dir, &["validate", &name]);
    assert!(code == 0 && said.contains("1 derived (worked out once refilled), read all 5 entries, empty (rvr refill presses its columns again), signed by you"), "{said}");
    let (code, said) = rvr(&dir, &["validate", &name, "--complete"]);
    assert!(code == 1 && said.contains("it is empty, and --complete asks for its columns"), "{said}");
    assert!(rvr(&dir, &["dump", &name, "--columns"]).1.contains("No columns: the file is empty"));
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 1 && said.contains("as it is empty (rvr refill gives its columns back"), "{said}");
    assert!(rvr(&dir, &["empty", &name, "--force"]).1.contains("is empty already"));
    let (code, said) = rvr(&dir, &["refill", &name]);
    assert!(code == 0 && said.contains("every column is back, byte for byte as it was, and its signature, by your own key ") && said.contains(", is as it was") && !said.contains("Signed with your key"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), whole);
    // An input that is gone: not emptied, unless with --anyway; then not refilled until it is back.
    std::fs::rename(dir.join("uproot_flat.root"), dir.join("away.root")).unwrap();
    let (code, said) = rvr(&dir, &["empty", &name, "--force"]);
    assert!(code == 1 && said.contains("was not emptied") && said.contains("uproot_flat.root no longer exists"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), whole);
    assert_eq!(rvr(&dir, &["empty", &name, "--anyway", "--force"]).0, 0);
    let (code, said) = rvr(&dir, &["refill", &name]);
    assert!(code == 1 && said.contains(&format!("{name} is still empty")), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), shell);
    std::fs::rename(dir.join("away.root"), dir.join("uproot_flat.root")).unwrap();
    assert_eq!(rvr(&dir, &["refill", &name]).0, 0);
    assert_eq!(std::fs::read(&path).unwrap(), whole);
    // A press of the study does not build on an empty file: it presses every column.
    assert_eq!(rvr(&dir, &["empty", &name, "-f"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("cannot be built on, as it is empty"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap().len(), whole.len());
    let left: Vec<String> = std::fs::read_dir(&*dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.contains("refill") || n.contains(".partial")).collect();
    assert!(left.is_empty(), "{left:?}");
    // A file with holes is not emptied: a refill could not read what its press read.
    let dir = twice("empty-holes", recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::remove_file(dir.join("flat_b.root")).unwrap();
    assert_eq!(rvr(&dir, &["press"]).0, 3);
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(&dir, &["empty", &name, "--force"]);
    assert!(code == 1 && said.contains("has 1 hole: a refill reads what its press read"), "{said}");
}

#[test]
fn totals_sum_every_event_read_before_the_cuts_and_a_derived_column_uses_them() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\njets = \"nJet >= 1\"\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\nsumw = \"weight\"\nn = \"1\"\nx = { value = \"x\", tree = \"other\" }\n\n[derived]\nw = \"weight / sample.sumw\"\n";
    let dir = twice("totals", recipe);
    let header = pressed(&dir, &[]);
    let flat = &header["made"]["files"]["flat"];
    // Each copy: weights 1, 0.5, 2.25, 1, 0.75 of five events, the cut keeping four; x is 0, 1, 2 in its other tree.
    for file in ["./flat_a.root", "./flat_b.root"] {
        let totals = &flat["files"][file]["totals"];
        assert_eq!((totals["sumw"].as_f64(), totals["n"].as_f64(), totals["x"].as_f64()), (Some(5.5), Some(5.0), Some(3.0)), "{file}");
    }
    let given = |key: &str| flat["metadata"][key].as_f64();
    assert_eq!((given("sumw"), given("n"), given("x"), &header["data"]["rows"]), (Some(11.0), Some(10.0), Some(6.0), &8.into()));
    // Each is a sample column, and the derived column works out with it.
    let shown = columns(&dir);
    assert!(shown.contains("sumw") && shown.contains("derived = \"weight / sample.sumw\""), "{shown}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let (code, said) = rvr(&dir, &["validate", &name]);
    assert!(code == 0 && said.contains("1 derived"), "{said}");
    // A column added is read for the events kept, and the totals are the file's.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("[\"weight\"]", "[\"weight\", \"nJet\"]")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Reading only nJet"), "{said}");
    let header: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    assert_eq!(header["made"]["files"]["flat"]["metadata"]["sumw"].as_f64(), Some(11.0));
    // Totals changed: rvr status says so, and every event is read again.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("n = \"1\"\n", "")).unwrap();
    assert!(rvr(&dir, &["status"]).1.contains("the totals under [totals] changed"));
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("the totals under [totals] have changed"), "{said}");
    // The first events only: the totals over them; a total of another tree counts a whole file, and is refused.
    let first = recipe.replace("x = { value = \"x\", tree = \"other\" }\n", "");
    std::fs::write(dir.join("recipe.toml"), &first).unwrap();
    let header = pressed(&dir, &["--events", "2", "--full"]);
    assert_eq!(header["made"]["files"]["flat"]["metadata"]["sumw"].as_f64(), Some(1.5));
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
    let (code, said) = rvr(&dir, &["press", "--events", "2"]);
    assert!(code == 1 && said.contains("[totals] x sums the tree other, whose entries count every event of a file, and flat reads only some of its events"), "{said}");
    // The events' own tree is not another tree.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("tree = \"other\"", "tree = \"analysis\"")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("[totals] x names the tree analysis, the one flat reads its events from"), "{said}");
}

#[test]
fn totals_hold_when_a_sample_is_added_and_name_their_mistakes() {
    let one = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n[[sample]]\nname = \"a\"\nfiles = \"flat_a.root\"\nmetadata = { norm = 2 }\n";
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\nw = \"weight\"\n\n[keep]\njets = \"nJet >= 1\"\n\n[columns]\nbranches = [\"weight\"]\n\n[totals]\nsumw = \"w\"\n\n[derived]\n";
    let dir = twice("totals-added", recipe);
    std::fs::write(dir.join("samples.toml"), one).unwrap();
    pressed(&dir, &[]);
    // A sample added is read alone, and each sample's total is its own events' sum, once.
    std::fs::write(dir.join("samples.toml"), format!("{one}\n[[sample]]\nname = \"b\"\nfiles = \"flat_b.root\"\n")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("the new sample b"), "{said}");
    let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let header: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    let sumw = |sample: &str| header["made"]["files"][sample]["metadata"]["sumw"].as_f64();
    assert_eq!((sumw("a"), sumw("b")), (Some(5.5), Some(5.5)));
    // A defined name a total reaches, changed: every event is read again.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("w = \"weight\"", "w = \"weight * 2\"")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("the totals under [totals] have changed"), "{said}");
    let header: Value = serde_json::from_str(&rvr(&dir, &["header", &name]).1).unwrap();
    assert_eq!(header["made"]["files"]["b"]["metadata"]["sumw"].as_f64(), Some(11.0));
    // A sum that is not a finite number, and a total named like a key of samples.toml, are refused by name.
    std::fs::write(dir.join("recipe.toml"), recipe.replace("sumw = \"w\"", "sumw = \"w / (nJet - nJet)\"")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains("[totals] sumw: its sum is not a finite number"), "{said}");
    std::fs::write(dir.join("recipe.toml"), recipe.replace("sumw = \"w\"", "norm = \"w\"")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains("[totals] norm has the name of a key of samples.toml"), "{said}");
}

#[test]
fn resolve_reads_samples_toml_on_rails_and_pins_files_wherever_it_runs() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("rails", FLAT, recipe);
    // Resolved from the folder above, the lock's paths are still the study's own.
    let name = dir.file_name().unwrap().to_str().unwrap().to_owned();
    let (code, said) = rvr(dir.parent().unwrap(), &["samples", "resolve", &format!("{name}/samples.toml")]);
    assert_eq!(code, 0, "{said}");
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    assert!(lock.contains("at = \"./uproot_flat.root\"\n"), "{lock}");
    assert!(lock.contains("[[branches]]\n") && lock.contains("{ name = \"Jet_pt\", kind = \"f4\", shape = \"jagged\" },"), "{lock}");
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    // A link to a file is the file; a file reached twice is refused.
    std::os::unix::fs::symlink(dir.join("uproot_flat.root"), dir.join("link.root")).unwrap();
    let samples = |files: &str| FLAT.replace("files = \"uproot_flat.root\"", files);
    std::fs::write(dir.join("samples.toml"), samples("files = \"link.root\"")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    std::fs::write(dir.join("samples.toml"), samples("files = [\"uproot_flat.root\", \"uproot_*.root\"]")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("./uproot_flat.root is reached twice"), "{said}");
    std::fs::write(dir.join("samples.toml"), samples("files = [\"link.root\", \"uproot_flat.root\"]")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("./uproot_flat.root is reached twice"), "{said}");
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("sub/uproot_flat.root")).unwrap();
    std::fs::write(dir.join("samples.toml"), samples("files = [\"sub/uproot_flat.root\", \"./sub/../sub/uproot_flat.root\"]")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("./sub/../sub/uproot_flat.root is reached twice"), "{said}");
    // One folder written two ways is one folder: every file in it is read.
    std::fs::rename(dir.join("sub/uproot_flat.root"), dir.join("sub/a.root")).unwrap();
    std::fs::copy(dir.join("sub/a.root"), dir.join("sub/b.root")).unwrap();
    let two = samples("files = \"sub/a.root\"") + "\n[[sample]]\nname = \"b\"\nfiles = \"./sub/b.root\"\ntree = \"analysis\"\n";
    std::fs::write(dir.join("samples.toml"), two).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 0 && !said.contains("no sample reads"), "{said}");
    // On rails: a table left out is refused (off a terminal, nothing is offered), and a key no
    // sample takes says where metadata goes.
    std::fs::write(dir.join("samples.toml"), FLAT.replace("[eras]\n", "")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("it leaves out [eras]"), "{said}");
    std::fs::write(dir.join("samples.toml"), FLAT.replace("metadata = { kind = \"test\" }", "kind = \"test\"")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("has no key \"kind\"") && said.contains("metadata = { kind = … }"), "{said}");
}

#[test]
fn a_file_under_the_home_folder_is_pinned_and_read_as_samples_toml_writes_it() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("home", &FLAT.replace("files = \"uproot_flat.root\"", "files = \"~/data/uproot_flat.root\""), recipe);
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("data")).unwrap();
    std::fs::copy(dir.join("uproot_flat.root"), home.join("data/uproot_flat.root")).unwrap();
    let (code, said) = rvr_at_home(&dir, &home, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    assert!(lock.contains("at = \"~/data/uproot_flat.root\"\n"), "{lock}");
    let (code, said) = rvr_at_home(&dir, &home, &["press"]);
    assert_eq!(code, 0, "{said}");
}

#[test]
fn a_value_the_recipe_cannot_take_stops_the_press_naming_its_file_entry_column_and_value() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\nn = { value = \"nJet * 100\", stored = \"u1\" }\n\n[totals]\n\n[derived]\n";
    let dir = study("unfit", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("uproot_flat.root, tree analysis, entry 0: [columns] n: 300 does not fit in u1, which holds 0 to 255"), "{said}");
    assert!(!said.contains("hole"), "{said}");
    // A total whose sum stops being finite names the entry whose value made it so.
    let total = recipe.replace("n = { value = \"nJet * 100\", stored = \"u1\" }\n", "").replace("[totals]\n", "[totals]\ninverse = \"1 / (nJet - 3)\"\n");
    std::fs::write(dir.join("recipe.toml"), total).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("uproot_flat.root, tree analysis, entry 0: [totals] inverse: its sum is not a finite number once this entry's value, inf, is added"), "{said}");
    // A total over another tree of the file is read through runs the same way, and names that tree's entry.
    let other = recipe.replace("n = { value = \"nJet * 100\", stored = \"u1\" }\n", "").replace("[totals]\n", "[totals]\nsum_x = { value = \"x\", tree = \"other\" }\ninverse = { value = \"1 / (x - 1)\", tree = \"other\" }\n");
    std::fs::write(dir.join("recipe.toml"), &other).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("uproot_flat.root: tree \"other\", entry 1: [totals] inverse: its sum is not a finite number once this entry's value, inf, is added"), "{said}");
    std::fs::write(dir.join("recipe.toml"), other.replace("inverse = { value = \"1 / (x - 1)\", tree = \"other\" }\n", "")).unwrap();
    let made = pressed(&dir, &[]);
    assert_eq!(made["made"]["files"]["flat"]["metadata"]["sum_x"], 3.0, "{made}");
}

#[test]
fn derive_says_every_problem_of_the_recipe_a_file_carries_in_one_sentence() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";
    let dir = study("carried", FLAT, recipe);
    let mut header = pressed(&dir, &[]);
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    // Its recipe written by hand with two problems this rvr refuses.
    let wrong = recipe.replace("[keep]\n", "[keep]\na = \"cnt(nJet)\"\nb = \"count(nJet, 1)\"\n");
    header["made"]["recipe"]["text"] = wrong.as_str().into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(&wrong).into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    rowver::write::rewrite(dir.join(&file), dir.join("carried.rvr"), &header).unwrap();
    let (code, said) = rvr(&dir, &["derive", "carried.rvr", "twice = nJet * 2"]);
    assert_eq!(code, 1, "{said}");
    let line = said.lines().find(|line| line.contains("cannot be read by this rvr")).unwrap_or_else(|| panic!("{said}"));
    assert!(line.contains("([keep] a: There is no function named cnt") && line.contains("; [keep] b: count takes 1 value, not 2)"), "{said}");
}
