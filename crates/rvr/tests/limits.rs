//! The stated limits taken at their lines and refused just past them, as rvr meets them, and
//! studies far larger than the examples: 10,000 files, 1,000 columns.

mod common;

use common::{fixture, header, name, rvr, rvr_at_home, samples, study};
use std::path::Path;

fn recipe(columns: &str) -> String {
    format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\n{columns}\n[totals]\n\n[derived]\n")
}

/// uproot_flat.root written to `to`, its tree `analysis` saying it holds `entries`: the tree's
/// fEntries (big-endian, after TAttMarker's size 1.0) lies uncompressed in that file. Its baskets
/// hold the first 5.
fn claiming(to: &Path, entries: u64) {
    let mut bytes = std::fs::read(fixture("uproot_flat.root")).unwrap();
    let five = [&1.0_f32.to_be_bytes()[..], &5_u64.to_be_bytes()].concat();
    let found: Vec<usize> = bytes.windows(12).enumerate().filter(|(_, w)| *w == five).map(|(at, _)| at + 4).collect();
    assert_eq!(found.len(), 1, "analysis's fEntries is not where it was");
    bytes[found[0]..found[0] + 8].copy_from_slice(&entries.to_be_bytes());
    std::fs::write(to, bytes).unwrap();
}

/// The study's file, validated in full and complete: what rvr validate says of it.
fn valid(dir: &Path) -> String {
    let (code, said) = rvr(dir, &["validate", "--complete", &name(dir)]);
    assert_eq!(code, 0, "{said}");
    said
}

// FORMAT 6.1, 6.3; SPEC 6.2.1, 6.2.7
#[test]
fn a_sample_of_two_to_the_40_entries_is_resolved_pressed_and_valid_and_one_entry_more_is_refused() {
    let dir = study("two-to-the-40", &samples("[[sample]]\nname = \"big\"\nfiles = [\"big.root\", \"uproot_flat.root\"]\nevents = 3\n"), &recipe("branches = [\"nJet\"]\n"));
    // 2^40 − 5 entries and the 5 of uproot_flat.root: 2^40 in all, as many as a sample may hold.
    claiming(&dir.join("big.root"), (1 << 40) - 5);
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 0 && said.contains("3 of 1,099,511,627,776 events"), "{said}");
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    assert!(lock.contains("events_total = 1099511627776\n"), "{lock}");
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(valid(&dir).contains("valid, 3 rows, 1 column, read 3 of 1,099,511,627,776 entries"));
    // One entry more is refused, and the lock left as it was.
    claiming(&dir.join("big.root"), (1 << 40) - 4);
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("  FAIL  big: its files hold more than 2^40 entries in all, more than a sample of a .rvr file may: split them among samples of fewer files"), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("samples.lock")).unwrap(), lock);
    // One file alone past it: it is to be written again as smaller ones, for events = N does not help.
    claiming(&dir.join("big.root"), (1 << 40) + 1);
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("  FAIL  big: one of its files holds more than 2^40 entries, more than a sample of a .rvr file may: write that file again as smaller ones and split them among samples"), "{said}");
}

