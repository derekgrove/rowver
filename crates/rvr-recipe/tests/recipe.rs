//! Every expression of the fixture against what NumPy 2.5.1, awkward 2.12.0, coffea 2026.7.0 and
//! vector 1.8.1 gave for it.

use rvr_recipe::{parse, per_event, Array, Columns, Constant, Kind, Recipe, Scope, Store, Values};
use serde_json::Value;
use std::collections::BTreeMap;

/// The fixture's columns, served by name.
struct Held(BTreeMap<String, Array>);

impl Columns for Held {
    /// Hand out a copy of a held column.
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        Ok(self.0.get(name).cloned())
    }

    /// Whether a column of this name is held.
    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
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
    let defines: BTreeMap<String, _> = fixture["defines"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, text)| (name.clone(), parse(text.as_str().unwrap()).unwrap()))
        .collect();
    let scope = Scope::new(&held, &sample, &defines);
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
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
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
    assert!(message("Jet.pt")
        .contains("Only sample has a field after a dot, as in sample.era (the dot at column 4)"));
    assert!(message("sample.").contains("Expected a name after sample."));
    assert_eq!(message("count(Jet where pt > 30)"), "Expected ) at column 11");
    assert!(message("nothing(3)").contains("There is no function named nothing"));
    assert!(message("nothing(MET)").contains("There is no function named nothing"), "the name is checked before its values");
    assert!(message("sqrt(1, 2)").contains("takes 1 value, not 2"));
    assert!(message("'a' + 1").contains("Text"));
    assert!(message("sample.absent + 1").contains("sample.absent is used as a number, but this sample has no absent: give it one in samples.toml"));
    assert!(message(&"(".repeat(500)).contains("more than 100 levels deep"));
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
    // Recipe::read refuses a name that uses itself; a scope built by hand is still safe.
    let defines: BTreeMap<String, _> = [
        ("loop".to_owned(), parse("loop + 1").unwrap()),
        ("ring".to_owned(), parse("round + 1").unwrap()),
        ("round".to_owned(), parse("ring * 2").unwrap()),
    ]
    .into();
    let scope = Scope::new(&held, &sample, &defines);
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
    assert!(run("loop").unwrap_err().0.contains("loop refers to itself"));
    assert!(run("ring > 0").unwrap_err().0.contains("refers to itself"));
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
    // NaN is neither true nor false: made b1, it is a data error of its event.
    assert_eq!(run("stored(ratio, 'b1')").unwrap_err().1, Some(0));
    assert_eq!(whole("count(Jet_pt > 30)"), [1, 0, 1]);
    assert!(
        run("count(Jet_pt[Muon_pt > 3])").is_err(),
        "a mask of other lists"
    );
    // join takes lists only, two or more; the kinematics take three or four of one list.
    assert_eq!(run("join(Jet_pt)").unwrap_err().0, "join takes 2 values or more, not 1");
    assert!(run("join(Jet_pt, nJet)").unwrap_err().0.contains("join needs a list per event"));
    assert_eq!(run("pt(Jet_pt, Jet_pt)").unwrap_err().0, "pt takes 3 or 4 values (pt, eta, phi, and optionally a mass), not 2");
    assert!(run("pair_rapidity(Jet_pt)").unwrap_err().0.contains("pair_rapidity takes 3 or 4 values (pt, eta, phi"));
    let unlike = run("rapidity(Jet_pt, Jet_pt, Muon_pt)").unwrap_err();
    assert!(unlike.0.starts_with("rapidity needs pt, eta, phi, and any mass of the same list, member by member, and in this event one has 2 members and the other 1"), "{unlike:?}");
    assert_eq!(unlike.1, Some(0));
    assert!(run("floor(Jet_pt > 30)").is_ok(), "floor keeps true-or-false values, as NumPy");
}

