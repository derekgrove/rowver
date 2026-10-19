//! Every expression of the fixture against what NumPy 2.5.1, awkward 2.12.0 and coffea 2026.7.0
//! gave for it.

use rvr_recipe::{
    parse, per_event, Array, Columns, Constant, Helper, Kind, Recipe, Scope, Store, Values,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// The fixture's columns, served by name.
struct Held(BTreeMap<String, Array>);

impl Columns for Held {
    /// Hand out a copy of a held column.
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        Ok(self.0.get(name).cloned())
    }

    /// Whether a held column's name begins with this.
    fn begins(&self, start: &str) -> bool {
        self.0.keys().any(|name| name.starts_with(start))
    }
}

/// Read one recorded number, with infinities spelled as text.
fn float(value: &Value) -> f64 {
    match value.as_str() {
        Some("inf") => f64::INFINITY,
        Some("-inf") => f64::NEG_INFINITY,
        Some(_) => f64::NAN,
        None => value.to_string().parse().unwrap(),
    }
}

/// Read a recorded column or result into an array.
fn array(record: &Value) -> Array {
    let values = record["values"].as_array().unwrap();
    if record["shape"] == "constant" {
        return Array::Constant(match &values[0] {
            Value::Bool(truth) => Constant::Truth(*truth),
            number if number.to_string().contains(['.', 'e']) => Constant::Float(float(number)),
            number => Constant::Whole(number.to_string().parse().unwrap()),
        });
    }
    let kind = Kind::named(record["kind"].as_str().unwrap()).unwrap();
    let held = if kind.float() {
        Values::float(kind, values.iter().map(float).collect())
    } else {
        let whole = |value: &Value| match value {
            Value::Bool(truth) => *truth as i64,
            number => number.to_string().parse::<i128>().unwrap() as i64,
        };
        Values::whole(kind, values.iter().map(whole).collect())
    };
    match record["counts"].as_array() {
        Some(counts) => {
            let counts: Vec<usize> = counts
                .iter()
                .map(|count| count.as_u64().unwrap() as usize)
                .collect();
            Array::object(&counts, held).unwrap()
        }
        None => Array::Event(held),
    }
}

/// Whether two results agree: exactly, or with `close` to a few parts in ten million of the
/// larger of the value and `scale`, where 32-bit functions round differently. The scale is
/// the size of the quantities that went into a formula that cancels, such as a mass.
fn agree(ours: &Array, theirs: &Array, close: bool, scale: f64) -> bool {
    let near = |a: &Values, b: &Values| {
        a.kind == b.kind
            && a.len() == b.len()
            && match (&a.store, &b.store) {
                (Store::Float(x), Store::Float(y)) if close => {
                    x.iter().zip(y.iter()).all(|(x, y)| {
                        x == y
                            || (x.is_nan() && y.is_nan())
                            || (x - y).abs() <= 4e-7 * y.abs().max(scale).max(1e-30)
                    })
                }
                (x, y) => x == y,
            }
    };
    match (ours, theirs) {
        (Array::Constant(Constant::Float(a)), Array::Constant(Constant::Float(b))) => {
            (a - b).abs() <= 1e-12 * b.abs()
        }
        (Array::Constant(a), Array::Constant(b)) => a == b,
        (Array::Event(a), Array::Event(b)) => near(a, b),
        (
            Array::Object { offsets, values },
            Array::Object {
                offsets: others,
                values: b,
            },
        ) => offsets == others && near(values, b),
        _ => false,
    }
}

