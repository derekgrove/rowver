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

#[test]
fn a_damaged_basket_is_named_with_its_file_tree_and_entries() {
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[copy]\nbranches = [\"weight\"]\n";
    let dir = study("damaged", FLAT, recipe);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let path = dir.join("uproot_flat.root");
    let (seek, nbytes) = {
        let file = hep_root::RootFile::open(hep_root::LocalFile::open(&path).unwrap()).unwrap();
        let tree = file.tree_at("analysis").unwrap();
        let branch = tree.branch("weight").unwrap();
        (
            branch.basket_seek[0] as usize,
            branch.basket_bytes[0] as usize,
        )
    };
    // Damaged after the lock was written: press checks size and adler32 first, so write a lock
    // that matches the damaged file.
    let mut bytes = std::fs::read(&path).unwrap();
    for b in &mut bytes[seek + nbytes - 12..seek + nbytes] {
        *b ^= 0x5a;
    }
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("uproot_flat.root, tree analysis, entries 0 to "),
        "{said}"
    );
    assert!(
        said.contains("in basket 0 of branch weight (entries 0 to "),
        "{said}"
    );
    assert!(said.contains("The file is damaged there"), "{said}");
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
