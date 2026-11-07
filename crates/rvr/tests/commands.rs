//! The commands against SPEC §1, §5, §6, §8 to §12, where no press test shows them: the parts,
//! the questions asked at a terminal (through `script`, which gives rvr one), the folder keys live
//! in, the plan of a press, and the release.

mod common;

use common::{at_terminal, rvr, study, Study};
use std::path::Path;
use std::process::Command;

const SAMPLES: &str = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nfiles = \"uproot_flat.root\"\ntree = \"analysis\"\nmetadata = { kind = \"test\" }\n";
const RECIPE: &str = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

// SPEC 1.4, 5.1.7, 4.1.6
#[test]
fn a_missing_table_is_offered_at_a_terminal_and_written_in_on_yes_in_any_case() {
    let missing = RECIPE.replace("[totals]\n\n", "");
    let dir = study("offer", SAMPLES, RECIPE);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::write(dir.join("recipe.toml"), &missing).unwrap();
    // Away from a terminal nothing is asked.
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && !said.contains("[y/N]"), "{said}");
    let (code, said) = at_terminal(&dir, &["press"], "nope");
    assert!(code == 1 && said.contains("Add [totals] to recipe.toml, empty and in its place, then continue pressing? [y/N] "), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("recipe.toml")).unwrap(), missing);
    let (code, said) = at_terminal(&dir, &["press"], "YeS");
    assert_eq!(code, 0, "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("recipe.toml")).unwrap(), RECIPE);
    // A recipe whose [columns] stores nothing is not the offer's to mend.
    std::fs::write(dir.join("recipe.toml"), RECIPE.replace("branches = [\"nJet\"]\n", "")).unwrap();
    let (code, said) = at_terminal(&dir, &["press"], "y");
    assert!(code == 1 && !said.contains("[y/N]"), "{said}");
}

// SPEC 3.1.4, 5.1.7
#[test]
fn resolve_offers_the_missing_tables_of_samples_toml_at_a_terminal() {
    let missing = SAMPLES.replace("[eras]\n\n", "");
    let dir = study("offer-samples", &missing, RECIPE);
    let (code, said) = at_terminal(&dir, &["samples", "resolve", "samples.toml"], "y");
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("[eras]") && said.contains("[y/N] "), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("samples.toml")).unwrap(), SAMPLES);
}

// SPEC 8.9, 5.1.7
#[test]
fn empty_asks_at_a_terminal_naming_the_files_rows_columns_and_size() {
    let dir = study("empty-asks", SAMPLES, RECIPE);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    let whole = std::fs::read(dir.join(&file)).unwrap();
    let (code, said) = at_terminal(&dir, &["empty", &file], "n");
    assert!(code == 1 && said.contains("(5 rows, 1 column, ") && said.contains(" kB)? "), "{said}");
    assert_eq!(std::fs::read(dir.join(&file)).unwrap(), whole);
    let (code, said) = at_terminal(&dir, &["empty", &file], "y");
    assert_eq!(code, 0, "{said}");
    assert!(std::fs::read(dir.join(&file)).unwrap().len() < whole.len());
}

// SPEC 10.1
#[test]
fn keys_live_in_rvr_config_else_xdg_config_home_else_dot_config() {
    let dir = Study::new("config");
    let made = |set: &[(&str, &Path)]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rvr"));
        command.args(["key", "new"]).current_dir(&*dir).env_remove("RVR_CONFIG").env_remove("XDG_CONFIG_HOME").env("HOME", dir.join("home"));
        for (name, value) in set {
            command.env(name, value);
        }
        command.output().unwrap()
    };
    let run = |set: &[(&str, &Path)]| assert!(made(set).status.success());
    run(&[("XDG_CONFIG_HOME", &dir.join("xdg"))]);
    assert!(dir.join("xdg/rvr/key").exists());
    run(&[("RVR_CONFIG", &dir.join("own")), ("XDG_CONFIG_HOME", &dir.join("xdg2"))]);
    assert!(dir.join("own/key").exists() && !dir.join("xdg2").exists());
    // An XDG_CONFIG_HOME that is not a full path is passed over.
    run(&[("XDG_CONFIG_HOME", Path::new("xdg3"))]);
    assert!(dir.join("home/.config/rvr/key").exists() && !dir.join("xdg3").exists());
    // A HOME that is not a full path is refused, never read from where rvr runs.
    let out = made(&[("HOME", Path::new("home"))]);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && said.contains("HOME is not set to a full path, so rvr cannot find ~/.config/rvr"), "{said}");
}