/// Load the fixture written by `tests/generate_recipe_fixture.py`.
fn fixture() -> Value {
    let path = format!("{}/tests/fixtures/recipe.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Evaluate every recorded expression and compare kind, shape, and values.
#[test]
fn expressions_give_what_numpy_and_awkward_give() {
    let fixture = fixture();
    let columns = fixture["columns"].as_object().unwrap();
    let held = Held(
        columns
            .iter()
            .map(|(name, record)| (name.clone(), array(record)))
            .collect(),
    );
    let sample: BTreeMap<String, Constant> = fixture["sample"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| {
            let constant = match value {
                Value::String(text) => Constant::Text(text.clone()),
                Value::Bool(truth) => Constant::Truth(*truth),
                number if number.to_string().contains('.') => Constant::Float(float(number)),
                number => Constant::Whole(number.as_i64().unwrap()),
            };
            (name.clone(), constant)
        })
        .collect();
    let objects: BTreeMap<String, _> = fixture["objects"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, text)| (name.clone(), parse(text.as_str().unwrap()).unwrap()))
        .collect();
    let helpers: BTreeMap<String, Helper> = fixture["helpers"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, helper)| {
            let takes = helper["takes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap().to_owned());
            let value = parse(helper["value"].as_str().unwrap()).unwrap();
            (
                name.clone(),
                Helper {
                    takes: takes.collect(),
                    value,
                },
            )
        })
        .collect();
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let mut wrong = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let text = case["expression"].as_str().unwrap();
        let expected = array(case);
        let result = parse(text).and_then(|expression| scope.evaluate(&expression));
        match result {
            Ok(ours)
                if agree(
                    &ours,
                    &expected,
                    case["close"] == true,
                    case["scale"].as_f64().unwrap_or(0.0),
                ) => {}
            Ok(ours) => wrong.push(format!(
                "{text}\n  ours   {}\n  theirs {}",
                brief(&ours, &expected),
                brief(&expected, &ours)
            )),
            Err(error) => wrong.push(format!("{text}\n  failed: {error}")),
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} differ:\n{}",
        wrong.len(),
        fixture["cases"].as_array().unwrap().len(),
        wrong.join("\n")
    );
}

/// Show an array for a failure message: its kind and shape, and its values from the first
/// place where it and the other array differ.
fn brief(array: &Array, other: &Array) -> String {
    let parts = |array: &Array| match array {
        Array::Constant(constant) => (format!("constant {constant:?}"), Vec::new()),
        Array::Event(values) => (format!("event {:?}", values.kind), shown(values)),
        Array::Object { offsets, values } => (
            format!("object {:?} of {} lists", values.kind, offsets.len() - 1),
            shown(values),
        ),
    };
    let ((shape, own), (_, others)) = (parts(array), parts(other));
    let from = own.iter().zip(&others).position(|(a, b)| a != b);
    let from = from.unwrap_or(own.len().min(others.len()));
    let some: Vec<&str> = own.iter().skip(from).take(6).map(String::as_str).collect();
    format!(
        "{shape}, {} values, from {from}: {}",
        own.len(),
        some.join(" ")
    )
}

/// Each value of a column as text.
fn shown(values: &Values) -> Vec<String> {
    match &values.store {
        Store::Float(held) => held.iter().map(|value| format!("{value:?}")).collect(),
        Store::Whole(held) => held.iter().map(|value| value.to_string()).collect(),
    }
}

/// Refuse expressions that cannot be read or do not make sense, saying why.
#[test]
fn mistakes_are_refused_with_a_reason() {
    let held = Held(BTreeMap::new());
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let message = |text: &str| {
        parse(text)
            .and_then(|expression| scope.evaluate(&expression))
            .unwrap_err()
            .0
    };
    assert!(message("MET >").contains("Expected a value at the end"));
    assert!(message("MET > 3 )").contains("Unexpected text at column 9"));
    assert!(message("(MET > 3").contains("Expected )"));
    assert!(message("'open").contains("never closed"));
    assert!(message("3 $ 4").contains("Unexpected $"));
    assert_eq!(message("MET > 30"), "There is no column named MET");
    assert_eq!(message("Jet.pt"), "There is no column named Jet");
    assert!(message("nothing(3)").contains("no operation or helper named nothing"));
    assert!(message("sqrt(1, 2)").contains("takes 1 value, not 2"));
    assert!(message("'a' + 1").contains("Text"));
    assert!(message("sample.absent + 1").contains("missing"));
    assert!(message(&"(".repeat(500)).contains("Too many nested brackets"));
    assert_eq!(
        per_event(Array::Constant(Constant::Truth(true)), 2).unwrap(),
        Array::Event(Values::whole(Kind::B1, vec![1, 1]))
    );
}

