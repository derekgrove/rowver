//! The lock validator (FORMAT §6.3) on texts that break one rule each.

mod common;

use rowver::study::lock::Lock;
use serde_json::json;
use sha2::{Digest, Sha256};

const LIST: &str = r#"[{"kind":"none","name":"a","shape":"flat","why":"a TObject"},{"kind":"i4","name":"nJet","shape":"flat"}]"#;
const LISTED: &str = r#"  { name = "a", kind = "none", shape = "flat", why = "a TObject" },
  { name = "nJet", kind = "i4", shape = "flat" },"#;

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A valid lock: one sample choosing 3 of its file's 5 entries at random, the list of branches given.
fn lock(listed: &str, list: &str) -> String {
    let sha = sha(list);
    format!(r#"format = "rvr-samples-lock"
version = 1
resolved = "2026-11-04T16:06:01Z"
samples_toml_sha256 = "{}"

[[sample]]
name = "flat"
label = "flat"
asked = ["a.root"]
tree = "analysis"
branches = "{sha}"
events = 3
choose = "random"
seed = 4
events_total = 5
events_read = 3
metadata = {{}}

[[sample.file]]
at = "./a.root"
bytes = 39262
adler32 = "bbc43743"
uuid = "31e851e4-b994-11f1-ba5c-1a45bb13c76a"
entries = 5
take = 3

[[branches]]
sha256 = "{sha}"
list = [
{listed}
]
"#, "0".repeat(64))
}

fn refused(text: &str) -> String {
    match Lock::read(text) {
        Ok(_) => panic!("accepted:\n{text}"),
        Err(problems) => problems.to_string(),
    }
}

// FORMAT 6.3
#[test]
fn each_rule_of_the_lock_refuses_a_text_that_breaks_it_alone() {
    let good = lock(LISTED, LIST);
    Lock::read(&good).unwrap_or_else(|e| panic!("{e}"));
    let cases = refusals();
    let mut wrong = Vec::new();
    for (rule, text, says) in &cases {
        let said = refused(text);
        if !said.contains(says) {
            wrong.push(format!("{rule}: {said}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Each rule, a lock that breaks it alone, and what its refusal says.
fn refusals() -> Vec<(&'static str, String, &'static str)> {
    let good = lock(LISTED, LIST);
    let no_why = (LISTED.replace(", why = \"a TObject\"", ""), LIST.replace(r#","why":"a TObject""#, ""));
    let why_too = (LISTED.replace("shape = \"flat\" },", "shape = \"flat\", why = \"x\" },"), LIST.replace(r#""shape":"flat"}]"#, r#""shape":"flat","why":"x"}]"#));
    let cases: [(&'static str, String, &'static str); 7] = [
        ("why missing on a branch rvr cannot read", lock(&no_why.0, &no_why.1), "why"),
        ("why on a branch rvr reads", lock(&why_too.0, &why_too.1), "why"),
        ("a list no sample has", format!("{good}\n[[branches]]\nsha256 = \"{}\"\nlist = [{{ name = \"x\", kind = \"f4\", shape = \"flat\" }}]\n", sha(r#"[{"kind":"f4","name":"x","shape":"flat"}]"#)), "no sample"),
        ("no take for a random choice", good.replace("take = 3\n", ""), "take"),
        ("choose for a choice not random", good.replace("choose = \"random\"\nseed = 4\n", "choose = \"first\"\n").replace("take = 3\n", "stop = 3\n"), "choose"),
        ("no branch list at all", good[..good.find("[[branches]]").unwrap()].to_owned(), "branches"),
        ("no metadata table holds the key the JSON reader takes for a number", good.replace("metadata = {}", "metadata = { x = { \"$serde_json::private::Number\" = \"12\" } }"), "is a key no .rvr header holds"),
    ];
    cases.into_iter().collect()
}

// FORMAT 6.3
#[test]
fn the_conformance_corpus_holds_what_this_validator_says() {
    let good = lock(LISTED, LIST);
    let mut cases = vec![json!({"rule": "a random choice of 3 of 5, a branch rvr cannot read", "text": good, "verdict": "valid"})];
    for (rule, text, _) in refusals() {
        cases.push(json!({"rule": format!("FORMAT §6.3 {rule}"), "text": text, "verdict": "refused", "rust_says": refused(&text)}));
    }
    let manifest = json!({"about": "samples.lock texts, each valid or refused with the rule it breaks (FORMAT.md §6.3); see README.md", "cases": cases});
    common::corpus::corpus("lock", &[], &manifest);
}
