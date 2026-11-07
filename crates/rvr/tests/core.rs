//! The pressing core against SPEC §2, §4 and §7, statement by statement, and FORMAT.md's running
//! example pressed again and compared byte for byte where it is fixed.

mod common;

use common::{header, name, rvr, samples, study, Study};
use serde_json::Value;
use std::path::Path;

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
    // Signed with a throwaway key, as FORMAT.md says.
    assert_eq!(rvr(&dir, &["key", "new", "Throwaway"]).0, 0);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let bytes = std::fs::read(dir.join("tiny.rvr")).unwrap();
    // The magic, the arrays from byte 8, the header, its length, and the magic again.
    let n = bytes.len();
    assert_eq!((&bytes[..8], &bytes[n - 8..]), (&common::MAGIC[..], &common::MAGIC[..]));
    let (text, arrays) = common::parts(&bytes);
    let h: Value = serde_json::from_slice(text).unwrap();
    // The header's length is the build's own (its commit and compiler are in it): FORMAT.md quotes one.
    assert_eq!((arrays.len(), n), (1962, 8 + 1962 + text.len() + 12));
    assert_eq!(&bytes[n - 12..n - 8], (text.len() as u32).to_le_bytes());
    let d = &h["data"];
    let cut = |v: &Value| v.as_str().unwrap()[..8].to_owned();
    let at = |v: &Value| (v["at"].as_u64().unwrap(), v["size"].as_u64().unwrap());
    let cols = |name: &str| d["cols"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap().clone();
    let (njet, jet, ht) = (cols("nJet"), cols("Jet_pt"), cols("ht"));
    assert_eq!([at(&d["sample"]), at(&d["entry"]), at(&njet), at(&jet["offsets"]), at(&jet["content"]), at(&ht)], [(0, 240), (240, 430), (670, 254), (924, 432), (1356, 324), (1680, 282)]);
    assert_eq!(&arrays[..6], [0x28, 0xb5, 0x2f, 0xfd, 0x04, 0x38]);
    assert_eq!((&d["rows"], &d["chunk_rows"]), (&serde_json::json!(8), &serde_json::json!(65_536)));
    let hashes = [cut(&d["sample"]["sha256"]), cut(&d["entry"]["sha256"]), cut(&njet["sha256"]), cut(&jet["offsets"]["sha256"]), cut(&jet["content"]["sha256"]), cut(&ht["sha256"]), cut(&d["sha256"])];
    assert_eq!(hashes, ["0b8e189d", "d618105e", "d9237950", "af72e3c9", "69fa98c7", "e5fc4c13", "d943535f"]);
    let both = |v: &Value| v["by_sample"].as_array().unwrap().iter().map(cut).collect::<Vec<_>>();
    assert_eq!([both(&d["entry"]), both(&njet), both(&jet["offsets"]), both(&jet["content"])], [["904543e2"; 2], ["84697797"; 2], ["439e0157"; 2], ["bcda9316"; 2]]);
    assert_eq!((&njet["lim"], &njet["nice"], &njet["vals"]), (&serde_json::json!([1, 4]), &serde_json::json!([1, 4]), &serde_json::json!([1, 3, 4])));
    assert_eq!((&ht["lim"], &ht["nice"], &jet["content"]["count"]), (&serde_json::json!([101.703125, 154.046875]), &serde_json::json!([100.0, 160.0]), &serde_json::json!(22)));
    let sample_cols: Vec<(String, String)> = d["sample_cols"].as_array().unwrap().iter().map(|c| (c["name"].as_str().unwrap().to_owned(), cut(&c["sha256"]))).collect();
    assert_eq!(&sample_cols[..2], [("kind".to_owned(), "023c71f9".to_owned()), ("xsec".to_owned(), "e98f4aa4".to_owned())]);
    assert_eq!(d["sample_cols"][1]["values"], serde_json::json!([2.5, null]));
    let mean = &h["derived"]["columns"][0];
    assert_eq!((cut(&mean["sha256"]), both(mean)), ("9e715cef".to_owned(), vec!["124d1600".to_owned(); 2]));
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
#[ignore = "makes a 2.1M-row ntuple with uproot (RVR_PYTHON, `mamba run -n rowver python` when not set) and presses it four times: run it in a release build"]
fn what_a_press_holds_does_not_grow_with_the_events_it_keeps() {
    let jets = "branches = [\"run\", \"luminosityBlock\", \"event\", \"nJet\", \"Jet_pt\", \"Jet_eta\", \"MET\", \"weight\", \"x0\", \"x1\", \"x2\", \"x3\", \"x4\", \"x5\", \"x6\", \"x7\", \"x8\", \"x9\"]\nht = \"sum(Jet_pt)\"\n";
    let recipe = recipe("", jets, "ratio = \"ht / MET\"\nlead = \"greatest(Jet_pt, -1)\"\n");
    let dir = study("memory", &samples("[[sample]]\nname = \"big\"\nfiles = \"big.root\"\ntree = \"Events\"\n"), &recipe);
    let python = std::env::var("RVR_PYTHON").unwrap_or_else(|_| "mamba run -n rowver python".into());
    let mut words = python.split_whitespace();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/make_large_ntuple.py");
    let made = std::process::Command::new(words.next().unwrap()).args(words).arg(script).arg(dir.join("big.root")).arg("2100000").output().unwrap();
    assert!(made.status.success(), "the ntuple was not made (RVR_PYTHON names a Python with uproot and awkward): {}", String::from_utf8_lossy(&made.stderr));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // A million kept events and two million: past the first million, reading holds what it will
    // hold (hep-root's basket cache is full: measured without [derived], 59 and 69 MB at one worker).
    let (mut pressed, mut built, mut widest) = (Vec::new(), Vec::new(), 0);
    for events in ["1000000", "2000000"] {
        let _ = std::fs::remove_file(dir.join(name(&dir)));
        let (peak_of, said) = peak(&dir, &["press", "--events", events]);
        let data = header(&dir)["data"].clone();
        let rows = data["rows"].as_u64().unwrap();
        eprintln!("--events {events}: {rows} rows kept, press peak {} MB\n{said}", peak_of >> 20);
        pressed.push((rows, peak_of));
        // The most bytes a row one stored column holds: a flat one's width, a jagged one's offset
        // (8) and its members (their width, times its members a row).
        for c in data["cols"].as_array().unwrap() {
            let width = |kind: &Value| rowver::rvr::width(kind.as_str().unwrap()).unwrap() as u64;
            let bytes = match c["kind"].as_str().unwrap() {
                "jagged" => 8 + width(&c["content"]["kind"]) * c["content"]["count"].as_u64().unwrap() / rows,
                _ => width(&c["kind"]),
            };
            widest = widest.max(bytes);
        }
        // Building on it: a derived column added, nothing read.
        std::fs::write(dir.join("recipe.toml"), recipe.replace("[derived]\n", "[derived]\nmemory_probe = \"1\"\n")).unwrap();
        let (peak_of, _) = peak(&dir, &["press", "--events", events]);
        std::fs::write(dir.join("recipe.toml"), &recipe).unwrap();
        eprintln!("--events {events}: build-on peak {} MB", peak_of >> 20);
        built.push((rows, peak_of));
    }
    // Twice the events kept: a press holds at most a tenth more, and 8 MB (SPEC 7.6.1).
    let ((few, small), (many, large)) = (pressed[0], pressed[1]);
    assert!(many >= 2 * few - 1 && large <= small + small / 10 + (8 << 20), "{pressed:?}");
    // Building on it, a press also holds 8 bytes a row of the file, and one of its columns at a
    // time (SPEC 7.6.2): at most `8 + widest` bytes a row more for each row more.
    let ((few, small), (many, large)) = (built[0], built[1]);
    assert!(large <= small + small / 10 + (8 << 20) + (8 + widest) * (many - few), "{built:?}, {widest} bytes a row in the widest column");
}

// FORMAT 3, 7, 8, 10; SPEC 4.1.1
#[test]
fn a_file_of_many_chunks_holds_and_slices_the_values_of_one_of_one_chunk() {
    let samples = samples("[[sample]]\nname = \"whole\"\nfiles = [\"runs.root\", \"copy.root\"]\ntree = \"Events\"\n\n[[sample]]\nname = \"one\"\nfiles = \"runs.root\"\ntree = \"Events\"\n");
    let columns = "branches = [\"run\", \"event\", \"nJet\", \"Jet_pt\", \"MET\"]\nht = \"sum(Jet_pt)\"\n";
    let recipe = recipe("some = \"nJet >= 1\"\n", columns, "ratio = \"ht / MET\"\nfirst = \"pair_first(Jet_pt)\"\n");
    let dir = study("chunks", &samples, &recipe);
    for to in ["runs.root", "copy.root"] {
        std::fs::copy(common::fixture("runs.root"), dir.join(to)).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // The same study pressed in chunks of 1,024 rows and of 65,536.
    let pressed = |chunk_rows: &str, out: &str| {
        std::fs::write(dir.join("recipe.toml"), recipe.replacen("version = 1\n", &format!("version = 1\nchunk_rows = {chunk_rows}\n"), 1)).unwrap();
        let (code, said) = common::rvr_with(&dir, &[("RVR_RUN", "97")], &["press", "--workers", "3", "--out", out]);
        assert_eq!(code, 0, "{said}");
        assert_eq!(rvr(&dir, &["validate", out]).0, 0);
        rowver::rvr::Reader::open(dir.join(out)).unwrap()
    };
    let (small, large) = (pressed("1024", "small.rvr"), pressed("65536", "large.rvr"));
    let (rows, spans) = (small.rows().unwrap(), small.chunk_spans().unwrap());
    assert!(rows > 3000 && spans.len() == 5 && large.chunk_spans().unwrap().len() == 2, "{rows} rows, chunks {spans:?}");
    // Every value, fact and count alike; only the checksums cut at the chunks differ.
    let all = |file: &str| rvr(&dir, &["dump", "--all", file]).1.lines().filter(|l| l.contains('\t')).map(str::to_owned).collect::<Vec<_>>();
    let values = all("small.rvr");
    assert!(values.len() == rows + 1 && values == all("large.rvr"));
    let (a, b) = (&small.header()["data"], &large.header()["data"]);
    assert_eq!((&a["cols"][3]["lim"], &a["cols"][3]["content"]["count"]), (&b["cols"][3]["lim"], &b["cols"][3]["content"]["count"]));
    assert_ne!(a["sha256"], b["sha256"]);
    // Rows read alone, across chunks and samples, are the column's.
    for name in ["event", "Jet_pt", "ratio", "first"] {
        let whole = small.column(name).unwrap().unwrap();
        for (from, to) in [(0, 1), (1000, 1100), (1023, 1025), (rows - 5, rows), (7, 7), (0, rows)] {
            let slice = small.slice(name, from, to).unwrap().unwrap();
            assert_eq!(slice, whole.rows(from, to).unwrap(), "{name} rows {from} to {to}");
        }
    }
    let entries = small.entry().unwrap();
    assert_eq!(small.entry_slice(1020, 1030).unwrap(), entries[1020..1030]);
    assert!(small.slice("event", 5, rows + 1).is_err());
    // Every slice is refused past the file or with its first row after its last, sample and entry too.
    for refused in [small.slice("event", 9, 3).err(), small.sample_slice(9, 3).err(), small.entry_slice(9, 3).err(), small.sample_slice(0, rows + 1).err(), small.entry_slice(rows, rows + 1).err()] {
        assert!(refused.is_some_and(|e| e.to_string().contains("the first no later than the last")));
    }
    let (code, said) = rvr(&dir, &["diff", "small.rvr", "large.rvr", "--rows"]);
    assert!(code == 0 && said.contains("The two hold the same events with the same values."), "{said}");
    // From their headers, two files of other chunks say so, and name no column as differing.
    let (code, said) = rvr(&dir, &["diff", "small.rvr", "large.rvr"]);
    assert!(code == 1 && said.contains("chunk_rows: the files are cut in chunks of 1,024 and 65,536 rows: their per-sample checksums cannot be compared; rvr diff --rows compares the events") && !said.contains("differs in sample"), "{said}");
}

// SPEC 8.5
#[test]
fn a_file_another_program_made_is_not_built_on_and_its_program_is_named() {
    let dir = copies("other-program", 0, &recipe("", "branches = [\"nJet\"]\n", ""));
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    let path = dir.join(name(&dir));
    let mut header = header(&dir);
    header["made"]["toolchain"]["program"] = "rvrcpp".into();
    header["made"]["toolchain"]["version"] = "15.1.0".into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let other = dir.join("other.rvr");
    rowver::write::rewrite(&path, &other, &header).unwrap();
    std::fs::rename(&other, &path).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains(&format!("Pressing every column: {} cannot be built on, as it was made by rvrcpp 15.1.0, not rvr", name(&dir))), "{said}");
}

// SPEC 4.2.1, 4.2.2, 7.2.1, 7.2.2, 4.5.1, 4.5.3; RECIPE functions-exactly, pairs-within-a-list; FORMAT 10
#[test]
fn a_press_writes_the_same_file_whatever_its_runs_workers_and_crowding() {
    // RVR_RUN and RVR_RUN_PAIRS are read by a debug build only.
    if !cfg!(debug_assertions) {
        return eprintln!("not checked here: a release build presses in runs of 50,000 alone");
    }
    let samples = samples("[[sample]]\nname = \"whole\"\nfiles = [\"runs.root\", \"copy.root\"]\ntree = \"Events\"\n\n[[sample]]\nname = \"picked\"\nfiles = \"runs.root\"\ntree = \"Events\"\nevents = 500\nchoose = \"random\"\nseed = 3\n");
    let recipe = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\ngood = \"Jet_pt > 40\"\n\n[keep]\nsome = \"nJet >= 2\"\n\n[columns]\nbranches = [\"nJet\", \"Jet_pt\", \"MET\"]\nht = \"sum(Jet_pt[good])\"\nmjj = \"greatest(pair_mass(Jet_pt, Jet_eta, Jet_phi), -1)\"\npairs = \"pair_first(Jet_pt) * pair_second(Jet_pt)\"\n\n[totals]\nsumw = \"weight\"\n\n[derived]\nfirst = \"pair_first(Jet_pt)\"\nratio = \"ht / MET\"\n";
    let dir = study("runs", &samples, recipe);
    for to in ["runs.root", "copy.root"] {
        std::fs::copy(common::fixture("runs.root"), dir.join(to)).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // What a press made, apart from when and by which build: the arrays, derived records and every sample's totals.
    let made = |run: &str, workers: &str, pairs: &str| {
        let _ = std::fs::remove_file(dir.join(name(&dir)));
        let (code, said) = common::rvr_with(&dir, &[("RVR_RUN", run), ("RVR_RUN_PAIRS", pairs)], &["press", "--workers", workers]);
        assert_eq!(code, 0, "run {run}, {workers} workers, pairs {pairs}: {said}");
        // The settings were taken (a debug build says so): runs of that size, and halving when forced.
        let halved = said.contains("RVR_RUN_PAIRS: ") && said.contains("worked on in halves");
        assert!(said.contains(&format!("RVR_RUN: runs of {run} entries")) && halved == (pairs != "67108864"), "run {run}, pairs {pairs}: {said}");
        let h = header(&dir);
        serde_json::json!([h["data"], h["derived"], h["made"]["files"]])
    };
    let first = made("50000", "1", "67108864");
    assert_eq!(rvr(&dir, &["validate", &name(&dir)]).0, 0);
    let rows = first[0]["rows"].as_u64().unwrap();
    assert!(rows > 900 && first[2]["whole"]["metadata"]["sumw"].as_f64().unwrap() > 0.0, "{rows}");
    for run in ["1", "7", "997", "50000"] {
        for workers in ["1", "2", "10"] {
            for pairs in ["67108864", "60"] {
                assert!(made(run, workers, pairs) == first, "run {run}, {workers} workers, pairs {pairs}: another file");
            }
        }
    }
}