/// What the fixture does not hold: loops, lazy sample conditions, NaN, and 64-bit identities.
#[test]
fn edge_cases_follow_the_documented_rules() {
    let big = Values::whole(Kind::U8, vec![u64::MAX as i64, 3, (1_u64 << 63) as i64]);
    let ratio = Values::float(Kind::F8, vec![f64::NAN, 0.25, -2.0]);
    let lists = |values: Values| Array::object(&[2, 0, 1], values).unwrap();
    let muons = Array::object(&[1, 1, 1], Values::float(Kind::F4, vec![9.0, 8.0, 7.0])).unwrap();
    let columns = [
        ("event", Array::Event(big)),
        ("ratio", Array::Event(ratio)),
        ("nJet", Array::Event(Values::whole(Kind::I4, vec![2, 0, 1]))),
        (
            "Jet_pt",
            lists(Values::float(Kind::F4, vec![40.0, 20.0, 35.0])),
        ),
        ("Jet_id", lists(Values::whole(Kind::I4, vec![6, 2, 6]))),
        ("Muon_pt", muons),
    ];
    let held = Held(
        columns
            .into_iter()
            .map(|(name, array)| (name.to_owned(), array))
            .collect(),
    );
    let sample: BTreeMap<String, Constant> = [("is_data".to_owned(), Constant::Truth(true))].into();
    let objects: BTreeMap<String, _> =
        [("loop".to_owned(), parse("loop where pt > 1").unwrap())].into();
    let again = Helper {
        takes: vec!["x".into()],
        value: parse("again(x)").unwrap(),
    };
    let helpers: BTreeMap<String, Helper> = [("again".to_owned(), again)].into();
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let run = |text: &str| parse(text).and_then(|expression| scope.evaluate(&expression));
    let hundredths = |value: f64| {
        if value.is_nan() {
            -99
        } else {
            (value * 100.0) as i64
        }
    };
    let whole = |text: &str| match run(text).unwrap() {
        Array::Event(values) => match values.store {
            Store::Whole(held) => held.to_vec(),
            Store::Float(held) => held.iter().copied().map(hundredths).collect(),
        },
        Array::Constant(Constant::Whole(value)) => vec![value],
        other => panic!("{other:?}"),
    };
    let top = 1_u64 << 63;
    assert!(run("count(loop)")
        .unwrap_err()
        .0
        .contains("refers to itself"));
    assert!(run("again(1)").unwrap_err().0.contains("refers to itself"));
    assert_eq!(whole("if(sample.is_data, 1, genWeight)"), [1]);
    assert_eq!(
        run("sample.is_data or genWeight > 0").unwrap(),
        Array::Constant(Constant::Truth(true))
    );
    assert!(run("if(sample.is_data, genWeight, 1)").is_err());
    assert_eq!(whole("max(event, 5)"), [u64::MAX as i64, 5, top as i64]);
    assert_eq!(whole("abs(event)"), [u64::MAX as i64, 3, top as i64]);
    assert_eq!(whole("sign(event)"), [1, 1, 1]);
    assert_eq!(whole("event > 4"), [1, 0, 1]);
    assert!(run("event > -1").unwrap_err().0.contains("does not fit"));
    assert_eq!(whole("max(ratio, 0)"), [-99, 25, 0]);
    assert_eq!(whole("min(ratio, 0)"), [-99, 0, -200]);
    assert_eq!(whole("stored(ratio, 'b1')"), [1, 1, 1]);
    assert_eq!(whole("count(Jet where pt > 30)"), [1, 0, 1]);
    assert!(run("count(Jet where id)")
        .unwrap_err()
        .0
        .contains("true or false"));
    assert!(run("count(Jet where Muon_pt > 3)")
        .unwrap_err()
        .0
        .contains("not about the members of Jet"));
}

