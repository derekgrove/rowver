//! Every expression in the README's tables, evaluated on its three example events, gives the
//! result the README says, and every mistake it lists is refused with the words it shows.

use rvr_recipe::{parse, Array, Columns, Constant, Kind, Recipe, Scope, Store, Values};
use serde_json::Value;
use std::collections::BTreeMap;

/// The README's example events, as columns by name.
struct Events(BTreeMap<String, Array>);

impl Columns for Events {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        Ok(self.0.get(name).cloned())
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

fn values(kind: &str, numbers: &[f64]) -> Values {
    let kind = Kind::named(kind).unwrap();
    match kind.float() {
        true => Values::float(kind, numbers.to_vec()),
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
        Store::Float(held) => Value::from(held[at]),
    };
    match array {
        Array::Constant(Constant::Whole(n)) => Value::from(*n),
        Array::Constant(Constant::Float(x)) => Value::from(*x),
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
        (Value::Null, Value::String(inf)) => inf == "inf", // serde_json writes infinity as null
        (a, b) => a == b,
    }
}

/// What the README writes, read as JSON: `inf` is text, and a cell with no code is plain words.
fn shown(cell: &str) -> Value {
    match code(cell) {
        Some(text) => {
            let quoted = text.replace("inf", "\"inf\"");
            serde_json::from_str(&quoted).unwrap_or_else(|_| Value::from(text))
        }
        None => Value::from(cell),
    }
}

#[test]
fn every_function_is_known_and_in_the_readme() {
    let text = std::fs::read_to_string(format!("{}/README.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let recipe = Recipe::read(RECIPE).unwrap();
    let (events, sample) = (events(), BTreeMap::new());
    let scope = Scope::new(&events, &sample, &recipe.defines);
    for name in rvr_recipe::FUNCTIONS {
        let said = scope.evaluate(&parse(&format!("{name}(MET_pt)")).unwrap());
        assert!(!matches!(&said, Err(e) if e.0.contains("no function named")), "{name} is listed but unknown");
        assert!(text.contains(&format!("`{name}(")) || text.contains(&format!("`{name}`")), "{name} is not in the README");
    }
    let said = scope.evaluate(&parse("cnt(MET_pt)").unwrap()).unwrap_err();
    assert!(said.0.contains("There is no function named cnt"), "{said:?}");
}

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
            Some(_) => Recipe::read(&format!("format = \"rvr-recipe\"\nversion = 1\n{expression}")).map(|_| Array::Constant(Constant::Missing(String::new()))),
            None => parse(expression).and_then(|e| scope.evaluate(&e)),
        };
        checked += 1;
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