#[test]
fn float_functions_give_the_same_bits_on_every_platform() {
    // The libm crate's results; macOS's own cos and exp give each a last bit one lower.
    let x = Array::Event(Values::float(Kind::F8, vec![0.46, 0.27]));
    let held = Held([("x".to_owned(), x)].into_iter().collect());
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
    let bits = |text: &str| match scope.evaluate(&parse(text).unwrap()).unwrap() {
        Array::Event(Values { store: Store::Float(held), .. }) => held.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert_eq!(bits("cos(x)")[0], 0x3fecac76498be14c);
    assert_eq!(bits("exp(x)")[1], 0x3ff4f59d48ad2682);
    // A constant is rounded as it stands, halves to even; a whole number stays one.
    let run = |text: &str| scope.evaluate(&parse(text).unwrap());
    assert_eq!(run("round(2.5)").unwrap(), Array::Constant(Constant::Float(2.0)));
    assert_eq!(run("round(-3)").unwrap(), Array::Constant(Constant::Whole(-3)));
    // NumPy rounds true-or-false values as 16-bit floats, a kind no column has.
    assert!(run("round(x > 0.3)").unwrap_err().0.starts_with("round rounds numbers, not true-or-false values"));
    assert_eq!(run("floor(1 > 0)").unwrap(), Array::Constant(Constant::Truth(true)));
}

/// A bare name is a defined name or a column; any other is refused as an unknown column,
/// wherever it stands, and a collection's prefix alone is no name.
#[test]
fn a_name_that_is_no_column_and_not_defined_is_refused_as_unknown() {
    let jets = Array::object(&[1, 0], Values::float(Kind::F4, vec![40.0])).unwrap();
    let held = Held([("Jet_pt".to_owned(), jets)].into());
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
    let message = |text: &str| {
        let read = parse(text).and_then(|expression| scope.evaluate(&expression));
        read.unwrap_err().0
    };
    assert_eq!(
        message("mass(Muon_pt, Muon_eta, Muon_phi)"),
        "There is no column named Muon_pt"
    );
    assert_eq!(message("Muon_pt > 3"), "There is no column named Muon_pt");
    assert_eq!(message("count(Jet)"), "There is no column named Jet");
    assert_eq!(message("Jet > 30"), "There is no column named Jet");
    assert!(scope.evaluate(&parse("Jet_pt").unwrap()).is_ok());
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
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
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
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
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

/// The columns of a run, counting how often each is asked for.
struct Counted(Held, std::cell::RefCell<BTreeMap<String, usize>>);

impl Columns for Counted {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        *self.1.borrow_mut().entry(name.to_owned()).or_default() += 1;
        self.0.column(name)
    }

    fn has(&self, name: &str) -> bool {
        self.0.has(name)
    }
}

/// `[define]` names expressions, each used by the lines below it, whatever a file calls its
/// branches; a defined name is worked out once however often it is used.
#[test]
fn defined_names_are_expressions_used_below_them() {
    let text = r#"
format = "rvr-recipe"
version = 1

[define]
good_lep = "PT_lep > 10 and abs(Eta_lep) < 2.4"
negative = "good_lep and Charge_lep < 0"

[keep]
one = "count(good_lep) >= 1"

[columns]
branches = ["PT_lep"]
n_negative = "count(negative)"
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
            lists(Values::whole(Kind::I4, vec![-1, -1, -1])),
        ),
    ]));
    let counted = Counted(held, Default::default());
    let sample = BTreeMap::new();
    let scope = Scope::new(&counted, &sample, &recipe.defines);
    let run = |text: &str| parse(text).and_then(|expression| scope.evaluate(&expression));
    let each = |values: Vec<i64>| Array::Event(Values::whole(Kind::I8, values));
    assert_eq!(run("count(good_lep)").unwrap(), each(vec![1, 0, 0]));
    assert_eq!(run("count(negative)").unwrap(), each(vec![1, 0, 0]));
    assert_eq!(
        run("PT_lep[good_lep]").unwrap(),
        Array::object(&[1, 0, 0], Values::float(Kind::F4, vec![25.0])).unwrap()
    );
    assert_eq!(
        run("count(good_lep) + count(good_lep) + sum(PT_lep[good_lep])").unwrap(),
        Array::Event(Values::float(Kind::F8, vec![27.0, 0.0, 0.0]))
    );
    // Eta_lep is read only through good_lep, which is worked out once for all of them.
    assert_eq!(counted.1.borrow()["Eta_lep"], 1);
    assert!(run("good_lep()").unwrap_err().0.contains(
        "good_lep is a defined name, not a function: write it without brackets, as good_lep"
    ));
    // A defined name that is also a column is refused where it is used: a name means one thing.
    let both: BTreeMap<String, _> = [("PT_lep".to_owned(), parse("Eta_lep").unwrap())].into();
    let shadowed = Scope::new(&counted, &sample, &both);
    assert!(shadowed.evaluate(&parse("PT_lep > 1").unwrap()).unwrap_err().0.contains("PT_lep is a name [define] gives an expression, and also the name of a branch of this file"));
    // `where` is a name like any other: after a value it is text left over.
    assert_eq!(run("count(PT_lep where PT_lep > 1)").unwrap_err().0, "Expected ) at column 14");
    assert!(parse("sample.where == 'x'").is_ok() && parse("branch('where') > 1").is_ok());
    // A name with several dots is suggested whole.
    assert!(parse("sum(recoMuons_muons__RECO.obj.pt_)")
        .unwrap_err()
        .0
        .contains("branch('recoMuons_muons__RECO.obj.pt_')"));
    let refused = |body: &str| {
        let head = "format = \"rvr-recipe\"\nversion = 1\n";
        Recipe::read(&format!("{head}{body}")).unwrap_err().0
    };
    assert_eq!(
        refused("[define]\na = \"b > 1\"\nb = \"PT_lep\"\n"),
        "[define] a uses b, which is defined below it: a defined name may use only the names defined above it"
    );
    assert!(refused("[define]\na = \"a + 1\"\n").contains("[define] a uses a, itself"));
    // branch('b') reads the branch b, not the name b defined below it.
    assert!(Recipe::read("format = \"rvr-recipe\"\nversion = 1\n[define]\na = \"branch('b') * 2\"\nb = \"PT_lep\"\n").is_ok());
    // A function always has brackets, so a defined name may share a function's name.
    assert!(Recipe::read("format = \"rvr-recipe\"\nversion = 1\n[define]\nfirst = \"PT_lep\"\n").is_ok());
    assert!(refused("[define]\n\"good-lep\" = \"PT_lep\"\n").contains("one plain name"));
    assert!(refused("[define]\nsample = \"PT_lep\"\n").contains("one plain name"));
    assert!(refused("[define.good]\nvalue = \"PT_lep\"\n")
        .contains("[define] good must be an expression in quotes"));
    // The earlier language's tables and keys are refused as any unknown name is.
    assert_eq!(
        refused("[object.good]\nfrom = \"lep\"\n"),
        "A recipe has no key \"object\"; it takes format, version, title, define, keep, columns, totals, derived."
    );
    assert!(refused("[column]\n").ends_with("Did you mean: columns?"));
    assert!(refused("[derive]\n").contains("A recipe has no key \"derive\"; it takes format, version, title, define, keep, columns, totals, derived. Did you mean: derived, define?"));
    assert_eq!(
        refused("[keep]\nx = { if = \"sample.y == 1\", pass = \"PT_lep > 1\" }\n"),
        "[keep] x has no key \"pass\"; it takes if, apply.\n[keep] x needs apply = \"…\": the cut, for the samples its if is true for"
    );
    assert!(refused("[keep]\nx = { if = \"sample.y == 1\" }\n").contains("[keep] x needs apply"));
    assert!(refused("[columns]\nbranches = [\"\"]\n")
        .contains("branches must be a list of branch names"));
    assert!(refused("[columns]\nbranches = [\"a\"]\na = \"PT_lep\"\n")
        .contains("Two columns are named a: give one another name"));
    // Text is not a name: a defined name may hold text that another, later, name spells.
    let quoted = "[define]\nis_mu = \"sample.channel == 'mu'\"\nmu = \"PT_lep > 20\"\n";
    let head = "format = \"rvr-recipe\"\nversion = 1\n";
    assert!(Recipe::read(&format!("{head}{quoted}")).is_ok());
    // A defined name is not a branch to store as it is.
    assert!(
        refused("[define]\ngood = \"PT_lep > 1\"\n[columns]\nbranches = [\"good\"]\n")
            .contains("[columns] branches lists good, a name [define] gives an expression")
    );
    // A dot after anything but sample is refused, saying how to name a branch with a dot in it.
    assert!(refused("[columns]\npt = \"sum(AnalysisJetsAuxDyn.pt)\"\n")
        .contains("a branch whose name holds a dot is written branch('AnalysisJetsAuxDyn.pt')"));
    // A derived column uses what the file holds: the stored columns and the derived ones above.
    let derived = "[define]\ngood = \"PT_lep > 1\"\n[columns]\nbranches = [\"PT_lep\"]\nn = \"count(good)\"\n";
    let read = Recipe::read(&format!("{head}{derived}[derived]\nhalf = \"n / 2\"\nquarter = \"half / 2\"\n")).unwrap();
    assert_eq!(read.derived, [("half".to_owned(), "n / 2".to_owned()), ("quarter".to_owned(), "half / 2".to_owned())]);
    for (line, said) in [
        ("x = \"good\"", "x uses good, but good is a line of [define], which the file does not keep"),
        ("x = \"Eta_lep\"", "x uses Eta_lep, but the file does not store Eta_lep"),
        ("x = \"y\"\ny = \"n\"", "x uses y, but y is a derived column below it"),
        ("n = \"PT_lep\"", "n is already the name of a column, a total or a line of [define]"),
        ("good = \"n\"", "good is already the name of a column, a total or a line of [define]"),
        ("entry = \"n\"", "a derived column is named with one plain name"),
        ("x = \"branch('Eta_lep')\"", "x uses Eta_lep, but the file does not store Eta_lep"),
        ("x = \"branch('y') + 1\"\ny = \"n\"", "x uses y, but y is a derived column below it"),
        ("x = \"branch(sample.trigger)\"", "branch(sample.KEY) names another column for each sample"),
    ] {
        let wrong = Recipe::read(&format!("{head}{derived}[derived]\n{line}\n")).unwrap_err().0;
        assert!(wrong.contains(said), "{line}: {wrong}");
    }
    // Every table empty, or left out, is the same recipe.
    let empty = Recipe::read("format = \"rvr-recipe\"\nversion = 1\n[define]\n[keep]\n[columns]\n")
        .unwrap();
    assert_eq!(
        empty,
        Recipe::read("format = \"rvr-recipe\"\nversion = 1\n").unwrap()
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
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
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
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
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

    // A value outside the kind, or NaN, is a data error of its value.
    for (value, said) in [(-1.0, "-1.0 does not fit in u8"), (f64::NAN, "NaN is no number"), (f64::INFINITY, "inf does not fit in u8"), (18446744073709551616.0, "1.8446744073709552e19 does not fit in u8")] {
        let error = Values::float(Kind::F8, vec![0.0, value]).convert(Kind::U8).unwrap_err();
        assert!(error.0.starts_with(said) && error.1 == Some(1), "{value}: {error:?}");
    }
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
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
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

/// `[totals]` sums an expression over every event read, before the cuts, or over another tree of
/// each file; only a derived column may use one, as `sample.NAME`.
#[test]
fn totals_are_sums_only_derived_columns_use() {
    let head = "format = \"rvr-recipe\"\nversion = 1\n";
    let text = format!("{head}[columns]\nbranches = [\"w\"]\n[totals]\nsumw = \"genWeight\"\nruns = {{ value = \"genEventSumw\", tree = \"Runs\" }}\n[derived]\nweight = \"w / sample.sumw\"\n");
    let read = Recipe::read(&text).unwrap();
    let totals: Vec<(&str, Option<&str>)> = read.totals.iter().map(|t| (t.name.as_str(), t.tree.as_deref())).collect();
    assert_eq!(totals, [("sumw", None), ("runs", Some("Runs"))]);
    assert_eq!(read.derived, [("weight".to_owned(), "w / sample.sumw".to_owned())]);
    let refused = |body: &str| Recipe::read(&format!("{head}{body}")).unwrap_err().0;
    for (body, said) in [
        ("[keep]\nx = \"sample.sumw > 0\"\n[totals]\nsumw = \"w\"\n", "[keep] x uses sample.sumw, a total of [totals], known only once every event is read: use it under [derived]"),
        ("[columns]\ny = \"w / sample.sumw\"\n[totals]\nsumw = \"w\"\n", "[columns] y uses sample.sumw"),
        ("[totals]\nsumw = \"w\"\n[derived]\nsumw = \"w\"\n", "[derived] sumw: sumw is already the name of a column, a total or a line of [define]"),
        ("[columns]\nbranches = [\"w\"]\n[totals]\nw = \"w\"\n", "[totals] w: w is already the name of a column or a line of [define]"),
        ("[totals]\nevents_read = \"1\"\n", "[totals] events_read: a total is named with one plain name"),
        ("[define]\ng = \"w > 1\"\n[totals]\nt = { value = \"g\", tree = \"Runs\" }\n", "[totals] t reads the tree Runs, and g is a line of [define]"),
        ("[totals]\nt = { tree = \"Runs\" }\n", "[totals] t needs a value"),
        ("[totals]\nt = { value = \"1\", tree = \"\" }\n", "[totals] t tree must name a tree of each file"),
    ] {
        let wrong = refused(body);
        assert!(wrong.contains(said), "{body}: {wrong}");
    }
}
