//! `rvr diff A B --rows` (SPEC §9.4.1): two files' events joined by identity and compared value by
//! value, on presses of runs.root.

mod common;

use common::{rvr, study, Study};
use std::path::{Path, PathBuf};

const SAMPLES: &str = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"Events\"\n\n[eras]\n\n[[sample]]\nname = \"one\"\nfiles = \"runs.root\"\n\n[[sample]]\nname = \"two\"\nfiles = \"copy.root\"\n";

fn recipe(cut: &str, ht: &str) -> String {
    format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\njets = \"nJet >= {cut}\"\n\n[columns]\nbranches = [\"run\", \"event\", \"nJet\", \"Jet_pt\", \"MET\"]\nht = \"{ht}\"\n\n[totals]\n\n[derived]\nratio = \"ht / MET\"\n")
}

/// A press of runs.root (sample one) and a copy of it (sample two), samples in this order, with
/// this cut and ht; the file it wrote, moved out of the study as `name`.rvr.
fn pressed(root: &Path, name: &str, samples: &str, cut: &str, ht: &str) -> PathBuf {
    let dir = study(name, samples, &recipe(cut, ht));
    for to in ["runs.root", "copy.root"] {
        std::fs::copy(common::fixture("runs.root"), dir.join(to)).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    let to = root.join(format!("{name}.rvr"));
    std::fs::rename(dir.join(format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())), &to).unwrap();
    to
}

fn diff(root: &Path, a: &Path, b: &Path, more: &[&str]) -> (i32, String) {
    let words = [&["diff", a.to_str().unwrap(), b.to_str().unwrap(), "--rows"][..], more].concat();
    rvr(root, &words)
}

// SPEC 9.4.1, 5.1.6
#[test]
fn rows_are_matched_by_file_and_entry_whatever_the_samples_order() {
    let root = Study::new("diff-rows");
    let reordered = SAMPLES.replace("name = \"one\"\nfiles = \"runs.root\"\n\n[[sample]]\nname = \"two\"\nfiles = \"copy.root\"", "name = \"two\"\nfiles = \"copy.root\"\n\n[[sample]]\nname = \"one\"\nfiles = \"runs.root\"");
    assert_ne!(reordered, SAMPLES);
    let a = pressed(&root, "rows-a", SAMPLES, "2", "sum(Jet_pt)");
    let same = pressed(&root, "rows-same", &reordered, "2", "sum(Jet_pt)");
    // The same events, the samples in another order.
    let (code, said) = diff(&root, &a, &same, &[]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("sample one:") && said.contains("0 only in A, 0 only in B") && !said.contains("first difference"), "{said}");
    // A looser cut: the events of nJet 2 only A holds; B's every one A holds too.
    let tight = pressed(&root, "rows-tight", SAMPLES, "3", "sum(Jet_pt)");
    let (code, said) = diff(&root, &a, &tight, &[]);
    assert_eq!(code, 1, "{said}");
    let rows = |file: &Path, sample: usize| {
        let (_, h) = rvr(&root, &["header", file.to_str().unwrap()]);
        serde_json::from_str::<serde_json::Value>(&h).unwrap()["data"]["samples"][sample]["rows"].as_u64().unwrap()
    };
    let extra = rows(&a, 0) - rows(&tight, 0);
    let grouped = |n: u64| rowver::rvr::grouped(n);
    assert!(extra > 0 && said.contains(&format!("sample one: {} events in both, {} only in A, 0 only in B", grouped(rows(&tight, 0)), grouped(extra))), "{said}");
    // --sample compares that sample alone.
    let (_, said) = diff(&root, &a, &tight, &["--sample", "two"]);
    assert!(said.contains("sample two:") && !said.contains("sample one:"), "{said}");
    // A sample neither holds is refused, naming those they hold.
    let (code, said) = diff(&root, &a, &tight, &["--sample", "three"]);
    assert!(code == 2 && said.contains("holds a sample three: the samples they hold are one, two"), "{said}");
}

