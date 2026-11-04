//! Every expression in the README's tables, evaluated on its three example events, gives the
//! result the README says, and every mistake it lists is refused with the words it shows.

use rvr_recipe::{parse, Array, Constant, Kind, Recipe, Scope, Store, Values};
use serde_json::Value;
use std::collections::BTreeMap;

mod common;
use common::Held as Events;

fn values(kind: &str, numbers: &[f64]) -> Values {
    let kind = Kind::named(kind).unwrap();
    match kind.float() {
        // Held as a file holds them: an f4 column's values are 32-bit floats.
        true => Values::float(kind, numbers.iter().map(|n| kind.round(*n)).collect()),
        false => Values::whole(kind, numbers.iter().map(|&n| n as i64).collect()),
    }
}

fn flat(kind: &str, numbers: [f64; 3]) -> Array {
    Array::Event(values(kind, &numbers))
}

fn jagged(kind: &str, lists: [&[f64]; 3]) -> Array {
    let counts: Vec<usize> = lists.iter().map(|list| list.len()).collect();
    Array::object(&counts, values(kind, &lists.concat())).unwrap()
}

/// The table "The example events", with the kinds NanoAOD stores them as.
fn events() -> Events {
    let jet = |kind, lists| jagged(kind, lists);
    Events(BTreeMap::from([
        ("Jet_pt".into(), jet("f4", [&[65.0, 22.0, 41.0], &[], &[120.0, 35.0]])),
        ("Jet_eta".into(), jet("f4", [&[0.5, -2.1, 1.2], &[], &[-0.2, 0.9]])),
        ("Jet_phi".into(), jet("f4", [&[0.1, 2.9, -1.0], &[], &[3.0, -3.0]])),
        ("Jet_mass".into(), jet("f4", [&[8.0, 4.0, 6.0], &[], &[12.0, 5.0]])),
        ("nJet".into(), flat("u4", [3.0, 0.0, 2.0])),
        ("Muon_pt".into(), jet("f4", [&[44.0, 12.0], &[25.0], &[]])),
        ("Muon_eta".into(), jet("f4", [&[0.4, 1.9], &[-0.3], &[]])),
        ("Muon_phi".into(), jet("f4", [&[0.2, -2.8], &[1.5], &[]])),
        ("Muon_tightId".into(), jet("b1", [&[1.0, 0.0], &[1.0], &[]])),
        ("Muon_charge".into(), jet("i4", [&[1.0, -1.0], &[-1.0], &[]])),
        ("Electron_pt".into(), jet("f4", [&[], &[30.0], &[50.0, 9.0]])),
        ("Electron_eta".into(), jet("f4", [&[], &[1.1], &[-0.6, 2.0]])),
        ("Electron_phi".into(), jet("f4", [&[], &[-1.2], &[0.4, -2.2]])),
        ("Electron_charge".into(), jet("i4", [&[], &[1.0], &[-1.0, -1.0]])),
        ("MET_pt".into(), flat("f4", [31.5, 85.0, 16.0])),
        ("MET_phi".into(), flat("f4", [-0.5, 2.0, 1.0])),
        ("Flags".into(), flat("i4", [11.0, 4.0, 1.0])),
    ]))
}

/// The names the README's examples define.
const RECIPE: &str = r#"
format = "rvr-recipe"
version = 1

[define]
good_jet = "Jet_pt > 30 and abs(Jet_eta) < 2.4"
good_muon = "Muon_tightId and Muon_pt > 20"
hard_central = "good_jet and Jet_pt > 100"
clean_jet = "Jet_pt > 30 and dr_nearest(Jet_eta, Jet_phi, Muon_eta[good_muon], Muon_phi[good_muon]) > 0.4"
hem_veto = "count(Jet_eta < -1.3 and Jet_phi > 2.5) == 0"

[keep]

[columns]
branches = ["MET_pt"]

[totals]

[derived]
"#;

/// The README's tables: for each row, its cells by the header above them.
fn tables(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows = Vec::new();
    let mut header: Option<Vec<String>> = None;
    for line in text.lines() {
        if !line.starts_with('|') {
            header = None;
            continue;
        }
        let cells: Vec<String> = line.trim_matches('|').split('|').map(|c| c.trim().to_owned()).collect();
        match &header {
            None => header = Some(cells),
            Some(_) if cells.iter().all(|c| c.chars().all(|ch| ch == '-')) => {}
            Some(names) => rows.push(names.iter().cloned().zip(cells).collect()),
        }
    }
    rows
}

