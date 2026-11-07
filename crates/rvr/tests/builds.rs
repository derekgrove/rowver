//! rvr built from no known commit, or from changes it kept no record of (SPEC §12): built again
//! here, its commit fixed through build.rs's `RVR_BUILD_COMMIT`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

/// A target folder of its own, deleted when the test ends, passed or failed.
struct Target(PathBuf);

impl Drop for Target {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// rvr built into `target` with `commit` as its commit; what the outer cargo set is not passed on.
fn built(target: &Target, commit: &str) -> PathBuf {
    let mut cargo = Command::new(env!("CARGO"));
    cargo.args(["build", "--offline", "-p", "rvr", "--bin", "rvr", "--target-dir"]).arg(&target.0);
    for set in ["CARGO_BUILD_TARGET", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_RUSTFLAGS", "RUSTFLAGS", "RVR_BUILD_COMMIT"] {
        cargo.env_remove(set);
    }
    assert!(cargo.env("RVR_BUILD_COMMIT", commit).status().unwrap().success());
    target.0.join("debug/rvr")
}

/// Run that rvr in `dir`.
fn run(rvr: &Path, dir: &Path, set: &[(&str, &str)], args: &[&str]) -> (i32, String) {
    let out = Command::new(rvr).args(args).current_dir(dir).env("RVR_CONFIG", dir.join(".rvr-config")).envs(set.iter().copied()).output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
}

const SAMPLES: &str = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nfiles = [\"uproot_flat.root\", \"copy.root\"]\ntree = \"analysis\"\n";
const RECIPE: &str = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";

// SPEC 7.5.6, 8.6, 12.2, 12.3
#[test]
#[ignore = "builds rvr a second time, about 27 s"]
fn a_build_of_no_known_commit_presses_whole_files_only() {
    let target = Target(Path::new(env!("CARGO_TARGET_TMPDIR")).join("built"));
    for commit in ["unknown", "0123456789abcdef0123456789abcdef01234567-dirty"] {
        let rvr = built(&target, commit);
        let dir = common::study("unknown-build", SAMPLES, RECIPE);
        std::fs::copy(dir.join("uproot_flat.root"), dir.join("copy.root")).unwrap();
        let name = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
        let (_, said) = run(&rvr, &dir, &[], &["--version"]);
        assert!(said.contains(&format!("; commit {commit})")), "{said}");
        assert_eq!(run(&rvr, &dir, &[], &["samples", "resolve", "samples.toml"]).0, 0);
        // Another build's stopped press is not carried on, and it says so; stopped part way, it
        // keeps no journal: pressing again starts from the first file.
        assert_eq!(common::rvr_with(&dir, &[("RVR_STOP_AFTER", "1")], &["press", "--workers", "1"]).0, 1);
        let (code, said) = run(&rvr, &dir, &[("RVR_STOP_AFTER", "1")], &["press", "--workers", "1"]);
        assert!(code == 1 && said.contains("Starting over: the press that stopped in ") && said.contains(".partial was another press: "), "{said}");
        let (code, said) = run(&rvr, &dir, &[], &["press", "--workers", "1"]);
        assert_eq!(code, 0, "{said}");
        assert!(!said.contains("Picking up") && said.contains("  1/2 "), "{said}");
        let (_, header) = run(&rvr, &dir, &[], &["header", &name]);
        let header: serde_json::Value = serde_json::from_str(&header).unwrap();
        assert_eq!(header["made"]["toolchain"]["commit"], commit);
        // A column added: it says why every column would be pressed, and presses them with --full.
        std::fs::write(dir.join("recipe.toml"), RECIPE.replace("[\"nJet\"]", "[\"nJet\", \"Jet_pt\"]")).unwrap();
        let (code, said) = run(&rvr, &dir, &[], &["press"]);
        assert!(code == 2 && said.contains("such a build builds on no file") && said.contains("Give --full to press every column without asking"), "{said}");
        let (code, said) = run(&rvr, &dir, &[], &["press", "--full"]);
        assert_eq!(code, 0, "{said}");
    }
}

// SPEC 12.2
#[test]
fn this_checkout_names_its_commit_and_any_change_to_its_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(&root).args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let (Some(head), Some(changes)) = (git(&["rev-parse", "HEAD"]), git(&["status", "--porcelain", "--", "crates", "Cargo.toml", "Cargo.lock"])) else {
        return eprintln!("not checked here: not a git checkout");
    };
    let dir = common::Study::new("version");
    let (code, said) = common::rvr(&dir, &["--version"]);
    assert_eq!(code, 0);
    let commit = said.trim().strip_prefix(&format!("rvr {} (reads and writes rowver 15; commit ", env!("CARGO_PKG_VERSION"))).and_then(|c| c.strip_suffix(')')).unwrap_or_else(|| panic!("{said}"));
    match changes.is_empty() {
        true => assert_eq!(commit, head),
        false => {
            let hash = commit.strip_prefix(&format!("{head}-dirty-")).unwrap_or_else(|| panic!("{commit}"));
            assert!(hash.len() == 12 && hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{commit}");
        }
    }
}
