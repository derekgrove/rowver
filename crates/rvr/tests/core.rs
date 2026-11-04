//! The pressing core against SPEC §2, §4 and §7, statement by statement, and FORMAT.md's running
//! example pressed again and compared byte for byte where it is fixed.

mod common;

use common::{rvr, study, Study};
use serde_json::Value;
use std::path::Path;

fn samples(body: &str) -> String {
    format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n{body}")
}

fn recipe(keep: &str, columns: &str, derived: &str) -> String {
    format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n{keep}\n[columns]\n{columns}\n[totals]\n\n[derived]\n{derived}")
}

/// A study of one sample reading the ntuple and `copies` copies of it.
fn copies(name: &str, copies: usize, recipe: &str) -> Study {
    let files: Vec<String> = (0..=copies).map(|i| format!("\"f{i:02}.root\"")).collect();
    let dir = study(name, &samples(&format!("[[sample]]\nname = \"flat\"\nfiles = [{}]\n", files.join(", "))), recipe);
    for i in 0..=copies {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join(format!("f{i:02}.root"))).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    dir
}

fn name(dir: &Path) -> String {
    format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())
}

fn header(dir: &Path) -> Value {
    let (code, said) = rvr(dir, &["header", &name(dir)]);
    assert_eq!(code, 0, "{said}");
    serde_json::from_str(&said).unwrap()
}

// SPEC 4.3.1, 4.3.3, 7.7.3
#[test]
fn the_cutflow_counts_what_is_left_after_each_cut_in_order_over_every_file() {
    let dir = copies("cutflow", 1, &recipe("jets = \"nJet >= 1\"\nmany = \"nJet >= 3\"\n", "branches = [\"nJet\"]\n", ""));
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    // nJet is 3, 4, 3, 0, 1 in each file.
    let flow = "Read 10 entries; kept after each cut:\n  jets                                       8\n  many                                       6\n";
    assert!(said.contains(flow), "{said}");
    let dir = copies("no-cut", 0, &recipe("", "branches = [\"nJet\"]\n", ""));
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Read 5 entries; with no cuts, every one is kept.\nWrote "), "{said}");
    // Signed when you have a key, and said so.
    assert_eq!(rvr(&dir, &["key", "new"]).0, 0);
    std::fs::remove_file(dir.join(name(&dir))).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("Signed with your key "), "{said}");
}

// SPEC 4.6.3
#[test]
fn a_derived_column_that_fails_on_a_samples_first_run_stops_the_press_there() {
    let dir = copies("derived-early", 3, &recipe("", "branches = [\"nJet\"]\n", "big = \"stored(nJet * 1000, 'u1')\"\n"));
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("3000") && !said.contains("  2/4 "), "{said}");
    assert!(!dir.join(name(&dir)).exists());
}

// SPEC 7.1.2, 6.4.2
#[test]
fn the_choice_options_give_every_sample_the_rows_of_their_table() {
    let random = samples("[[sample]]\nname = \"flat\"\nfiles = [\"uproot_flat.root\"]\nevents = 3\nchoose = \"random\"\nseed = 4\n");
    let dir = study("options", &random, &recipe("", "branches = [\"nJet\"]\n", ""));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let events = |flags: &[&str]| {
        let (code, said) = rvr(&dir, &[&["press"], flags].concat());
        assert_eq!(code, 0, "{flags:?}: {said}");
        header(&dir)["made"]["files"]["flat"]["events"].clone()
    };
    let random = |n: u64, seed: u64| serde_json::json!({"choose": "random", "n": n, "seed": seed, "algorithm": "splitmix64-selection-sampling"});
    assert_eq!(events(&[]), random(3, 4));
    assert_eq!(events(&["--events", "all"]), serde_json::json!({"choose": "all"}));
    assert_eq!(events(&["--events", "2"]), serde_json::json!({"choose": "first", "n": 2}));
    assert_eq!(events(&["--events", "2", "--random"]), random(2, 1));
    assert_eq!(events(&["--events", "2", "--random", "--seed", "18446744073709551615"]), random(2, u64::MAX));
    assert_eq!(events(&["--random"]), random(3, 4));
    assert_eq!(events(&["--seed", "9"]), random(3, 9));
    let (code, said) = rvr(&dir, &["press", "--events", "all", "--random"]);
    assert_eq!(code, 2, "{said}");
    // samples plan takes the same options, and says the same choice.
    let (code, said) = rvr(&dir, &["samples", "plan", "samples.lock", "--events", "2", "--random"]);
    assert!(code == 0 && said.contains("(--events 2 --random)") && said.contains("2 of 5 events (random)"), "{said}");
}

