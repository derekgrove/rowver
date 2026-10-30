//! The example studies in docs/examples: every samples.toml expands and every recipe.toml reads,
//! as `rvr press` reads them before it opens a file. Pressing them needs the network and a grid
//! proxy: `cargo test -p rvr --test examples -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every example study, by folder; none when the docs are not there (the crate as published).
fn studies() -> Vec<PathBuf> {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
    let Ok(entries) = std::fs::read_dir(&examples) else {
        return Vec::new();
    };
    let mut studies: Vec<_> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("samples.toml").is_file())
        .collect();
    studies.sort();
    assert!(!studies.is_empty(), "no study in {}", examples.display());
    studies
}

/// A folder removed when it goes out of scope, even when a check in it failed.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run rvr in `dir`, and fail naming the study when it fails.
fn rvr(dir: &Path, study: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .current_dir(dir)
        .env("RVR_CONFIG", dir.join(".rvr-config"))
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{}: rvr {}: {said}",
        study.display(),
        args.join(" ")
    );
}

#[test]
fn every_example_study_reads() {
    for study in studies() {
        let name = study.file_name().unwrap().to_str().unwrap();
        let scratch = Scratch(
            std::env::temp_dir().join(format!("rvr-example-read-{}-{name}", std::process::id())),
        );
        std::fs::create_dir_all(&scratch.0).unwrap();
        std::fs::copy(study.join("samples.toml"), scratch.0.join("samples.toml")).unwrap();
        rvr(&scratch.0, &study, &["samples", "expand", "samples.toml"]);
        let recipe = std::fs::read_to_string(study.join("recipe.toml")).unwrap();
        if let Err(error) = rvr::read_recipe(&recipe) {
            panic!("{}: {error}", study.display());
        }
    }
}

#[test]
#[ignore = "reads Open Data and DAS datasets: needs the network and a grid proxy"]
fn every_example_study_presses() {
    for study in studies() {
        let name = study.file_name().unwrap().to_str().unwrap();
        let scratch = Scratch(
            std::env::temp_dir().join(format!("rvr-example-{}-{name}", std::process::id())),
        );
        let dir = &scratch.0;
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        for file in ["samples.toml", "recipe.toml"] {
            std::fs::copy(study.join(file), dir.join(file)).unwrap();
        }
        rvr(dir, &study, &["samples", "resolve", "samples.toml"]);
        rvr(dir, &study, &["press", "--events", "2000"]);
        rvr(dir, &study, &["validate", &format!("{name}.rvr")]);
    }
}
