//! `rvr samples resolve` on local files: what it reads, what it refuses, and the lock it writes.

mod common;

use common::{rvr, samples, study};

const RECIPE: &str = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\"]\n\n[totals]\n\n[derived]\n";

/// One sample reading `files` (a TOML value).
fn reading(files: &str) -> String {
    samples(&format!("[[sample]]\nname = \"flat\"\nfiles = {files}\n"))
}

// SPEC 6.2.4
#[test]
fn resolve_reads_no_event() {
    let dir = study("no-event", &reading("\"uproot_flat.root\""), RECIPE);
    // A basket of nJet (bytes 1122 to 1216) made unreadable: the file still resolves.
    let path = dir.join("uproot_flat.root");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[1130..1210].fill(0xff);
    std::fs::write(&path, bytes).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("damaged"), "{said}");
}

// SPEC 6.2.7, 6.2.8, 3.8.3
#[test]
fn resolve_writes_all_or_nothing_and_says_how_to_fix_each_failure() {
    let good = reading("\"uproot_flat.root\"");
    let dir = study("all-or-nothing", &good, RECIPE);
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    let lock = std::fs::read(dir.join("samples.lock")).unwrap();
    let bad = samples("[[sample]]\nname = \"ok\"\nfiles = \"uproot_flat.root\"\n[[sample]]\nname = \"typo\"\nfiles = \"uproot_flt.root\"\n[[sample]]\nname = \"tree\"\nfiles = \"codecs-zlib.root\"\n[[sample]]\nname = \"rnt\"\nfiles = \"rntuple.root\"\ntree = \"Events\"\n");
    std::fs::write(dir.join("samples.toml"), bad).unwrap();
    std::fs::copy(common::fixture("rntuple.root"), dir.join("rntuple.root")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("  FAIL  typo: ") && said.contains("Did you mean: uproot_flat.root?"), "{said}");
    assert!(said.contains("  FAIL  tree: ") && said.contains("Its trees: Events, Empty"), "{said}");
    assert!(said.contains("  FAIL  rnt: ./rntuple.root holds \"Events\" as an RNTuple, which rvr cannot read (it reads TTrees): write its events as a TTree"), "{said}");
    assert!(!said.contains("FAIL  ok"), "{said}");
    // The old lock stands as it was, and nothing staged is left beside it.
    assert_eq!(std::fs::read(dir.join("samples.lock")).unwrap(), lock);
    let left: Vec<_> = std::fs::read_dir(&*dir).unwrap().flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().contains("rvr-new")).collect();
    assert!(left.is_empty(), "{left:?}");
}

// SPEC 1.5, 6.2.7
#[test]
fn a_file_cut_short_before_resolve_is_said_to_be_cut_short() {
    let dir = study("cut-short", &reading("\"uproot_flat.root\""), RECIPE);
    let path = dir.join("uproot_flat.root");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("uproot_flat.root is cut short (header says the file ends at ") && said.contains("put a whole copy in its place, then run resolve again"), "{said}");
    assert!(!said.contains("changed during reading"), "{said}");
}

// SPEC 3.8.2, 3.8.3, 6.2.9
#[test]
fn a_pattern_matches_in_byte_order_and_a_link_counts_as_its_file() {
    let dir = study("patterns", &reading("\"*.root\""), RECIPE);
    for name in ["d/B.root", "d/a.root", "e/a1.root", "e/a10.root"] {
        std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(name)).unwrap();
    }
    std::os::unix::fs::symlink(dir.join("uproot_flat.root"), dir.join("d/link.root")).unwrap();
    std::fs::write(dir.join("samples.toml"), reading("[\"d/*.root\", \"e/a?.root\"]")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    let at: Vec<&str> = lock.lines().filter_map(|l| l.strip_prefix("at = ")).collect();
    assert_eq!(at, ["\"d/B.root\"", "\"d/a.root\"", "\"d/link.root\"", "\"e/a1.root\""]);
    // The folders read hold a .root file no sample reads: it is named, as a check.
    assert!(said.contains("also hold 1 .root file no sample reads") && said.contains("  e/a10.root"), "{said}");
}

// SPEC 3.8.4, 3.6.3
#[test]
fn a_file_reached_twice_or_a_file_placeholder_over_two_files_is_refused() {
    let dir = study("twice", &reading("[\"uproot_flat.root\", \"uproot_*.root\"]"), RECIPE);
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("uproot_flat.root") && said.contains("twice"), "{said}");
    assert!(!dir.join("samples.lock").exists());
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("uproot_two.root")).unwrap();
    let one = samples("[[sample]]\nname = \"flat\"\nfiles = \"uproot_*.root\"\nmetadata = { from = \"{file}\" }\n");
    std::fs::write(dir.join("samples.toml"), one).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("{file}"), "{said}");
}