/// The first `code span` of a cell.
fn code(cell: &str) -> Option<&str> {
    let start = cell.find('`')? + 1;
    Some(&cell[start..start + cell[start..].find('`')?])
}

/// A result as JSON, as the README writes it.
fn json(array: &Array) -> Value {
    let one = |values: &Values, at: usize| match &values.store {
        Store::Whole(held) if values.kind == Kind::B1 => Value::Bool(held[at] != 0),
        Store::Whole(held) => Value::from(held[at]),
        // Infinities and NaN as the README writes them; JSON has no number for them.
        Store::Float(held) if held[at].is_nan() => Value::from("nan"),
        Store::Float(held) if held[at].is_infinite() => Value::from(if held[at] > 0.0 { "inf" } else { "-inf" }),
        Store::Float(held) => Value::from(held[at]),
    };
    match array {
        Array::Constant(Constant::Whole(n)) => Value::from(*n),
        Array::Constant(Constant::Float(x)) => one(&Values::float(Kind::F8, vec![*x]), 0),
        Array::Constant(Constant::Truth(t)) => Value::Bool(*t),
        Array::Constant(Constant::Text(t)) => Value::from(t.clone()),
        Array::Constant(Constant::Missing(_)) => Value::from("missing"),
        Array::Event(values) => (0..values.len()).map(|at| one(values, at)).collect(),
        Array::Object { offsets, values } => offsets
            .windows(2)
            .map(|w| Value::Array((w[0]..w[1]).map(|at| one(values, at)).collect()))
            .collect(),
    }
}

/// Whether a result is what the README shows, to the seven or so digits it writes.
fn same(ours: &Value, shown: &Value) -> bool {
    match (ours, shown) {
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b)),
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            (a - b).abs() <= 2e-6 * a.abs().max(1.0)
        }
        (a, b) => a == b,
    }
}

/// What the README writes, read as JSON: `inf`, `-inf` and `nan` are text, and a cell with no
/// code is plain words.
fn shown(cell: &str) -> Value {
    match code(cell) {
        Some(text) => {
            let quoted = text.replace("-inf", "\"-INF\"").replace("inf", "\"inf\"").replace("-INF", "-inf").replace("nan", "\"nan\"");
            serde_json::from_str(&quoted).unwrap_or_else(|_| Value::from(text))
        }
        None => Value::from(cell),
    }
}

