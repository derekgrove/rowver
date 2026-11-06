//! The validators, the agreement check and the Plan in front of the pressing core: every problem
//! of a study at once, the options read one way, and a core that fails only on the data.

mod common;

use common::{reheadered, rvr, study, Study};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

/// The file a press of `dir` writes.
fn target(dir: &Path) -> String {
    format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())
}

const FLAT: &str = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nlabel = \"Flat ntuple\"\nfiles = \"uproot_flat.root\"\ntree = \"analysis\"\nmetadata = { kind = \"test\" }\n";

/// FLAT's file twice, as one sample.
const TWICE: &str = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nlabel = \"Flat ntuple\"\nfiles = \"flat_*.root\"\ntree = \"analysis\"\n";

/// A recipe storing `columns`, keeping what `keep` says.
fn recipe(keep: &str, columns: &str) -> String {
    format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n{keep}\n[columns]\n{columns}\n\n[totals]\n\n[derived]\n")
}

/// A study of TWICE, resolved.
fn twice(name: &str, recipe: &str) -> Study {
    let dir = study(name, TWICE, recipe);
    for copy in ["flat_a.root", "flat_b.root"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(copy)).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    dir
}

// SPEC 1.5, 2.2, 2.3, 3.1.2, 4.1.1, 7.1.2
#[test]
fn a_study_whose_problems_span_its_inputs_is_refused_with_every_one_at_once() {
    let dir = study("every", FLAT, &recipe("", "branches = [\"nJet\"]"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // A validator's problems in two inputs.
    std::fs::write(dir.join("samples.toml"), FLAT.replace("[places]", "colour = 1\n\n[places]")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe("", "branches = [\"nJet\"]").replace("version = 1\n", "version = 1\nselect = 2\n")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("samples.toml has no key \"colour\"") && said.contains("recipe.toml: A recipe has no key \"select\""), "{said}");
    // The agreement check's, across the lock, the recipe and the options.
    std::fs::write(dir.join("samples.toml"), format!("{FLAT}# edited\n")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe("", "branches = [\"nJet\"]\nmet = \"MET_ptt\"").replace("[derived]\n", "[derived]\nkind = \"nJet * 2\"\n")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--seed", "3"]);
    assert_eq!(code, 1, "{said}");
    for problem in ["samples.toml has changed since samples.lock was written", "--seed is for a random choice", "[columns] met: There is no column named MET_ptt", "[derived] kind has the name of a key of samples.toml"] {
        assert!(said.contains(problem), "{problem}: {said}");
    }
    assert!(said.lines().count() >= 4 && !said.contains("Pressing"), "one a line: {said}");
}

// SPEC 9.3; FORMAT 3, 7, 12
#[test]
fn a_header_whose_rows_disagree_is_shown_and_its_columns_refused_not_read_past() {
    // nJet is 3, 4, 3, 0, 1: four rows kept, in one chunk.
    let derived = recipe("jets = \"nJet >= 1\"\n", "branches = [\"nJet\"]").replace("[derived]\n", "[derived]\ntwice = \"nJet * 2\"\n");
    let dir = study("disagree", FLAT, &derived);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["press", "--out", "four.rvr"]).0, 0);
    // data.rows says 5 of a file whose samples (and chunks) hold 4: its first 5 rows are refused,
    // and every row its arrays hold is shown. A file record says 7: the derived column is refused.
    reheadered(&dir.join("four.rvr"), &dir.join("rows.rvr"), |h| h["data"]["rows"] = 5.into());
    reheadered(&dir.join("four.rvr"), &dir.join("files.rvr"), |h| h["data"]["files"][0]["rows"] = 7.into());
    let files = "the header's files hold more rows than its samples";
    let cases = [
        (&["dump", "rows.rvr"][..], 1, "rows 0 to 5 of sample are asked for, and the file has 4".to_owned()),
        (&["dump", "rows.rvr", "--all"], 0, "\nflat\t4\t1\t2\n".to_owned()),
        (&["dump", "files.rvr"], 0, format!("(not worked out: {files}")),
        (&["dump", "files.rvr", "--all"], 1, files.to_owned()),
    ];
    for (args, want, why) in cases {
        let (code, said) = rvr(&dir, args);
        assert!(code == want && said.contains("is not valid") && said.contains(&why) && !said.contains("panicked"), "{args:?}: {said}");
    }
}

// SPEC 2.6, 4.2.2, 4.2.3, 4.4.2, 7.5.3, 7.5.4, 7.5.5, 7.5.7, 8.9
#[test]
fn a_data_error_stops_the_press_only_in_a_chosen_and_kept_event_and_keeps_what_was_read() {
    // nJet is 3, 4, 3, 0, 1: x is infinite at entry 3 alone, which the cut takes away.
    let x = "x = { value = \"100 / nJet\", stored = \"u1\" }";
    let dir = study("data", FLAT, &recipe("jets = \"nJet >= 1\"\n", x));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    // An unsigned file emptied keeps its header, and says it is not signed.
    std::fs::copy(dir.join(target(&dir)), dir.join("copy.rvr")).unwrap();
    let (code, said) = rvr(&dir, &["empty", "copy.rvr", "-f"]);
    assert!(code == 0 && said.contains("keeps its study and every checksum (it is not signed)."), "{said}");
    // A file of no rows has nothing to give back: it is left as it is.
    std::fs::write(dir.join("recipe.toml"), recipe("jets = \"nJet >= 99\"\n", x).replace("[derived]\n", "[derived]\nhalf = \"x / 2\"\n")).unwrap();
    assert_eq!(rvr(&dir, &["press", "--out", "none.rvr"]).0, 0);
    for dump in [&["dump", "none.rvr"][..], &["dump", "none.rvr", "--all"]] {
        let (code, said) = rvr(&dir, dump);
        assert!(code == 0 && said.contains("half"), "{dump:?}: {said}");
    }
    let before = std::fs::read(dir.join("none.rvr")).unwrap();
    let (code, said) = rvr(&dir, &["empty", "none.rvr", "-f"]);
    assert!(code == 0 && said.contains("none.rvr has no rows, so its arrays hold no bytes: there is nothing to empty."), "{said}");
    assert_eq!(std::fs::read(dir.join("none.rvr")).unwrap(), before);
    assert_eq!(rvr(&dir, &["validate", "none.rvr"]).0, 0);
    std::fs::write(dir.join("recipe.toml"), recipe("jets = \"nJet >= 1\"\n", x)).unwrap();
    // A random choice of entries that leaves entry 3 out reads no error either, without the cut.
    std::fs::write(dir.join("recipe.toml"), recipe("", x)).unwrap();
    let seed = (1..100).find(|seed| !rowver::choice::per_file(&[5], 2, *seed)[0].contains(&3)).unwrap().to_string();
    let (code, said) = rvr(&dir, &["press", "--events", "2", "--random", "--seed", &seed]);
    assert_eq!(code, 0, "{said}");
    // Chosen and kept, it stops the press naming the file, the entry, the column and the value.
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("uproot_flat.root, tree analysis, entry 3: [columns] x: inf does not fit in u1"), "{said}");

    // Of two files, the one read keeps: the next press of the same study carries on from it.
    let late = "x = { value = \"if(sample.file == './flat_b.root', 100 / (nJet - nJet), 1)\", stored = \"u1\" }";
    let dir = twice("data-late", &recipe("", late));
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("flat_b.root, tree analysis, entry 0: [columns] x: ") && said.contains("carries on from the 1 of 2 files done") && said.contains("mend the recipe"), "{said}");
    assert!(dir.join(format!("{}.partial", target(&dir))).is_dir(), "what was read is kept");
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert!(code == 1 && said.contains("Picking up") && said.contains("1 of 2 files done"), "{said}");
    std::fs::write(dir.join("recipe.toml"), recipe("", "x = { value = \"1\", stored = \"u1\" }\nbranches = [\"nJet\"]")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Starting over") && !dir.join(format!("{}.partial", target(&dir))).exists(), "{said}");
}

