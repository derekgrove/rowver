//! The recipe part of the conformance corpus (tests/conformance/recipe): recipe texts the
//! validator accepts, with what they name, and texts it refuses, each breaking one rule; the
//! expressions with their arrays are the two fixtures of tests/fixtures, which the manifest names.

#[allow(dead_code)]
#[path = "../../rowver/tests/common/corpus.rs"]
mod corpus;

use rvr_recipe::Recipe;
use serde_json::json;

const TABLES: &str = "[define]\n\n[keep]\n\n[columns]\nbranches = [\"MET_pt\"]\n\n[totals]\n\n[derived]\n";

/// A recipe of the header, then `body`, then the five tables (those `body` writes left out).
fn recipe(body: &str) -> String {
    let mut text = format!("format = \"rvr-recipe\"\nversion = 1\n{body}");
    for table in ["define", "keep", "columns", "totals", "derived"] {
        if !body.lines().any(|line| line.trim() == format!("[{table}]")) {
            let full = TABLES.split("\n\n").find(|t| t.starts_with(&format!("[{table}]"))).unwrap();
            text += &format!("\n{full}\n");
        }
    }
    text
}

/// Each rule (the README's section and a few words), and a recipe that breaks it alone.
fn refusals() -> Vec<(&'static str, String)> {
    vec![
        ("the-keys-exactly: format required", recipe("").replacen("format = \"rvr-recipe\"\n", "", 1)),
        ("the-keys-exactly: format is rvr-recipe", recipe("").replacen("rvr-recipe", "rvr-recipes", 1)),
        ("the-keys-exactly: version is 1", recipe("").replacen("version = 1", "version = 2", 1)),
        ("the-keys-exactly: no other top key", recipe("select = \"x\"\n")),
        ("the-keys-exactly: title is text", recipe("title = 3\n")),
        ("the-keys-exactly: every table written", recipe("").replace("\n[totals]\n", "\n")),
        ("the-keys-exactly: [columns] stores a column", recipe("[columns]\n")),
        ("the-keys-exactly: a cut has apply", recipe("[keep]\nmet = { if = \"MET_pt > 0\" }\n")),
        ("the-keys-exactly: a cut has no other key", recipe("[keep]\nmet = { apply = \"MET_pt > 0\", when = \"x\" }\n")),
        ("the-keys-exactly: stored is a kind", recipe("[columns]\nx = { value = \"MET_pt\", stored = \"f2\" }\n")),
        ("the-keys-exactly: a total's tree is nonempty", recipe("[totals]\nt = { value = \"MET_pt\", tree = \"\" }\n")),
        ("the-keys-exactly: a plain name", recipe("[columns]\n\"two words\" = \"MET_pt\"\n")),
        ("the-keys-exactly: no word of the language", recipe("[columns]\nand = \"MET_pt\"\n")),
        ("the-keys-exactly: no column named entry", recipe("[columns]\nentry = \"MET_pt\"\n")),
        ("the-keys-exactly: no total named events_read", recipe("[totals]\nevents_read = \"MET_pt\"\n")),
        ("the-keys-exactly: no two columns alike", recipe("[columns]\nbranches = [\"MET_pt\", \"MET_pt\"]\n")),
        ("the-keys-exactly: a defined name uses those above it", recipe("[define]\na = \"b * 2\"\nb = \"MET_pt\"\n")),
        ("mistakes: syntax", recipe("[columns]\nx = \"MET_pt >\"\n")),
        ("mistakes: a function's values", recipe("[columns]\nx = \"count(MET_pt, 3)\"\n")),
        ("mistakes: a function that does not exist", recipe("[columns]\nx = \"cuont(MET_pt)\"\n")),
        ("mistakes: a dot after a name", recipe("[columns]\nx = \"Jet.pt\"\n")),
        ("mistakes: not of a number, as far as the text tells", recipe("[keep]\nc = \"not 3\"\n")),
        ("mistakes: and of numbers, as far as the text tells", recipe("[columns]\nx = \"count(MET_pt > 1) and 2\"\n")),
        ("operators: comparisons do not chain", recipe("[keep]\nc = \"1 < MET_pt < 2\"\n")),
        ("how-deep: 100 levels at most", recipe(&format!("[columns]\nx = \"{}MET_pt{}\"\n", "(".repeat(100), ")".repeat(100)))),
        ("the-parts-of-a-recipe: a derived column uses only stored columns and those above it", recipe("[derived]\na = \"b * 2\"\nb = \"MET_pt\"\n")),
    ]
}

// RECIPE the-keys-exactly, the-parts-of-a-recipe, mistakes, how-deep; SPEC 4.1.5
#[test]
fn the_conformance_corpus_holds_what_this_validator_says() {
    let mut cases = Vec::new();
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
    let mut studies: Vec<_> = std::fs::read_dir(&examples).unwrap().flatten().map(|e| e.path().join("recipe.toml")).filter(|p| p.exists()).collect();
    studies.sort();
    let mut valid: Vec<(String, String)> = studies.iter().map(|p| (format!("the example {}", p.parent().unwrap().file_name().unwrap().to_string_lossy()), std::fs::read_to_string(p).unwrap())).collect();
    valid.push(("every table, totals over another tree, stored kinds, a cut for some samples".into(), recipe("title = \"t\"\n[define]\ngood = \"Jet_pt > 30\"\n[keep]\nhem = { if = \"sample.era == 3\", apply = \"count(good) >= 1\" }\n[columns]\nbranches = [\"MET_pt\"]\nw = { value = \"genWeight\", stored = \"f4\" }\n[totals]\nsumw = \"genWeight\"\nruns = { value = \"genEventSumw\", tree = \"Runs\" }\n[derived]\nr = \"w / sample.sumw\"\n")));
    for (rule, text) in valid {
        let read = Recipe::read(&text).unwrap_or_else(|e| panic!("{rule}: {e}"));
        let named = json!({
            "title": read.title,
            "defines": read.depths.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "keep": read.keep.iter().map(|c| &c.name).collect::<Vec<_>>(),
            "columns": read.columns.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "stored": read.stored.iter().map(|(n, k)| (n.clone(), json!(format!("{k:?}").to_lowercase()))).collect::<serde_json::Map<_, _>>(),
            "totals": read.totals.iter().map(|t| json!({"name": t.name, "tree": t.tree})).collect::<Vec<_>>(),
            "derived": read.derived.iter().map(|(n, e)| json!({"name": n, "expression": e})).collect::<Vec<_>>(),
        });
        cases.push(json!({"rule": rule, "text": text, "verdict": "valid", "names": named}));
    }
    for (rule, text) in refusals() {
        let said = Recipe::read(&text).err().unwrap_or_else(|| panic!("accepted: {rule}")).0;
        cases.push(json!({"rule": format!("RECIPE {rule}"), "text": text, "verdict": "refused", "rust_says": said.lines().collect::<Vec<_>>()}));
    }
    let manifest = json!({
        "about": "recipe.toml texts, each valid (with what it names) or refused with the rule it breaks; expressions and their arrays are crates/rvr-recipe/tests/fixtures/recipe.json and properties.json; see README.md",
        "expressions": ["crates/rvr-recipe/tests/fixtures/recipe.json", "crates/rvr-recipe/tests/fixtures/properties.json"],
        "cases": cases,
    });
    corpus::corpus("recipe", &[], &manifest);
}