// RECIPE functions, mistakes; SPEC 1.4
#[test]
fn every_function_is_known_and_in_the_readme() {
    let text = std::fs::read_to_string(format!("{}/README.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let recipe = Recipe::read(RECIPE).unwrap();
    let (events, sample) = (events(), BTreeMap::new());
    let scope = Scope::new(&events, &sample, &recipe.defines);
    // Each is called with the fewest values it takes, so that every name listed is one the
    // evaluator knows (it has no other: a name not listed is refused before it is looked up).
    for (name, least, _) in rvr_recipe::FUNCTIONS {
        let arguments = vec!["MET_pt"; *least].join(", ");
        let said = scope.evaluate(&parse(&format!("{name}({arguments})")).unwrap());
        assert!(!matches!(&said, Err(e) if e.0.contains("no function named") || e.0.contains(" takes ")), "{name}: {said:?}");
        assert!(text.contains(&format!("`{name}(")) || text.contains(&format!("`{name}`")), "{name} is not in the README");
    }
    let said = scope.evaluate(&parse("cnt(MET_pt)").unwrap()).unwrap_err();
    assert!(said.0.starts_with("There is no function named cnt. Did you mean: ") && said.0.contains("count"), "{said:?}");
    assert!(said.0.ends_with(&format!("? Every function, with an example, is in the recipe reference: {}", rvr_recipe::REFERENCE)), "{said:?}");
}

// RECIPE the-example-events, values, operators, choosing-members-of-a-list, functions, reducing-a-jagged-list-to-a-flat-value, positions-and-order, joining-lists, math-value-by-value, bits, angles-and-kinematics, pairs-within-a-list, text-branches-and-stored-types, defined-names, mistakes, the-keys-exactly, from-python-to-rvr, longer-ones, picking-the-jets-you-want, more-than-one-kind-of-particle, kinds-exactly, two-kinds-together, fixed-values-beside-a-column, numbers, conversions, what-differs-from-numpy
#[test]
fn every_readme_example_gives_the_result_it_shows() {
    let text = std::fs::read_to_string(format!("{}/README.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let recipe = Recipe::read(RECIPE).unwrap();
    let sample = BTreeMap::from([
        ("era".to_owned(), Constant::Whole(3)),
        ("kind".to_owned(), Constant::Text("signal".into())),
        ("xsec".to_owned(), Constant::Float(0.386)),
        ("rho".to_owned(), Constant::Text("MET_pt".into())),
    ]);
    let events = events();
    let scope = Scope::new(&events, &sample, &recipe.defines);
    let (mut checked, mut wrong) = (0, Vec::new());
    for row in tables(&text) {
        let said = row.get("`rvr` says");
        let expression = ["rvr", "expression", "example"].iter().find_map(|h| row.get(*h).and_then(|c| code(c)));
        let Some(expression) = expression else { continue };
        if said.is_none() && !row.contains_key("result") {
            continue;
        }
        // A line of a recipe (`select = "…"`) is read as one; anything else is an expression.
        let result = match expression.split_once(" = ") {
            Some(_) => Recipe::read(&common::complete(expression)).map(|_| Array::Constant(Constant::Missing(String::new()))),
            None => parse(expression).and_then(|e| scope.evaluate(&e)),
        };
        checked += 1;
        // A row that says the result's kind is checked for it too.
        let kind = |array: &Array| match array {
            Array::Event(values) | Array::Object { values, .. } => values.kind.name(),
            Array::Constant(_) => "",
        };
        if let (Some(want), Ok(ours)) = (row.get("kind").and_then(|cell| code(cell)), &result) {
            if kind(ours) != want {
                wrong.push(format!("{expression}: README says kind {want}, rvr gives {}", kind(ours)));
            }
        }
        match (said, result) {
            (Some(said), Err(error)) if error.0.contains(said.as_str()) => {}
            (Some(said), other) => wrong.push(format!("{expression}: expected the mistake {said:?}, got {other:?}")),
            (None, Ok(ours)) if same(&json(&ours), &shown(&row["result"])) => {}
            (None, Ok(ours)) => wrong.push(format!("{expression}: README shows {}, rvr gives {}", row["result"], json(&ours))),
            (None, Err(error)) => wrong.push(format!("{expression}: README shows {}, rvr fails: {}", row["result"], error.0)),
        }
    }
    // The longer examples: a ```toml line `name = "…"`, then the line "Both give `…`" after it.
    let mut last = None;
    for line in text.lines() {
        if let Some((_, quoted)) = line.split_once(" = \"").filter(|_| !line.starts_with('|')) {
            last = quoted.strip_suffix('"').map(str::to_owned);
        } else if let (Some(expression), Some(rest)) = (&last, line.strip_prefix("Both give ")) {
            checked += 1;
            match parse(expression).and_then(|e| scope.evaluate(&e)) {
                Ok(ours) if same(&json(&ours), &shown(rest)) => {}
                Ok(ours) => wrong.push(format!("{expression}: README shows {rest}, rvr gives {}", json(&ours))),
                Err(error) => wrong.push(format!("{expression}: README shows {rest}, rvr fails: {}", error.0)),
            }
            last = None;
        }
    }
    assert!(checked > 100, "only {checked} examples were found in the README");
    assert!(wrong.is_empty(), "{} of {checked} README examples are wrong:\n{}", wrong.len(), wrong.join("\n"));
}

// RECIPE kinds-exactly, two-kinds-together
#[test]
fn the_readme_promotion_table_is_how_kinds_combine() {
    let text = std::fs::read_to_string(format!("{}/README.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let rows: Vec<_> = tables(&text).into_iter().filter(|row| row.contains_key("combined with")).collect();
    assert_eq!(rows.len(), 11);
    let names = ["b1", "i1", "i2", "i4", "i8", "u1", "u2", "u4", "u8", "f4", "f8"];
    for row in rows {
        let left = Kind::named(&row["combined with"]).unwrap();
        for right in names {
            assert_eq!(left.with(Kind::named(right).unwrap()).name(), row[right], "{} with {right}", left.name());
        }
    }
}