// SPEC 2.6, 4.5.2
#[test]
fn a_file_without_the_tree_or_branch_a_total_sums_stops_the_press() {
    let vectors = FLAT.replace("uproot_flat.root", "stl_vectors.root").replace("tree = \"analysis\"", "tree = \"ntuple/Events\"");
    let total = |tree: &str, branch: &str, column: &str| recipe("", &format!("branches = [\"{column}\"]")).replace("[totals]\n", &format!("[totals]\nsum = {{ value = \"{branch}\", tree = \"{tree}\" }}\n"));
    let dir = study("other-tree", &vectors, &total("analysis", "x", "evt"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("stl_vectors.root: [totals] sum sums the tree \"analysis\", which this file does not have") && !said.contains("hole"), "{said}");
    let dir = study("other-branch", FLAT, &total("other", "y", "nJet"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("uproot_flat.root: [totals] sum: its tree \"other\" has no branch y") && !said.contains("hole"), "{said}");
}

// SPEC 2.3, 2.6, 4.5.4
#[test]
fn a_total_that_is_a_list_is_refused_before_reading_or_stops_as_a_data_error_over_another_tree() {
    // Over the events' tree, its kind is known from the lock: refused before anything is read.
    let list = recipe("", "branches = [\"nJet\"]").replace("[totals]\n", "[totals]\nw = \"Jet_pt\"\n");
    let dir = study("total-list", FLAT, &list);
    // resolve refuses it as press does, and writes no lock; resolved with the column alone, press refuses it too.
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("[totals] w: a total sums one number per event") && !dir.join("samples.lock").exists(), "{said}");
    std::fs::write(dir.join("recipe.toml"), recipe("", "branches = [\"nJet\"]")).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::write(dir.join("recipe.toml"), &list).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("[totals] w: a total sums one number per event, and this gives a list for each: sum the list, as w = \"sum(Jet_pt)\"") && !said.contains("Pressing"), "{said}");
    // Over another tree, it is first typed as each file is read: a data error naming the tree.
    std::fs::write(dir.join("recipe.toml"), recipe("", "branches = [\"nJet\"]").replace("[totals]\n", "[totals]\nw = { value = \"x * sample.kind\", tree = \"other\" }\n")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("uproot_flat.root: tree \"other\": [totals] w: ") && said.contains("mend it to one number per entry of that tree") && !said.contains("bug"), "{said}");
}