// SPEC 6.4.1, 2.8
#[test]
fn samples_plan_says_what_a_press_would_read_from_the_lock_alone() {
    let dir = study("plan", SAMPLES, RECIPE);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // Nothing of any file is read: they may be gone.
    for file in ["uproot_flat.root", "stl_vectors.root", "codecs-zlib.root"] {
        std::fs::remove_file(dir.join(file)).unwrap();
    }
    let (code, said) = rvr(&dir, &["samples", "plan", "samples.lock", "--events", "3"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("3 of 5 events   1 of 1 files; stops at entry 3 of uproot_flat.root"), "{said}");
    assert!(said.contains("3 of 5 events in all."), "{said}");
}

// SPEC 6.1.1
#[test]
fn expand_lists_the_samples_and_with_json_their_metadata_asking_no_storage() {
    let samples = SAMPLES.replace("uproot_flat.root", "https://nowhere.invalid/store/x.root");
    let dir = study("expand", &samples, RECIPE);
    let (code, said) = rvr(&dir, &["samples", "expand", "samples.toml"]);
    assert!(code == 0 && said.contains("flat") && said.contains("analysis") && said.contains("https://nowhere.invalid/store/x.root"), "{said}");
    let (code, said) = rvr(&dir, &["samples", "expand", "samples.toml", "--json"]);
    assert_eq!(code, 0, "{said}");
    let json: serde_json::Value = serde_json::from_str(&said).unwrap();
    assert_eq!(json[0]["metadata"], serde_json::json!({"kind": "test"}));
    assert_eq!((&json[0]["label"], &json[0]["events"]), (&serde_json::json!("flat"), &serde_json::json!({"choose": "all"})));
}

// SPEC 5.5.2, 5.5.1
#[test]
fn try_reads_the_study_as_a_press_does_and_a_total_stands_in_as_1() {
    let dir = study("try", SAMPLES, &RECIPE.replace("[totals]\n", "[totals]\nw = \"weight\"\n"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["try", "nJet * sample.w"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("sample.w is a total of [totals], known once every event is read: it stands in as 1 here."), "{said}");
    assert!(said.contains("  entry        1   4"), "{said}");
    // Through the agreement check: a branch no sample has is refused, naming the nearest.
    let (code, said) = rvr(&dir, &["try", "nJett"]);
    assert!(code == 1 && said.contains("nJet"), "{said}");
}

// SPEC 1.3; FORMAT 11
#[test]
fn no_program_that_reads_files_is_named_in_the_code() {
    let mut named = Vec::new();
    for crate_ in ["rowver", "rvr", "rvr-recipe", "rowver-py"] {
        let mut folders = vec![root().join("crates").join(crate_).join("src")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if let Ok(text) = std::fs::read_to_string(&path) {
                    if text.contains("rowver_web") || text.contains("viewer") {
                        named.push(path);
                    }
                }
            }
        }
    }
    assert!(named.is_empty(), "{named:?}");
}

// SPEC 1.6, 1.7, 11.1
#[test]
fn the_parts_are_the_four_the_spec_names_sharing_one_version() {
    let manifest = |crate_: &str| std::fs::read_to_string(root().join("crates").join(crate_).join("Cargo.toml")).unwrap();
    assert!(manifest("rvr").contains("rowver.workspace = true") && manifest("rvr").contains("rvr-recipe.workspace = true"));
    assert!(manifest("rowver").contains("rvr-recipe.workspace = true") && !manifest("rvr-recipe").contains("rowver"));
    for crate_ in ["rowver", "rvr", "rvr-recipe", "rowver-py"] {
        assert!(manifest(crate_).contains("version.workspace = true"), "{crate_}");
    }
    let workspace = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    assert!(workspace.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))) && env!("CARGO_PKG_VERSION").starts_with("15."));
    let pyproject = std::fs::read_to_string(root().join("crates/rowver-py/pyproject.toml")).unwrap();
    assert!(pyproject.contains("[project.scripts]") && pyproject.contains("rvr = "), "{pyproject}");
    assert!(pyproject.contains("requires-python = \">=3.11\"") && pyproject.contains("numpy"), "{pyproject}");
}

// SPEC 12.4
#[test]
fn the_release_workflow_tests_and_builds_everything_before_it_publishes() {
    let Ok(workflow) = std::fs::read_to_string(root().join(".github/workflows/release.yml")) else {
        return eprintln!("not checked here: no release workflow (a package, not the repository)");
    };
    for part in [
        "cargo test", "target: x86_64, manylinux: \"2_28\"", "target: aarch64, manylinux: \"2_28\"", "universal2-apple-darwin",
        "python: [\"3.11\", \"3.14\"]", "container: \"almalinux:8\"", "aarch64-apple-darwin x86_64-apple-darwin",
        "needs: [plan, test-wheels, sdist, binaries]", "needs: [plan, test-wheels, sdist, binaries, crates-io]", "needs: [plan, test-wheels, sdist, binaries, crates-io, pypi]",
    ] {
        assert!(workflow.contains(part), "the release workflow lacks {part:?}");
    }
}

// SPEC 9.2
#[test]
fn dump_all_prints_every_row_of_every_valid_file_of_the_corpus() {
    let corpus = root().join("tests/conformance/rvr");
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(corpus.join("manifest.json")).unwrap()).unwrap();
    let dir = Study::new("dump-corpus");
    for entry in manifest["valid"].as_array().unwrap() {
        let file = corpus.join(entry["file"].as_str().unwrap());
        let (code, said) = rvr(&dir, &["dump", file.to_str().unwrap(), "--all"]);
        assert_eq!(code, 0, "{file:?}: {said}");
        let contents = &entry["contents"];
        if contents["empty"] == true {
            assert!(said.contains("No columns: the file is empty"), "{said}");
            continue;
        }
        let columns = contents["columns"].as_object().unwrap().len();
        let lines: Vec<&str> = said.lines().skip_while(|l| !l.starts_with("sample\tentry")).collect();
        let rows = contents["rows"].as_u64().unwrap() as usize;
        assert!(lines.len() > rows, "{file:?}: {said}");
        for (row, line) in lines[1..=rows].iter().enumerate() {
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(fields.len(), 2 + columns, "{file:?} row {row}: {line}");
            assert_eq!(fields[1], contents["entry"][row].to_string(), "{file:?} row {row}");
        }
    }
}