/// A bare name that is no column is a collection only when columns begin with it followed by
/// `_`; any other is refused as an unknown name, wherever it stands.
#[test]
fn a_name_that_is_no_column_and_no_collection_is_refused_as_unknown() {
    let jets = Array::object(&[1, 0], Values::float(Kind::F4, vec![40.0])).unwrap();
    let held = Held([("Jet_pt".to_owned(), jets)].into());
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let message = |text: &str| {
        let read = parse(text).and_then(|expression| scope.evaluate(&expression));
        read.unwrap_err().0
    };
    assert_eq!(
        message("mass(Muon_pt, Muon_eta, Muon_phi)"),
        "There is no column named Muon_pt"
    );
    assert_eq!(message("Muon_pt > 3"), "There is no column named Muon_pt");
    assert_eq!(message("count(Muon)"), "There is no column named Muon");
    // A name that only begins another, without the `_`, is no collection either.
    assert_eq!(message("Je > 3"), "There is no column named Je");
    assert_eq!(message("Jet_ > 3"), "There is no column named Jet_");
    // A collection is still told from a value, and its missing attribute named whole.
    assert_eq!(
        message("Jet > 30"),
        "An operator needs values, but Jet is a collection; name one of its attributes, such as Jet.pt"
    );
    assert_eq!(message("Jet.eta"), "There is no column named Jet_eta");
    assert!(scope.evaluate(&parse("Jet.pt").unwrap()).is_ok());
}

/// Particle coordinates must describe the same members of every event, even when totals match.
#[test]
fn kinematics_refuse_coordinates_from_different_event_lists() {
    let lists = |counts: &[usize], values: Vec<f64>| {
        Array::object(counts, Values::float(Kind::F8, values)).unwrap()
    };
    let columns = [
        ("pt", lists(&[2, 1], vec![10.0, 10.0, 3.0])),
        ("eta", lists(&[2, 1], vec![0.0, 0.0, 1.0])),
        ("phi", lists(&[2, 1], vec![0.0, std::f64::consts::PI, 0.0])),
        ("shifted", lists(&[1, 2], vec![0.0, 0.0, 1.0])),
        ("extra_event", lists(&[1, 1, 1], vec![0.0, 0.0, 1.0])),
        ("target_eta", lists(&[1, 0], vec![0.0])),
        ("target_phi", lists(&[1, 0], vec![0.0])),
        ("shifted_target", lists(&[0, 1], vec![0.0])),
    ];
    let held = Held(
        columns
            .into_iter()
            .map(|(name, array)| (name.to_owned(), array))
            .collect(),
    );
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let run = |text: &str| parse(text).and_then(|expression| scope.evaluate(&expression));
    for expression in [
        "mass(pt, shifted, phi)",
        "mass(pt, eta, shifted)",
        "mass(pt, extra_event, phi)",
        "dr_nearest(eta, shifted, target_eta, target_phi)",
        "dr_nearest(eta, phi, target_eta, shifted_target)",
        "nearest_index(eta, shifted, target_eta, target_phi)",
        "nearest_index(eta, phi, target_eta, shifted_target)",
    ] {
        assert!(
            run(expression).is_err(),
            "accepted mismatched coordinates: {expression}"
        );
    }
    let Array::Event(masses) = run("mass(pt, eta, phi)").unwrap() else {
        panic!("mass must give one value per event")
    };
    assert!((masses.number(0) - 20.0).abs() < 1e-12);
    assert!(masses.number(1).abs() < 1e-6);
    assert_eq!(
        run("nearest_index(eta, phi, target_eta, target_phi)").unwrap(),
        Array::object(&[2, 1], Values::whole(Kind::I8, vec![0, 0, -1])).unwrap()
    );
}