// SPEC 7.2.1
#[test]
fn a_press_reads_as_many_files_at_once_as_there_are_cores() {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let dir = copies("workers", cores + 1, &recipe("", "branches = [\"nJet\"]\n", ""));
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    let files = cores + 2;
    assert!(said.contains(&format!("Pressing 1 sample, {files} files, {} entries, {cores} at a time:", files * 5)), "{said}");
}

// SPEC 7.3.3, 2.6
#[test]
fn a_file_unreadable_for_want_of_rights_stops_the_press_and_keeps_what_it_read() {
    let uid = std::process::Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    if uid.ok().as_deref() == Some("0") {
        return eprintln!("not checked here: root reads a file whatever its rights");
    }
    let dir = copies("rights", 2, &recipe("", "branches = [\"nJet\"]\n", ""));
    let locked = dir.join("f01.root");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("f01.root is not yours to read") && said.contains("no file was made a hole"), "{said}");
    assert!(said.contains("carries on from the 1 of 3 files done"), "{said}");
    let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
    assert!(code == 0 && said.contains("Picking up"), "{said}");
}

// SPEC 7.5.2
#[test]
fn empty_and_refill_are_refused_while_a_press_holds_the_journal() {
    let dir = copies("journal-held", 0, &recipe("", "branches = [\"nJet\"]\n", ""));
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let file = name(&dir);
    let staged = dir.join(format!("{file}.partial"));
    std::fs::create_dir(&staged).unwrap();
    let journal = std::fs::File::create(staged.join("journal.jsonl")).unwrap();
    journal.lock().unwrap();
    for command in [&["empty", &file, "--force"][..], &["refill", &file][..], &["press", "--restart"][..]] {
        let (code, said) = rvr(&dir, command);
        assert!(code == 1 && said.contains("under way"), "{command:?}: {said}");
    }
    drop(journal);
    let (code, said) = rvr(&dir, &["empty", &file, "--force"]);
    assert_eq!(code, 0, "{said}");
}

// SPEC 2.1, 2.8, 9.5
#[test]
fn each_of_the_five_inputs_changes_what_a_press_would_do() {
    let dir = copies("inputs", 0, &recipe("", "branches = [\"nJet\"]\n", ""));
    // No file at the target yet: what a press would read.
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 1 && said.contains("rvr press would press every column: 1 sample, 1 file, 5 entries."), "{said}");
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    assert_eq!(rvr(&dir, &["status"]).0, 0);
    // The options.
    let (code, said) = rvr(&dir, &["status", "--events", "2"]);
    assert!(code == 1 && said.contains("--events"), "{said}");
    // The recipe.
    std::fs::write(dir.join("recipe.toml"), recipe("", "branches = [\"nJet\", \"Jet_pt\"]\n", "")).unwrap();
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 1 && said.contains("recipe.toml"), "{said}");
    // The earlier file, gone.
    std::fs::remove_file(dir.join(name(&dir))).unwrap();
    assert!(rvr(&dir, &["status"]).1.contains("is not there yet: rvr press makes it."));
    // samples.toml, edited since the lock.
    let edited = std::fs::read_to_string(dir.join("samples.toml")).unwrap() + "# edited\n";
    std::fs::write(dir.join("samples.toml"), edited).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("samples.toml"), "{said}");
}