// SPEC 3.1.1
#[test]
fn a_samples_toml_that_is_not_utf8_is_refused_in_plain_words() {
    let dir = study("latin1", &reading("\"uproot_flat.root\""), RECIPE);
    let mut bytes = reading("\"uproot_flat.root\"").into_bytes();
    bytes.extend(b"# caf\xe9\n");
    std::fs::write(dir.join("samples.toml"), bytes).unwrap();
    let (code, said) = rvr(&dir, &["samples", "expand", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("UTF-8"), "{said}");
}

/// A study resolved with the good recipe, then given `recipe`: resolve again must refuse it, saying
/// `says`, and leave the lock as it was.
fn gated(name: &str, samples: &str, recipe: &str, says: &str) {
    let dir = study(name, samples, RECIPE);
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    let lock = std::fs::read(dir.join("samples.lock")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(says) && said.contains("  FAIL  ") && said.contains("samples.lock was not written"), "{said}");
    assert_eq!(std::fs::read(dir.join("samples.lock")).unwrap(), lock, "the lock left as it was");
    // Nor is one written where there was none.
    std::fs::remove_file(dir.join("samples.lock")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 1);
    assert!(!dir.join("samples.lock").exists());
    // Press still refuses it, for a recipe changed after resolve.
    std::fs::write(dir.join("samples.lock"), &lock).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains(says), "{said}");
}

// SPEC 6.2.1, 6.2.7, 2.3
#[test]
fn resolve_writes_no_lock_the_recipe_cannot_be_pressed_from() {
    let one = reading("\"uproot_flat.root\"");
    gated("gate-branch", &one, &RECIPE.replace("[\"nJet\"]", "[\"nJet\", \"Jet_ptt\"]"), "Jet_ptt");
    gated("gate-define", &one, &RECIPE.replace("[define]\n", "[define]\nJet_pt = \"nJet * 2\"\n"), "Jet_pt");
    let two = samples("[[sample]]\nname = \"a\"\nfiles = \"uproot_flat.root\"\nmetadata = { scale = 1 }\n[[sample]]\nname = \"b\"\nfiles = \"two.root\"\nmetadata = { scale = 1.5 }\n");
    let dir = study("gate-kinds-files", &two, RECIPE);
    let file = |name: &str| dir.join(name);
    std::fs::copy(file("uproot_flat.root"), file("two.root")).unwrap();
    let two_kinds = RECIPE.replace("[derived]\n", "[derived]\nscaled = \"nJet * sample.scale\"\n");
    // Each sample's kind: i8 for one, f8 for the other.
    std::fs::write(file("recipe.toml"), &two_kinds).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("scaled") && !file("samples.lock").exists(), "{said}");
    std::fs::write(file("recipe.toml"), RECIPE).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "a good study resolves as before: {said}");
    assert!(said.contains("Wrote samples.lock"), "{said}");
}

// SPEC 6.2.1
#[test]
fn a_study_without_a_recipe_is_refused_at_resolve_pointing_at_rvr_new() {
    let dir = study("no-recipe", &reading("\"uproot_flat.root\""), RECIPE);
    std::fs::remove_file(dir.join("recipe.toml")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("recipe.toml") && said.contains("rvr new"), "{said}");
    assert!(!dir.join("samples.lock").exists());
    // A recipe its validator refuses is refused before any file is found.
    std::fs::write(dir.join("recipe.toml"), RECIPE.replace("nJet\"]", "nJet\"]\nht = \"sum(\"")).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("[columns] ht") && !said.contains("  ok  "), "{said}");
}

// SPEC 5.4.1, 5.4.2
#[test]
fn branches_lists_a_samples_first_file_before_any_lock_or_recipe() {
    let two = samples("[[sample]]\nname = \"flat\"\nfiles = \"uproot_*.root\"\n[[sample]]\nname = \"other\"\nfiles = \"stl_vectors.root\"\ntree = \"ntuple/Events\"\n");
    let dir = study("branches-first", &two, RECIPE);
    std::fs::remove_file(dir.join("recipe.toml")).unwrap();
    std::fs::copy(dir.join("uproot_flat.root"), dir.join("uproot_more.root")).unwrap();
    let (code, said) = rvr(&dir, &["branches"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("flat: uproot_flat.root, tree analysis, 17 branches\n") && said.contains("\n  nJet             i4  flat\n"), "{said}");
    let (code, said) = rvr(&dir, &["branches", "--sample", "other", "--find", "label"]);
    assert!(code == 0 && said.contains("other: stl_vectors.root, tree ntuple/Events") && said.contains("Not readable by rvr:"), "{said}");
    // Nothing is written, and with a lock the list is the lock's, the same.
    assert!(!dir.join("samples.lock").exists());
    let without = rvr(&dir, &["branches"]).1;
    std::fs::write(dir.join("recipe.toml"), "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\none = \"1\"\n\n[totals]\n\n[derived]\n").unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let with = rvr(&dir, &["branches"]).1;
    assert_eq!(with.lines().filter(|l| l.starts_with("  ")).collect::<Vec<_>>(), without.lines().filter(|l| l.starts_with("  ")).collect::<Vec<_>>());
    // A sample whose file cannot be found says so, as resolve would.
    std::fs::remove_file(dir.join("samples.lock")).unwrap();
    std::fs::write(dir.join("samples.toml"), samples("[[sample]]\nname = \"flat\"\nfiles = \"uproot_flt.root\"\n")).unwrap();
    let (code, said) = rvr(&dir, &["branches"]);
    assert!(code == 1 && said.contains("Did you mean: uproot_flat.root?"), "{said}");
}