/// What the fixture's columns cannot show of the list operations: where NaN is placed, and
/// what is refused. The expected positions are what awkward 2.12.0 gave for these lists.
#[test]
fn list_operations_place_nan_as_awkward_does_and_refuse_mistakes() {
    let nan = f64::NAN;
    let lists =
        |values: Vec<f64>| Array::object(&[3, 1, 0, 3], Values::float(Kind::F8, values)).unwrap();
    let held = Held(BTreeMap::from([
        (
            "x".to_owned(),
            lists(vec![3.0, nan, 1.0, nan, 2.0, 2.0, 1.0]),
        ),
        (
            "n".to_owned(),
            Array::Event(Values::whole(Kind::I4, vec![5, 6, 7, 8])),
        ),
        (
            "tiny".to_owned(),
            Array::Event(Values::whole(Kind::U1, vec![1, 2, 3, 4])),
        ),
        (
            // Two whole numbers that are the same 64-bit float, and the two largest unsigned.
            "big".to_owned(),
            Array::object(
                &[2, 0, 0, 2],
                Values::whole(Kind::U8, vec![1 << 53, (1 << 53) + 1, -2, -1]),
            )
            .unwrap(),
        ),
    ]));
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let run = |text: &str| parse(text).and_then(|expression| scope.evaluate(&expression));
    let places = |values: Vec<i64>| Array::object(&[3, 1, 0, 3], Values::whole(Kind::I8, values));
    let each = |values: Vec<i64>| Array::Event(Values::whole(Kind::I8, values));
    assert_eq!(
        run("order(x)").unwrap(),
        places(vec![1, 2, 0, 0, 2, 0, 1]).unwrap()
    );
    assert_eq!(
        run("order(x, 'descending')").unwrap(),
        places(vec![1, 0, 2, 0, 0, 1, 2]).unwrap()
    );
    assert_eq!(run("least_index(x, -1)").unwrap(), each(vec![2, 0, -1, 2]));
    assert_eq!(
        run("greatest_index(x, -1)").unwrap(),
        each(vec![0, 0, -1, 0])
    );
    // A position of -1, or one past the end of the list, takes the fallback.
    assert_eq!(
        run("take(x, index(x) + 2, -5) == -5").unwrap(),
        Array::object(
            &[3, 1, 0, 3],
            Values::whole(Kind::B1, vec![0, 1, 1, 1, 0, 1, 1])
        )
        .unwrap()
    );
    // Whole numbers are ordered as whole numbers, not through floats.
    assert_eq!(
        run("greatest_index(big, -1)").unwrap(),
        each(vec![1, -1, -1, 1])
    );
    assert_eq!(
        run("order(big, 'descending')").unwrap(),
        Array::object(&[2, 0, 0, 2], Values::whole(Kind::I8, vec![1, 0, 1, 0])).unwrap()
    );
    let message = |text: &str| run(text).unwrap_err().0;
    assert!(message("take(x, x > 1, 0)").contains("not true-or-false"));
    assert!(message("bit_and(tiny, 256)").contains("does not fit beside"));
    assert!(message("bit_and(x > 1, 1)").contains("not true-or-false"));
    assert!(message("shift_right(n, -1)").contains("negative"));
    assert!(message("bit_and(x, 1)").contains("whole numbers"));
    assert!(message("bit_and(n, n)").contains("whole number as its second value"));
    assert!(message("order(x, 'up')").contains("'ascending' or 'descending'"));
    assert!(message("take(x, x, 0)").contains("whole-number positions"));
    assert!(message("least_index(x, 0.5)").contains("whole number"));
    assert!(message("dr_farthest(x, x, x)").contains("takes 4 or 5 values"));
    assert!(message("pair_mass(x, x)").contains("optionally a mass"));
    assert!(message("delta_phi(x)").contains("takes 2 values"));
}