// FORMAT example, 1, 3, 4, 5, 6, 7, 8, 10
#[test]
fn the_running_example_of_format_md_is_what_a_press_writes() {
    let root = Study::new("format-example");
    let dir = root.join("tiny");
    std::fs::create_dir(&dir).unwrap();
    for to in ["signal.root", "background.root"] {
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uproot_flat.root"), dir.join(to)).unwrap();
    }
    let samples = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n[[sample]]\nname = \"signal\"\nlabel = \"Signal\"\nfiles = \"./signal.root\"\nmetadata = { kind = \"signal\", xsec = 2.5 }\n\n[[sample]]\nname = \"background\"\nlabel = \"Background\"\nfiles = \"./background.root\"\nmetadata = { kind = \"background\" }\n";
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\njets = \"nJet >= 1\"\n\n[columns]\nbranches = [\"nJet\", \"Jet_pt\"]\nht = \"sum(Jet_pt)\"\n\n[totals]\n\n[derived]\nmean_jet_pt = \"ht / nJet\"\n";
    std::fs::write(dir.join("samples.toml"), samples).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let bytes = std::fs::read(dir.join("tiny.rvr")).unwrap();
    assert_eq!(&bytes[..8], b"\x89RVR\r\n\x1a\n");
    let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let h: Value = serde_json::from_slice(&bytes[12..12 + length]).unwrap();
    let arrays = &bytes[12 + length..];
    assert_eq!(arrays.len(), 332);
    let d = &h["data"];
    let cut = |v: &Value| v.as_str().unwrap()[..8].to_owned();
    let at = |v: &Value| (v["at"].as_u64().unwrap(), v["size"].as_u64().unwrap());
    let cols = |name: &str| d["cols"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap().clone();
    let (njet, jet, ht) = (cols("nJet"), cols("Jet_pt"), cols("ht"));
    assert_eq!([at(&d["sample"]), at(&d["entry"]), at(&njet), at(&jet["offsets"]), at(&jet["content"]), at(&ht)], [(0, 50), (50, 51), (101, 50), (151, 52), (203, 80), (283, 49)]);
    assert_eq!(&arrays[..6], [0x28, 0xb5, 0x2f, 0xfd, 0x04, 0x38]);
    assert_eq!(d["rows"], 8);
    let hashes = [cut(&d["sample"]["sha256"]), cut(&d["entry"]["sha256"]), cut(&njet["sha256"]), cut(&jet["offsets"]["sha256"]), cut(&jet["content"]["sha256"]), cut(&ht["sha256"]), cut(&d["sha256"])];
    assert_eq!(hashes, ["eeb59ffe", "02ea099d", "4c0f18a1", "1e8928fc", "89d29a1d", "ce0457c7", "1325524b"]);
    let both = |v: &Value| v["by_sample"].as_array().unwrap().iter().map(cut).collect::<Vec<_>>();
    assert_eq!([both(&d["entry"]), both(&njet), both(&jet["offsets"]), both(&jet["content"])], [["045487dc"; 2], ["af095d9c"; 2], ["fa00c623"; 2], ["a5cf09fd"; 2]]);
    assert_eq!((&njet["lim"], &njet["nice"], &njet["vals"]), (&serde_json::json!([1, 4]), &serde_json::json!([1, 4]), &serde_json::json!([1, 3, 4])));
    assert_eq!((&ht["lim"], &ht["nice"], &jet["content"]["count"]), (&serde_json::json!([101.703125, 154.046875]), &serde_json::json!([100.0, 160.0]), &serde_json::json!(22)));
    let sample_cols: Vec<(String, String)> = d["sample_cols"].as_array().unwrap().iter().map(|c| (c["name"].as_str().unwrap().to_owned(), cut(&c["sha256"]))).collect();
    assert_eq!(&sample_cols[..2], [("kind".to_owned(), "023c71f9".to_owned()), ("xsec".to_owned(), "e98f4aa4".to_owned())]);
    assert_eq!(d["sample_cols"][1]["values"], serde_json::json!([2.5, null]));
    let mean = &h["derived"]["columns"][0];
    assert_eq!((cut(&mean["sha256"]), both(mean)), ("e641687e".to_owned(), vec!["e73c3af8".to_owned(); 2]));
    assert_eq!((&mean["lim"], &mean["nice"]), (&serde_json::json!([33.901041666666664, 120.5]), &serde_json::json!([33.0, 130.0])));
    let made = &h["made"];
    assert_eq!((cut(&made["recipe"]["sha256"]), cut(&made["samples"]["sha256"])), ("937f05be".to_owned(), "a960276a".to_owned()));
    let lock = made["lock"]["text"].as_str().unwrap();
    for line in ["samples_toml_sha256 = \"a960276a", "branches = \"2fccd77d", "bytes = 39262", "adler32 = \"bbc43743\"", "uuid = \"31e851e4-b994-11f1-ba5c-1a45bb13c76a\"", "events_total = 5"] {
        assert!(lock.contains(line), "{line}: {lock}");
    }
    let file = &made["files"]["signal"]["files"]["./signal.root"];
    assert_eq!((&file["tree"], &file["entries"]), (&serde_json::json!("analysis"), &serde_json::json!(5)));
}

/// The peak memory of rvr run in `dir`, in bytes, with what it said.
fn peak(dir: &Path, args: &[&str]) -> (u64, String) {
    let flag = if cfg!(target_os = "macos") { "-l" } else { "-v" };
    let out = std::process::Command::new("/usr/bin/time").arg(flag).arg(env!("CARGO_BIN_EXE_rvr")).args(args).current_dir(dir).output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    let line = said.lines().find(|l| l.contains("maximum resident set size") || l.contains("Maximum resident set size")).unwrap();
    let number: u64 = line.split_whitespace().find_map(|w| w.parse().ok()).unwrap();
    (if cfg!(target_os = "macos") { number } else { number * 1024 }, said)
}

// SPEC 7.6.1, 7.6.2
#[test]
#[ignore = "presses a large study, named by RVR_MEMORY_STUDY (a folder with samples.toml, samples.lock and recipe.toml)"]
fn what_a_press_holds_does_not_grow_with_the_events_it_keeps() {
    let from = std::path::PathBuf::from(std::env::var("RVR_MEMORY_STUDY").expect("RVR_MEMORY_STUDY names a study folder"));
    let run = Study::new("memory");
    let dir = run.join(from.file_name().unwrap());
    std::fs::create_dir(&dir).unwrap();
    // The texts copied, and everything else the study folder holds linked, so its relative paths read the same files.
    for entry in std::fs::read_dir(&from).unwrap().flatten() {
        let name = entry.file_name();
        match ["samples.toml", "samples.lock", "recipe.toml"].contains(&name.to_str().unwrap()) {
            true => std::fs::copy(entry.path(), dir.join(&name)).map(drop).unwrap(),
            false if !name.to_string_lossy().ends_with(".rvr") => std::os::unix::fs::symlink(entry.path(), dir.join(&name)).unwrap(),
            false => {}
        }
    }
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    let total: u64 = lock.lines().filter_map(|l| l.strip_prefix("events_total = ")).map(|n| n.parse::<u64>().unwrap()).sum();
    let mut seen = Vec::new();
    for share in [8, 2] {
        let _ = std::fs::remove_file(dir.join(name(&dir)));
        let events = (total / share).to_string();
        let (pressed, said) = peak(&dir, &["press", "--events", &events]);
        let rows = header(&dir)["data"]["rows"].as_u64().unwrap();
        // Building on it: a column dropped, only derived ones changed, nothing read.
        let recipe = std::fs::read_to_string(dir.join("recipe.toml")).unwrap();
        std::fs::write(dir.join("recipe.toml"), recipe.replace("[derived]\n", "[derived]\nmemory_probe = \"1\"\n")).unwrap();
        let (built, _) = peak(&dir, &["press", "--events", &events]);
        std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
        eprintln!("1/{share} of {total}: {rows} rows kept, press peak {} MB, build-on peak {} MB\n{said}", pressed >> 20, built >> 20);
        seen.push((rows, pressed, built));
    }
    let ((few, small, built_small), (many, large, built_large)) = (seen[0], seen[1]);
    assert!(many >= 3 * few, "{seen:?}");
    // Four times the rows: the press holds at most a quarter more.
    assert!(large <= small + small / 4, "{seen:?}");
    // Building on it: 8 bytes a row more, and one of its columns (at most 8 bytes a row again).
    assert!(built_large <= built_small + 16 * (many - few) + (64 << 20), "{seen:?}");
}