// FORMAT 1, 6.1; SPEC 5.1.4, 7.1.2
#[test]
fn the_counts_a_flag_takes_are_taken_at_their_largest() {
    let dir = study("largest", &samples("[[sample]]\nname = \"flat\"\nfiles = \"uproot_flat.root\"\n"), &recipe("branches = [\"nJet\"]\n"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    // --events 2^63 − 1 (and --seed 2^64 − 1) reach the file's header as they were given, a valid file.
    for (flags, events) in [
        (&["--events", "9223372036854775807"][..], r#""events":{"choose":"first","n":9223372036854775807}"#),
        (&["--events", "9223372036854775807", "--random", "--seed", "18446744073709551615"], r#""events":{"choose":"random","n":9223372036854775807,"seed":18446744073709551615,"#),
    ] {
        let (code, said) = rvr(&dir, &[&["press"], flags].concat());
        assert_eq!(code, 0, "{said}");
        let (code, text) = rvr(&dir, &["header", &name(&dir)]);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains(events), "{text}");
        assert!(valid(&dir).contains("read all 5 entries"));
    }
    let (code, said) = rvr(&dir, &["try", "nJet", "--events", "9223372036854775807"]);
    assert!(code == 0 && said.contains("5 shown of its 5 entries"), "{said}");
    // --hours 720 is taken, and rvr goes on to look for the certificate.
    let (code, said) = rvr_at_home(&dir, &dir, &["proxy", "init", "--hours", "720"]);
    assert!(code == 1 && said.contains("No grid certificate was found") && !said.contains("--hours"), "{said}");
}

// SPEC 6.2.7
#[test]
fn the_trees_of_a_file_lacking_the_tree_are_listed_three_folders_deep() {
    let dir = study("trees", &samples("[[sample]]\nname = \"flat\"\nfiles = \"uproot_flat.root\"\ntree = \"Events\"\n"), &recipe("branches = [\"nJet\"]\n"));
    // deep/a/b/t lies 3 folders deep; deep/a/b/c/hidden, 4, is past the listing.
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("has no tree \"Events\". Its trees: analysis, other, ntuple/Events, deep/a/b/t (1 folder, past the 3 folders deep or 64 folders the listing opens, was not looked in).\n"), "{said}");
}

// FORMAT 3, 6.3; SPEC 6.2.1, 7.2.2
#[test]
#[ignore = "presses 10,000 files, about 15 s in a debug build: cargo test -p rvr --test limits -- --ignored"]
fn a_lock_of_ten_thousand_files_is_resolved_pressed_and_valid() {
    let dir = study("ten-thousand", &samples("[[sample]]\nname = \"many\"\nfiles = \"in/f*.root\"\n"), &recipe("branches = [\"nJet\", \"Jet_pt\"]\n"));
    // Hard links: 10,000 files of one ntuple, each a file of its own name.
    std::fs::create_dir(dir.join("in")).unwrap();
    for i in 0..10_000 {
        std::fs::hard_link(dir.join("uproot_flat.root"), dir.join(format!("in/f{i:05}.root"))).unwrap();
    }
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 0 && said.contains("10,000 distinct files"), "{said}");
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains(&format!("Wrote {} (50,000 rows, 2 columns", name(&dir))), "{said}");
    assert!(valid(&dir).contains("valid, 50,000 rows, 2 columns, read all 50,000 entries"));
    let paths: Vec<String> = header(&dir)["data"]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_owned()).collect();
    assert_eq!(paths, (0..10_000).map(|i| format!("in/f{i:05}.root")).collect::<Vec<_>>());
    let (code, said) = rvr(&dir, &["samples", "check", "samples.lock"]);
    assert!(code == 0 && said.contains("Every one of the 10,000 files is as the lock recorded it"), "{said}");
}

// FORMAT 3, 4, 7; SPEC 4.1.2
#[test]
fn a_recipe_of_a_thousand_columns_is_pressed_and_valid() {
    let columns: String = (0..1000).map(|k| format!("c{k:04} = \"weight * {k} + nJet\"\n")).collect();
    let dir = study("thousand", &samples("[[sample]]\nname = \"flat\"\nfiles = \"uproot_flat.root\"\n"), &recipe(&format!("branches = [\"Jet_pt\"]\n{columns}")));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains(&format!("Wrote {} (5 rows, 1,001 columns", name(&dir))), "{said}");
    assert!(valid(&dir).contains("valid, 5 rows, 1,001 columns"));
    let header = header(&dir);
    let names: Vec<&str> = header["data"]["cols"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 1001);
    assert_eq!((names[0], names[1], names[1000]), ("Jet_pt", "c0000", "c0999"));
    let (code, said) = rvr(&dir, &["dump", &name(&dir), "--columns"]);
    assert!(code == 0 && said.contains("c0999"), "{}", &said[..said.len().min(2000)]);
}

// SPEC 3.5.10, 6.2.1; FORMAT 1
#[test]
fn metadata_nested_as_deep_as_a_header_holds_is_pressed_and_one_deeper_is_refused_at_resolve() {
    let deep = |n: usize| samples(&format!("[[sample]]\nname = \"flat\"\nfiles = \"uproot_flat.root\"\nmetadata = {{ deep = {}1{} }}\n", "[".repeat(n), "]".repeat(n)));
    let dir = study("nested", &deep(59), &recipe("branches = [\"nJet\"]\n"));
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(valid(&dir).contains("valid"));
    std::fs::write(dir.join("samples.toml"), deep(60)).unwrap();
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("nest more than 59 deep"), "{said}");
}