/// A `[collection.*]` names the column behind each attribute, for files that do not spell
/// columns as NanoAOD does; its members are counted from its lists, with no `n` column.
#[test]
fn a_collection_reads_its_attributes_from_the_columns_it_names() {
    let text = r#"
format = "rvr-recipe"
version = 1

[collection.lep]
pt = "PT_lep"
eta = "Eta_lep"
charge = "Charge_lep"

[object.good_lep]
from = "lep"
where = "pt > 10 and abs(eta) < 2.4"
"#;
    let recipe = Recipe::read(text).unwrap();
    let lists = |values: Values| Array::object(&[2, 0, 1], values).unwrap();
    let held = Held(BTreeMap::from([
        (
            "PT_lep".to_owned(),
            lists(Values::float(Kind::F4, vec![25.0, 8.0, 40.0])),
        ),
        (
            "Eta_lep".to_owned(),
            lists(Values::float(Kind::F4, vec![0.5, 1.0, -3.0])),
        ),
        (
            "Charge_lep".to_owned(),
            lists(Values::whole(Kind::I4, vec![1, -1, -1])),
        ),
    ]));
    let (sample, helpers) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &recipe.objects,
        collections: &recipe.collections,
        helpers: &helpers,
    };
    let run = |text: &str| parse(text).and_then(|expression| scope.evaluate(&expression));
    let each = |values: Vec<i64>| Array::Event(Values::whole(Kind::I8, values));
    assert_eq!(run("lep.pt").unwrap(), run("PT_lep").unwrap());
    assert_eq!(run("count(lep)").unwrap(), each(vec![2, 0, 1]));
    assert_eq!(run("count(good_lep)").unwrap(), each(vec![1, 0, 0]));
    assert_eq!(run("sum(good_lep.charge)").unwrap(), each(vec![1, 0, 0]));
    assert_eq!(
        run("count(lep where charge < 0)").unwrap(),
        each(vec![1, 0, 1])
    );
    assert_eq!(
        run("(lep where pt > 30).eta").unwrap(),
        Array::object(&[0, 0, 1], Values::float(Kind::F4, vec![-3.0])).unwrap()
    );
    // The presser is told of the columns behind the attributes the recipe uses, and of
    // the first, which counts the members; `charge` is used by no expression in it.
    let mentions = recipe.mentions();
    assert!(mentions.contains("PT_lep") && mentions.contains("Eta_lep"));
    assert!(!mentions.contains("Charge_lep"));
    let message = |text: &str| run(text).unwrap_err().0;
    let absent = BTreeMap::from([(
        "tau".to_owned(),
        vec![("pt".to_owned(), "PT_tau".to_owned())],
    )]);
    let lacking = Scope {
        collections: &absent,
        columns: &held,
        sample: &sample,
        objects: &recipe.objects,
        helpers: &helpers,
    };
    assert_eq!(
        lacking
            .evaluate(&parse("count(tau)").unwrap())
            .unwrap_err()
            .0,
        "tau.pt is the column PT_tau, and there is no column of that name"
    );
    assert_eq!(
        message("lep.phi"),
        "lep has no attribute named phi; it has pt, eta, charge"
    );
    // Inside a where, a name the collection does not have is looked for as any other name.
    assert_eq!(
        message("count(lep where phi > 0)"),
        "There is no column named phi"
    );
    assert!(message("lep + 1").contains("lep is a collection"));
    let refused = |body: &str| {
        let head = "format = \"rvr-recipe\"\nversion = 1\n";
        Recipe::read(&format!("{head}{body}")).unwrap_err().0
    };
    assert_eq!(
        refused("[collection.lep]\n"),
        "[collection.lep] names no column"
    );
    assert_eq!(
        refused("[collection.lep]\npt = 3\n"),
        "[collection.lep] pt must name a column in quotes"
    );
    assert!(refused("[collection.sample]\npt = \"PT_lep\"\n").contains("plain name"));
    assert!(refused("[collection.lep]\n\"lep-pt\" = \"PT_lep\"\n").contains("not a name"));
    assert!(refused("[collection.lep]\nwhere = \"PT_lep\"\n").contains("not a name"));
    assert!(refused("[collection.lep]\npt = \"\"\n").contains("must name a column"));
    assert!(
        refused("[collection.lep]\npt = \"PT_lep\"\n[object.lep]\nfrom = \"lep\"\n")
            .contains("both a collection and an object")
    );
}

