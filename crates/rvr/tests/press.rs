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
        .env("RVR_CONFIG", dir.join(".rvr-config"))
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
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_random_choice_is_recorded_checked_and_reproduced() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
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
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_is_signed_once_there_is_a_key() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
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
    assert!(said.contains("--random needs a number of events"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--seed", "3"]);
    assert_eq!(code, 1);
    assert!(said.contains("--seed is for a random choice"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--events", "all", "--random"]);
    assert_eq!(code, 1);
    assert!(said.contains("not --events all"), "{said}");

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

#[test]
fn a_column_added_to_the_recipe_is_read_alone_and_the_file_is_a_full_press() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[keep]
jets = "nJet >= 1"

[copy]
branches = ["weight", "nJet"]
"#;
    let dir = study("add", FLAT, recipe);
    let first = pressed(&dir, &[]);
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let added = recipe.replace(
        r#"branches = ["weight", "nJet"]"#,
        "branches = [\"weight\", \"nJet\", \"Jet_pt\"]\n\n[column]\nht = \"sum(Jet_pt)\"",
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

    // Nothing new: nothing is read. Another cut: every column is pressed again.
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("Nothing to read"), "{said}");
    // Even with nothing to read, every input is checked against the lock.
    let input = dir.join("uproot_flat.root");
    let bytes = std::fs::read(&input).unwrap();
    std::fs::write(&input, [&bytes[..], b"x"].concat()).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("changed since samples.lock was written"),
        "{said}"
    );
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
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn status_says_whether_the_file_is_current_and_what_press_would_read() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[keep]
jets = "nJet >= 1"

[copy]
branches = ["weight", "nJet"]
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
    assert!(
        said.contains("rvr press refuses until samples.lock is resolved again"),
        "{said}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// FLAT's file twice, as one sample: flat_a.root and flat_b.root.
const TWICE: &str = r#"format = "rvr-samples"
version = 1

[[sample]]
name = "flat"
label = "Flat ntuple"
files = "flat_*.root"
tree = "analysis"
"#;

/// A study of TWICE.
fn twice(name: &str, recipe: &str) -> PathBuf {
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
fn a_damaged_basket_is_a_hole_named_with_its_file_tree_and_entries() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
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
    let more = format!("{recipe}\n[column]\ndouble = \"weight * 2\"\n");
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
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_gone_or_changed_since_the_lock_is_a_hole_of_its_own() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
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
    let (code, said) = rvr(&dir, &["press", "--fill", "tape,unreachabel"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("not \"unreachabel\""), "{said}");
    let (code, said) = rvr(&dir, &["press", "--fill", "tape"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("has no holes of kind tape: its holes are changed, gone"), "{said}");
    // --fill gone reads only that kind: still gone, it fills nothing and leaves the file.
    let before = std::fs::read(dir.join(&name)).unwrap();
    let (code, said) = rvr(&dir, &["press", "--fill", "gone"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("reading 1 hole of 1 file"), "{said}");
    assert!(said.contains("None of the 1 hole could be read"), "{said}");
    assert_eq!(std::fs::read(dir.join(&name)).unwrap(), before);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fill_takes_the_choice_it_was_pressed_with_and_the_file_put_back() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
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
    std::fs::remove_dir_all(&dir).unwrap();
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
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[keep]\njets = \"nJet >= 1\"\n\n[copy]\nbranches = [\"weight\", \"Jet_pt\"]\n";
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
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_press_stopped_by_what_is_not_a_files_fault_keeps_what_it_read() {
    let samples = "format = \"rvr-samples\"\nversion = 1\n\n[[sample]]\nname = \"flat\"\nlabel = \"Flat\"\nfiles = [\"a/flat.root\", \"b/flat.root\"]\ntree = \"analysis\"\n";
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
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
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_mistake_in_the_recipe_stops_the_press_with_no_file_and_no_holes() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[column]\nht = \"sum(Jet_ptt)\"\n";
    let dir = twice("mistake", recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named Jet_ptt. Did you mean: Jet_pt"), "{said}");
    assert!(!said.contains("hole"), "{said}");
    assert!(!dir.join(format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_press_that_can_read_nothing_writes_nothing() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
    let dir = study("damaged-only", FLAT, recipe);
    damage(&dir.join("uproot_flat.root"), "weight");
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("every entry the press chose is in a hole"), "{said}");
    assert!(said.contains("in basket 0 of branch weight (entries 0 to "), "{said}");
    assert!(!dir.join(format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn try_shows_what_an_expression_gives_on_the_first_events() {
    let recipe = r#"format = "rvr-recipe"
version = 1

[object.hard_jet]
from = "Jet"
where = "pt > 30"

[keep]
jets = "nJet >= 1"

[copy]
branches = ["nJet"]
"#;
    let dir = study("try", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // nJet is 3, 4, 3, 0, 1.
    let (code, said) = rvr(&dir, &["try", "nJet", "--events", "4"]);
    assert_eq!(code, 0, "{said}");
    let values: Vec<&str> = said.lines().skip(1).map(|l| l.split_whitespace().last().unwrap()).collect();
    assert_eq!(values, ["3", "4", "3", "0"], "{said}");
    assert!(said.starts_with("flat: uproot_flat.root, tree analysis ("), "{said}");
    // The recipe's objects can be used, and --kept passes over the event the cut drops.
    let (code, said) = rvr(&dir, &["try", "hard_jet.pt", "--kept"]);
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
    // A flag it does not know, or one without its value, is refused; a word that is also a
    // flag's value is still a word.
    let (code, said) = rvr(&dir, &["try", "nJet", "--keep"]);
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("--keep is not one of its flags"), "{said}");
    assert_eq!(rvr(&dir, &["try", "nJet", "--events"]).0, 2);
    let (code, said) = rvr(&dir, &["try", "flat", "--sample", "flat", "--events", "1"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named flat"), "{said}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn branches_lists_the_collections_and_values_a_recipe_can_name() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"nJet\"]\n";
    let dir = study("branches", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["branches"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.starts_with("flat: uproot_flat.root, tree analysis, 17 branches\n"), "{said}");
    // Each group's counter comes first; every branch says its type and whether it is a list.
    let jet = "\n\nJet:\n  nJet     int32  flat\n  Jet_eta  float  jagged\n  Jet_pt   float  jagged\n";
    assert!(said.contains(jet), "{said}");
    assert!(said.contains("\n  Muon_tight   bool   jagged\n"), "{said}");
    assert!(said.contains("\n\nOther:\n  event            uint64  flat\n"), "{said}");
    assert!(said.contains("\n  pv               float   3 per event\n"), "{said}");
    let (code, said) = rvr(&dir, &["branches", "--find", "JET"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("3 of its 17 branches hold \"jet\""), "{said}");
    assert!(!said.contains("Muon"), "{said}");
    // A file whose counted branches have no prefix, and one with branches rvr cannot read.
    let others = [("codecs-zlib.root", "Events", "\n  pt   float   jagged\n"), ("stl_vectors.root", "ntuple/Events", "\n  label  (string: strings are not read)\n")];
    for (file, tree, line) in others {
        let samples = format!("[[sample]]\nname = \"other\"\nfiles = \"{file}\"\ntree = \"{tree}\"\n");
        std::fs::write(dir.join("samples.toml"), samples).unwrap();
        assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
        let (code, said) = rvr(&dir, &["branches"]);
        assert_eq!(code, 0, "{said}");
        assert!(said.contains(line), "{said}");
    }
    std::fs::write(dir.join("samples.toml"), FLAT).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // A wrong collection name suggests the collection, and says where to look.
    let (code, said) = rvr(&dir, &["try", "count(Jets where pt > 30)"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("There is no column named Jets. Did you mean: Jet,"), "{said}");
    assert!(said.contains("rvr branches lists every branch and collection the file has."), "{said}");
    std::fs::remove_dir_all(&dir).unwrap();
}