// SPEC 2.4, 3.8.1
#[test]
fn a_storage_host_with_no_door_is_refused_before_reading() {
    let dir = study("no-door", FLAT, &recipe("", "branches = [\"nJet\"]"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let url = "root://nodoor.example.org//store/flat.root";
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap().replace("./uproot_flat.root", url).replace("\"uproot_flat.root\"", &format!("{url:?}"));
    std::fs::write(dir.join("samples.lock"), lock).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains(&format!("samples.lock names {url}: nodoor.example.org has no WebDAV door in the WLCG registry")) && !said.contains("Pressing"), "{said}");
}

// SPEC 1.4, 5.1.1, 5.1.2, 5.1.3, 5.1.4, 5.1.6, 6.4.2
#[test]
fn every_command_reads_its_options_one_way_and_refuses_wrong_use_with_2() {
    let dir = study("options", FLAT, &recipe("", "branches = [\"nJet\"]"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // The command is the first argument that names one, flags before it or not.
    let (code, said) = rvr(&dir, &["--holes", "status"]);
    assert!(code == 1 && said.contains("is not there yet: rvr press makes it"), "{said}");
    let (code, said) = rvr(&dir, &["--out", "x.rvr", "status"]);
    assert!(code == 1 && said.contains("x.rvr is not there"), "a flag's value is passed over: {said}");
    // A word that names no command is refused, never read past for one that does.
    for (args, near) in [(&["prss", "new"][..], "press"), (&["stauts", "samples"], "status"), (&["derivee", "f.rvr", "--drop", "key"], "derive"), (&["samples", "resolv", "samples.toml"], "resolve")] {
        let (code, said) = rvr(&dir, args);
        assert!(code == 2 && said.contains(&format!("Did you mean: {near}?")), "{args:?}: {said}");
    }
    assert!(!dir.join("prss").exists());
    let wrong: &[(&[&str], &str)] = &[
        (&["press", "--evnts", "1"], "--events"),
        (&["status", "--hole"], "--holes"),
        (&["branches", "--fnd", "x"], "--find"),
        (&["try", "nJet", "--kep"], "--kept"),
        (&["derive", "f.rvr", "--dorp", "x"], "--drop"),
        (&["validate", "f.rvr", "--complet"], "--complete"),
        (&["dump", "f.rvr", "--column"], "--columns"),
        (&["reproduce", "f.rvr", "--studdy", "x"], "--study"),
        (&["empty", "f.rvr", "--forse"], "--force"),
        (&["refill", "f.rvr", "--worker", "2"], "--workers"),
        (&["samples", "plan", "samples.lock", "--event", "1"], "--events"),
        (&["samples", "expand", "samples.toml", "--jsn"], "--json"),
        (&["proxy", "init", "--hour", "3"], "--hours"),
    ];
    for (args, near) in wrong {
        let (code, said) = rvr(&dir, args);
        assert!(code == 2 && said.contains(&format!("Did you mean: {near}?")) && said.contains("usage:"), "{args:?}: {said}");
    }
    for args in [&["header", "a", "b"][..], &["diff", "a"], &["new"], &["key", "show", "x"], &["press", "--events=5"], &["press", "--random", "--random"], &["press", "--out"], &["press", "--workers", "0"], &["proxy", "init", "--hours", "721"], &["try", "nJet", "--events", "9223372036854775808"], &["empty", "x.rvr", "-f", "--force"]] {
        let (code, said) = rvr(&dir, args);
        assert!(code == 2 && said.contains(&format!("rvr {}", args[0])), "{args:?}: {said}");
    }
    // The usage names every command and flag of SPEC §5.2.
    let (code, said) = rvr(&dir, &["help"]);
    for part in ["rvr samples plan", "--hours H", "rvr key trust KEY NAME", "rvr key show", "rvr --version", "--force | -f", "--all", "--holes"] {
        assert!(code == 0 && said.contains(part), "{part}: {said}");
    }
    // A count may hold _ between its digits.
    let (code, said) = rvr(&dir, &["press", "--events", "50_000"]);
    assert!(code == 0 && said.contains("Pressing 1 sample, 1 file, 5 entries"), "{said}");
    // The plan's first line names every option given, the seed too.
    let (code, said) = rvr(&dir, &["samples", "plan", "samples.lock", "--events", "3", "--random", "--seed", "8"]);
    assert!(code == 0 && said.contains("(--events 3 --random --seed 8)"), "{said}");
    // The word after --fill is KINDS when every part is a kind, a folder of that name or not.
    std::fs::create_dir(dir.join("gone")).unwrap();
    let (code, said) = rvr(&dir, &["press", "--fill", "gone"]);
    assert!(code == 1 && said.contains("has no holes: there is nothing to fill"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--fill", "gon"]);
    assert!(code == 2 && said.contains("(did you mean --fill gone?)"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--fill", "gone,damage", "."]);
    assert!(code == 2 && said.contains("(did you mean --fill gone,damaged?)"), "{said}");
    // A study with neither samples.toml nor samples.lock is told to write the one, then resolve.
    let bare = study("bare", FLAT, &recipe("", "branches = [\"nJet\"]"));
    std::fs::remove_file(bare.join("samples.toml")).unwrap();
    let (code, said) = rvr(&bare, &["press"]);
    assert!(code == 1 && said.contains("has no samples.toml: write one") && !said.contains("has a samples.lock"), "{said}");
}

// SPEC 5.1.6
#[test]
fn a_closed_output_pipe_ends_the_command_with_141() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let status = Command::new(env!("CARGO_BIN_EXE_rvr")).arg("help").stdout(Stdio::from(writer)).stderr(Stdio::null()).status().unwrap();
    assert_eq!(status.code(), Some(141));
}

// SPEC 5.3.1, 10.1, 10.2
#[test]
fn new_names_the_study_after_its_folder_and_an_empty_config_variable_is_not_set() {
    let dir = study("named", FLAT, "");
    let at = dir.join("higgs");
    std::fs::create_dir(&at).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rvr")).args(["new", "."]).current_dir(&at).output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && said.contains("rvr validate higgs.rvr"), "{said}");
    assert!(std::fs::read_to_string(at.join("recipe.toml")).unwrap().contains("title = \"higgs\""));
    // RVR_CONFIG and XDG_CONFIG_HOME set but empty: the key goes to ~/.config/rvr, never here.
    let out = Command::new(env!("CARGO_BIN_EXE_rvr")).args(["key", "new"]).current_dir(&at).env("RVR_CONFIG", "").env("XDG_CONFIG_HOME", "").env("HOME", &*dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.join(".config/rvr/key").is_file() && !at.join("key").exists());
}

// SPEC 8.8, 9.3, 9.4
#[test]
fn header_prints_its_bytes_and_diff_and_derive_say_what_they_find() {
    let dir = study("inspect", FLAT, &recipe("", "branches = [\"nJet\"]"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["key", "new", "Me"]).0, 0);
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let file = dir.join(target(&dir));
    let bytes = std::fs::read(&file).unwrap();
    let (text, _) = common::parts(&bytes);
    let out = Command::new(env!("CARGO_BIN_EXE_rvr")).args(["header", file.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success() && out.stdout == text, "the header's bytes as the file holds them");
    // A header that fails its checks is printed all the same, and why is said (exit 1).
    let mut header: Value = serde_json::from_slice(text).unwrap();
    header["made"]["study"] = "other".into();
    let broken = dir.join("broken.rvr");
    rowver::write::rewrite(&file, &broken, &header).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rvr")).args(["header", broken.to_str().unwrap()]).output().unwrap();
    assert!(out.status.code() == Some(1) && !out.stdout.is_empty() && String::from_utf8_lossy(&out.stderr).contains("fails its checks"));
    let (code, said) = rvr(&dir, &["diff", &target(&dir), "broken.rvr"]);
    assert!(code == 1 && said.contains("broken.rvr: its header fails its checks, and is compared all the same"), "{said}");
    // A file against its emptied copy is not "no difference".
    std::fs::copy(&file, dir.join("copy.rvr")).unwrap();
    assert_eq!(rvr(&dir, &["empty", "copy.rvr", "-f"]).0, 0);
    let (code, said) = rvr(&dir, &["diff", &target(&dir), "copy.rvr"]);
    assert!(code == 1 && said.contains("copy.rvr is empty: its arrays are gone"), "{said}");
    // derive says whose signature it replaces.
    std::fs::remove_dir_all(dir.join(".rvr-config")).unwrap();
    assert_eq!(rvr(&dir, &["key", "new", "Other"]).0, 0);
    let (code, said) = rvr(&dir, &["derive", &target(&dir), "twice = nJet * 2"]);
    assert!(code == 0 && said.contains("which replaces the signature of key ") && said.contains("which you do not trust"), "{said}");
}

// SPEC 2.2; FORMAT 6.1
#[test]
fn a_lock_whose_random_takes_are_not_the_choices_shares_is_refused() {
    let samples = TWICE.replace("tree = \"analysis\"\n", "tree = \"analysis\"\nevents = 3\nchoose = \"random\"\nseed = 7\n");
    let dir = study("takes", &samples, &recipe("", "branches = [\"nJet\"]"));
    for copy in ["flat_a.root", "flat_b.root"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(copy)).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    let takes: Vec<&str> = lock.lines().filter(|l| l.starts_with("take = ")).collect();
    assert!(takes.len() == 2 && takes[0] != takes[1], "seed 7 splits 3 of 10 unevenly: {takes:?}");
    // Swapped, they add up as before, and neither is its file's share.
    let swapped = lock.replacen(takes[0], "TAKE", 1).replacen(takes[1], takes[0], 1).replacen("TAKE", takes[1], 1);
    std::fs::write(dir.join("samples.lock"), swapped).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("samples.lock does not check out: sample flat's file ./flat_a.root has take"), "{said}");
}

// SPEC 1.5, 2.3, 3.8.1
#[test]
fn a_storage_host_with_no_door_hides_no_problem_of_the_recipe() {
    let dir = study("doorless", FLAT, &recipe("", "branches = [\"nJet\"]"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    std::fs::write(dir.join("recipe.toml"), recipe("", "branches = [\"nJett\"]")).unwrap();
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    let far = lock.replace("./uproot_flat.root", "root://no-door.invalid//d/uproot_flat.root").replace("[\"uproot_flat.root\"]", "[\"root://no-door.invalid//d/uproot_flat.root\"]");
    std::fs::write(dir.join("samples.lock"), far).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("no-door.invalid has no WebDAV door") && said.contains("nJett"), "{said}");
}
