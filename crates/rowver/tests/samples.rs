//! The samples validator (SPEC §3) rule by rule: each text refused by its own rule, and the
//! values each accepted text makes.

mod common;

use rowver::study::samples::read;
use serde_json::json;

const HEAD: &str = "format = \"rvr-samples\"\nversion = 1\n[places]\n[defaults]\n[eras]\n";
const ONE: &str = "[[sample]]\nname = \"a\"\nfiles = \"x.root\"\ntree = \"T\"\n";

/// A text with `places`, `defaults` and `eras` written into their tables, then `samples`.
fn text(places: &str, defaults: &str, eras: &str, samples: &str) -> String {
    format!("format = \"rvr-samples\"\nversion = 1\n[places]\n{places}\n[defaults]\n{defaults}\n[eras]\n{eras}\n{samples}")
}

/// What the validator says of `text`, which it must refuse.
fn refused(text: &str) -> String {
    match read(text) {
        Ok(_) => panic!("accepted:\n{text}"),
        Err(problems) => problems.to_string(),
    }
}

// SPEC 3.1.1, 3.1.2, 3.1.5, 3.2.1, 3.2.2, 3.3.1, 3.4.1, 3.4.2, 3.5.2, 3.5.3, 3.5.4, 3.5.5, 3.5.6, 3.5.8, 3.5.9, 3.5.10, 3.6.2, 3.6.4, 3.7.4, 3.8.5, 1.5
#[test]
fn each_rule_refuses_a_text_that_breaks_it_alone() {
    let cases = refusals();
    let mut wrong = Vec::new();
    for (rule, text, says) in &cases {
        let said = refused(text);
        if !said.contains(says) {
            wrong.push(format!("{rule}: {said}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // The forms of 3.8.5 are named, all three.
    let forms = refused(&sample("files = \"file:///d/x.root\""));
    assert!(forms.contains("davs://") && forms.contains("https://"), "{forms}");
}

/// A sample `a` of tree T, with `body`.
fn sample(body: &str) -> String {
    format!("{HEAD}[[sample]]\nname = \"a\"\ntree = \"T\"\n{body}\n")
}

/// Each rule (SPEC's number and a few words), a text that breaks it alone, and what its refusal says.
fn refusals() -> Vec<(&'static str, String, &'static str)> {
    vec![
        ("3.1.1 TOML", format!("{HEAD}{ONE}name = "), "TOML"),
        ("3.1.2 top level", format!("sampel = 1\n{HEAD}{ONE}"), "Did you mean: sample"),
        ("3.1.5 one sample", HEAD.to_owned(), "[[sample]]"),
        ("3.2.1 place era", text("era = \"/d\"", "", "", ONE), "era"),
        ("3.2.1 place characters", text("\"a-b\" = \"/d\"", "", "", ONE), "a-b"),
        ("3.2.2 place text", text("p = 3", "", "", ONE), "text"),
        ("3.3.1 defaults name", text("", "name = \"a\"", "", ONE), "name"),
        ("3.3.1 defaults files", text("", "files = \"x\"", "", ONE), "files"),
        ("3.3.1 defaults trees", text("", "trees = { a = \"T\" }", "", ONE), "trees"),
        ("3.4.1 era name", text("", "", "[eras.\"UL 18\"]\nera = 3", ONE), "UL 18"),
        ("3.4.2 era value", text("", "", "[eras.UL18]\nwhen = 2026-11-02", ONE), "date or time"),
        ("3.5.2 name", sample("files = \"x\"\nname = \"a b\"").replacen("name = \"a\"\n", "", 1), "label"),
        ("3.5.3 label", sample("files = \"x\"\nlabel = \"\""), "label"),
        ("3.5.4 files empty", sample("files = []"), "files"),
        ("3.5.4 files text", sample("files = 3"), "files"),
        ("3.5.5 trees empty", sample("files = \"x\"\ntrees = {}").replace("tree = \"T\"\n", ""), "trees"),
        ("3.5.6 eras empty", sample("files = \"x\"\neras = []"), "eras"),
        ("3.5.6 era unknown", sample("files = \"x\"\neras = [\"UL19\"]"), "UL19"),
        ("3.5.8 random", sample("files = \"x\"\nchoose = \"random\""), "events"),
        ("3.5.9 seed", sample("files = \"x\"\nevents = 5\nchoose = \"random\"\nseed = -1"), "seed"),
        ("3.5.10 key start", sample("files = \"x\"\nmetadata = { \"1a\" = 1 }"), "1a"),
        ("3.5.10 key sample", sample("files = \"x\"\nmetadata = { sample = 1 }"), "sample"),
        ("3.5.10 key file", sample("files = \"x\"\nmetadata = { file = 1 }"), "file"),
        ("3.5.10 key word", sample("files = \"x\"\nmetadata = { and = 1 }"), "and"),
        ("3.5.10 events_total", sample("files = \"x\"\nmetadata = { events_total = 1 }"), "events_total"),
        ("3.5.10 events_missing", sample("files = \"x\"\nmetadata = { events_missing = 1 }"), "events_missing"),
        ("3.5.10 nan", sample("files = \"x\"\nmetadata = { x = nan }"), "finite"),
        ("3.5.10 inf", sample("files = \"x\"\nmetadata = { x = [1.0, -inf] }"), "finite"),
        ("3.6.2 era float", text("", "", "[eras.E]\nv = 2.5", "[[sample]]\nname = \"a\"\nfiles = \"{v}/x\"\ntree = \"T\"\neras = [\"E\"]"), "v"),
        ("3.6.4 unclosed", sample("files = \"{x.root\""), "{"),
        ("3.7.4 label twice", format!("{HEAD}{ONE}label = \"L\"\n[[sample]]\nname = \"b\"\nfiles = \"y.root\"\ntree = \"T\"\nlabel = \"L\"\n"), "label"),
        ("3.8.5 forms", sample("files = \"file:///d/x.root\""), "root://"),
        ("3.8.1 root:// with one slash", sample("files = \"root://h.org/store/x.root\""), "write it \"root://h.org//store/x.root\""),
        ("3.8.1 root:// with a port and one slash", sample("files = \"root://h.org:1094/store/x.root\""), "with two slashes before the path: write it \"root://h.org:1094//store/x.root\""),
        ("3.8.1 root:// with no path", sample("files = \"root://h.org\""), "write it \"root://h.org//path/file.root\""),
        ("3.8.1 a folder", sample("files = [\"x.root\", \"data/\"]"), "\"data/\" names a folder: name its files, or a pattern of them, such as \"data/*.root\""),
        ("3.8.1 a folder of storage", sample("files = \"https://h.org/store/\""), "such as \"https://h.org/store/*.root\""),
        ("3.8.1 the top folder of grid storage", sample("files = \"root://h.org//\""), "such as \"root://h.org//*.root\""),
        ("3.8.1 the folder itself", sample("files = \"data/..\""), "such as \"data/../*.root\""),
        ("3.8.1 the home folder", sample("files = \"~\""), "\"~\" names a folder"),
        ("3.8.1 the home folder at the end", sample("files = \"data/~\""), "\"data/~\" names a folder"),
    ]
}

// SPEC 3.2.2, 3.3.2, 3.5.3, 3.5.5, 3.6.2, 3.8.1
#[test]
fn accepted_texts_make_the_values_the_rules_give() {
    let made = read(&text("p = \"/data/\"", "tree = \"T\"", "[eras.E]\nn = 7", "[[sample]]\nname = \"a\"\nfiles = [\"{p}/{n}.root\", \"~/b.root\", \"c/d.root\", \"root://h.org//e.root\", \"davs://h.org:443/f.root\", \"https://h.org/g.root\"]\neras = [\"E\"]\ntrees = { p1 = \"T1\" }\n")).unwrap();
    // One trailing / of a place dropped; a whole-number era value put in text as its digits.
    assert_eq!(made[0].files[0], "/data/7.root");
    assert_eq!(&made[0].files[1..], ["~/b.root", "c/d.root", "root://h.org//e.root", "davs://h.org:443/f.root", "https://h.org/g.root"]);
    // A sample's own trees replaces a default tree; the label is the name when left out.
    assert_eq!((made[0].tree.as_str(), made[0].name.as_str(), made[0].label.as_str()), ("T1", "a_p1_E", "a p1 (E)"));
    let plain = read(&format!("{HEAD}{ONE}")).unwrap();
    assert_eq!(plain[0].label, "a");
}

/// Texts samples.toml may be, each with the samples it makes.
fn accepted() -> Vec<(&'static str, String)> {
    vec![
        ("one sample", format!("{HEAD}{ONE}")),
        ("places, defaults, an era, trees and every form of files", text("p = \"/data/\"", "tree = \"T\"", "[eras.E]\nn = 7", "[[sample]]\nname = \"a\"\nfiles = [\"{p}/{n}.root\", \"~/b.root\", \"c/d.root\", \"root://h.org//e.root\", \"davs://h.org:443/f.root\", \"https://h.org/g.root\"]\neras = [\"E\"]\ntrees = { p1 = \"T1\" }\n")),
        ("eras and mass points named and labelled", text("lpc = \"root://cmseos.fnal.gov//store/x/\"", "metadata = { lumi = 59.8 }", "[eras.UL17]\nera = 2\n[eras.UL18]\nera = 3", "[[sample]]\nname = \"Signal\"\nfiles = \"{lpc}/{era}/s_{era}.root\"\neras = [\"UL17\", \"UL18\"]\ntrees = { \"300/292.5\" = \"SMS_300\", \"400/300\" = \"SMS_400\" }\nmetadata = { kind = \"signal\", xsec = 2 }\n")),
        ("choices: first N, random with a seed, a default seed", text("", "tree = \"T\"\nevents = 10\nchoose = \"random\"\nseed = 7", "", "[[sample]]\nname = \"a\"\nfiles = \"x.root\"\n[[sample]]\nname = \"b\"\nfiles = \"y.root\"\nchoose = \"first\"\n[[sample]]\nname = \"c\"\nfiles = \"z.root\"\nevents = \"all\"\nchoose = \"first\"\n")),
    ]
}

// SPEC 3.1.1, 3.7.1, 3.7.2, 3.7.3
#[test]
fn the_conformance_corpus_holds_what_this_validator_says() {
    let mut cases = Vec::new();
    for (rule, text) in accepted() {
        let samples: Vec<serde_json::Value> = read(&text).unwrap().iter().map(|s| json!({"name": s.name, "label": s.label, "tree": s.tree, "files": s.files, "events": s.events.record(), "metadata": s.metadata})).collect();
        cases.push(json!({"rule": rule, "text": text, "verdict": "valid", "samples": samples}));
    }
    for (rule, text, _) in refusals() {
        cases.push(json!({"rule": format!("SPEC {rule}"), "text": text, "verdict": "refused", "rust_says": refused(&text)}));
    }
    let manifest = json!({"about": "samples.toml texts: each valid one with the samples it makes (SPEC §3.7), each refused one with the rule it breaks; see README.md", "cases": cases});
    common::corpus::corpus("samples", &[], &manifest);
}