/// Integer list reductions keep adjacent 64-bit values distinct without passing through floats.
#[test]
fn extrema_preserve_large_integer_ordering() {
    let boundary = 1_i64 << 53;
    let columns = [
        (
            "signed",
            Array::object(
                &[2, 0, 2],
                Values::whole(
                    Kind::I8,
                    vec![boundary + 1, boundary, -boundary, -boundary - 1],
                ),
            )
            .unwrap(),
        ),
        (
            "unsigned",
            Array::object(
                &[2, 0, 2],
                Values::whole(
                    Kind::U8,
                    vec![
                        (u64::MAX - 1) as i64,
                        u64::MAX as i64,
                        i64::MIN,
                        i64::MIN + 1,
                    ],
                ),
            )
            .unwrap(),
        ),
    ];
    let held = Held(
        columns
            .into_iter()
            .map(|(name, array)| (name.to_owned(), array))
            .collect(),
    );
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    for (expression, kind, expected) in [
        (
            "least(signed, 0)",
            Kind::I8,
            vec![boundary, 0, -boundary - 1],
        ),
        (
            "greatest(signed, 0)",
            Kind::I8,
            vec![boundary + 1, 0, -boundary],
        ),
        (
            "least(unsigned, 0)",
            Kind::U8,
            vec![(u64::MAX - 1) as i64, 0, i64::MIN],
        ),
        (
            "greatest(unsigned, 0)",
            Kind::U8,
            vec![u64::MAX as i64, 0, i64::MIN + 1],
        ),
    ] {
        let actual = scope.evaluate(&parse(expression).unwrap()).unwrap();
        assert_eq!(
            actual,
            Array::Event(Values::whole(kind, expected)),
            "{expression}"
        );
    }
}

/// Casting finite floats within the u64 range must not saturate at the signed i64 limit.
#[test]
fn stored_float_to_u64_preserves_values_above_signed_range() {
    let values = vec![
        0.0,
        1.75,
        9223372036854774784.0,
        9223372036854775808.0,
        9223372036854777856.0,
        18446744073709549568.0,
    ];
    let held = Held(BTreeMap::from([(
        "values".to_owned(),
        Array::Event(Values::float(Kind::F8, values)),
    )]));
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let result = scope
        .evaluate(&parse("stored(values, 'u8')").unwrap())
        .unwrap();
    let expected = [
        0_u64,
        1,
        9223372036854774784,
        9223372036854775808,
        9223372036854777856,
        18446744073709549568,
    ];
    assert_eq!(
        result,
        Array::Event(Values::whole(
            Kind::U8,
            expected.into_iter().map(|value| value as i64).collect()
        ))
    );

    // This correction does not choose a new policy for values outside that range.
    let outside = Values::float(
        Kind::F8,
        vec![-1.0, f64::NAN, f64::INFINITY, 18446744073709551616.0],
    );
    assert_eq!(
        outside.cast(Kind::U8),
        Values::whole(Kind::U8, vec![-1, 0, i64::MAX, i64::MAX])
    );
}

/// A missing or nonfinite particle coordinate must not become a plausible zero mass.
#[test]
fn mass_preserves_nan_instead_of_clipping_it_to_zero() {
    let lists = |values| Array::object(&[1, 1, 0, 2], Values::float(Kind::F8, values)).unwrap();
    let held = Held(BTreeMap::from([
        ("pt".to_owned(), lists(vec![1.0, f64::INFINITY, 2.0, 2.0])),
        ("eta".to_owned(), lists(vec![f64::NAN, 0.0, 0.0, 0.0])),
        (
            "phi".to_owned(),
            lists(vec![0.0, 0.0, 0.0, std::f64::consts::PI]),
        ),
    ]));
    let (sample, objects, helpers) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let scope = Scope {
        columns: &held,
        sample: &sample,
        objects: &objects,
        collections: &BTreeMap::new(),
        helpers: &helpers,
    };
    let Array::Event(masses) = scope
        .evaluate(&parse("mass(pt, eta, phi)").unwrap())
        .unwrap()
    else {
        panic!("mass must give one value per event")
    };
    assert!(masses.number(0).is_nan());
    assert!(masses.number(1).is_nan());
    assert_eq!(masses.number(2), 0.0);
    assert!((masses.number(3) - 4.0).abs() < 1e-12);
}