// SPEC 9.4.1
#[test]
fn the_first_differing_value_is_named_with_how_many_events_differ() {
    let root = Study::new("diff-values");
    let a = pressed(&root, "values-a", SAMPLES, "1", "sum(Jet_pt)");
    let b = pressed(&root, "values-b", SAMPLES, "1", "if(nJet == 5, sum(Jet_pt) + 1, sum(Jet_pt))");
    let (code, said) = diff(&root, &a, &b, &[]);
    assert_eq!(code, 1, "{said}");
    // The first event of five jets differs in ht, and in ratio, a derived column of it.
    assert!(said.contains("0 only in A, 0 only in B") && said.contains("first difference: entry "), "{said}");
    assert!(said.contains(", column ht: ") && said.contains("events differ"), "{said}");
    // One event of each sample differs, in the singular.
    let one = pressed(&root, "values-one", SAMPLES, "1", "if(event == 375462394065, sum(Jet_pt) + 1, sum(Jet_pt))");
    let (code, said) = diff(&root, &a, &one, &[]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("; 1 event differs") && !said.contains(" 1 events"), "{said}");
    // Two values whose words are alike (NaNs of two signs) are told apart by their bits.
    let nan = pressed(&root, "values-nan", SAMPLES, "1", "if(nJet == 5, inf - inf, sum(Jet_pt))");
    let negated = pressed(&root, "values-negated", SAMPLES, "1", "if(nJet == 5, -(inf - inf), sum(Jet_pt))");
    let (code, said) = diff(&root, &nan, &negated, &[]);
    assert_eq!(code, 1, "{said}");
    let bits: Vec<&str> = said.match_indices("NaN (bits 0x").map(|(at, _)| said[at..].split(')').next().unwrap()).collect();
    assert!(said.contains(", column ht: NaN (bits 0x") && bits.len() == 4 && bits[0] != bits[1], "{said}");
}

// SPEC 9.4.1
#[test]
fn a_key_of_stored_columns_matches_events_and_two_rows_of_one_key_are_refused() {
    let root = Study::new("diff-key");
    let a = pressed(&root, "key-a", SAMPLES, "1", "sum(Jet_pt)");
    let tight = pressed(&root, "key-tight", SAMPLES, "4", "sum(Jet_pt)");
    let (code, said) = diff(&root, &tight, &a, &["--key", "run,event"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(" 0 only in A, ") && !said.contains(", 0 only in B"), "{said}");
    let (code, said) = diff(&root, &a, &tight, &["--key", "run"]);
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("sample one of ") && said.contains("holds two events of one key (run "), "{said}");
    let (code, said) = diff(&root, &a, &tight, &["--key", "nowhere"]);
    assert!(code == 2 && said.contains("nowhere"), "{said}");
    let (code, said) = diff(&root, &a, &tight, &["--key", "Jet_pt"]);
    assert!(code == 2 && said.contains("Jet_pt"), "{said}");
    // An empty name, and a derived column, are refused as such.
    for empty in ["run,,event", "run,"] {
        let (code, said) = diff(&root, &a, &tight, &["--key", empty]);
        assert!(code == 2 && said.contains("names an empty column"), "{said}");
    }
    let (code, said) = diff(&root, &a, &tight, &["--key", "ratio"]);
    assert!(code == 2 && said.contains("ratio is a derived column of") && said.contains("a key is stored columns"), "{said}");
}

// SPEC 9.4.1
#[test]
fn copies_of_one_file_in_a_sample_are_told_apart_and_columns_of_two_kinds_are_named_not_compared() {
    let root = Study::new("diff-copies");
    let both = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"Events\"\n\n[eras]\n\n[[sample]]\nname = \"one\"\nfiles = [\"runs.root\", \"copy.root\"]\n";
    let a = pressed(&root, "copies-a", both, "1", "sum(Jet_pt)");
    let same = pressed(&root, "copies-same", both, "1", "sum(Jet_pt)");
    // The second copy's events match the second copy's, not the first's (one UUID, two files).
    let (code, said) = diff(&root, &a, &same, &[]);
    assert!(code == 0 && said.contains("0 only in A, 0 only in B") && said.contains("The two hold the same events with the same values."), "{said}");
    let wide = pressed(&root, "copies-wide", both, "1", "stored(sum(Jet_pt), 'f8')");
    let (code, said) = diff(&root, &a, &wide, &[]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("column ht is f4 in A and f8 in B: not compared") && !said.contains("first difference"), "{said}");
}
