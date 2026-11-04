//! Random expressions against the README's rules, worked out with NumPy by
//! tests/generate_recipe_properties.py: the same kind, shape and bits, or a data error where the
//! values are ones the recipe cannot take. `RECIPE_PROPERTIES` names another fixture (the
//! 10,000-case run, made by hand into a temporary folder).

use rvr_recipe::{parse, Array, Kind, Scope, Store, Values};
use serde_json::Value;
use std::collections::BTreeMap;

#[allow(dead_code)]
mod common;
use common::Held;

/// A recorded column or result: its floats from their bits, its whole numbers and truths as written.
fn array(record: &Value) -> Array {
    let kind = Kind::named(record["kind"].as_str().unwrap()).unwrap();
    let held = match record["bits"].as_array() {
        Some(bits) => {
            let bits = bits.iter().map(|b| b.as_u64().unwrap());
            let floats = match kind == Kind::named("f4").unwrap() {
                true => bits.map(|b| f32::from_bits(b as u32) as f64).collect(),
                false => bits.map(f64::from_bits).collect(),
            };
            Values::float(kind, floats)
        }
        None => {
            let whole = |value: &Value| match value {
                Value::Bool(truth) => *truth as i64,
                number => number.to_string().parse::<i128>().unwrap() as i64,
            };
            Values::whole(kind, record["values"].as_array().unwrap().iter().map(whole).collect())
        }
    };
    match record["counts"].as_array() {
        Some(counts) => Array::object(&counts.iter().map(|c| c.as_u64().unwrap() as usize).collect::<Vec<_>>(), held).unwrap(),
        None => Array::Event(held),
    }
}

/// Whether two arrays are the same: kind, shape, and every value's bits (any NaN for a NaN).
fn same(ours: &Array, theirs: &Array) -> bool {
    let values = |a: &Values, b: &Values| {
        a.kind == b.kind
            && match (&a.store, &b.store) {
                (Store::Float(x), Store::Float(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.to_bits() == q.to_bits() || (p.is_nan() && q.is_nan())),
                (x, y) => x == y,
            }
    };
    match (ours, theirs) {
        (Array::Event(a), Array::Event(b)) => values(a, b),
        (Array::Object { offsets, values: a }, Array::Object { offsets: others, values: b }) => offsets == others && values(a, b),
        _ => false,
    }
}

// RECIPE operators, choosing-members-of-a-list, reducing-a-jagged-list-to-a-flat-value, math-value-by-value, bits, text-branches-and-stored-types, kinds-exactly, two-kinds-together, fixed-values-beside-a-column, numbers, conversions, signed-zeros, what-differs-from-numpy, functions-exactly, values-the-recipe-cannot-take
#[test]
fn random_expressions_give_what_the_readme_rules_give() {
    let path = std::env::var("RECIPE_PROPERTIES").unwrap_or_else(|_| format!("{}/tests/fixtures/properties.json", env!("CARGO_MANIFEST_DIR")));
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let held = Held(fixture["columns"].as_object().unwrap().iter().map(|(name, record)| (name.clone(), array(record))).collect());
    let (sample, defines) = (BTreeMap::new(), BTreeMap::new());
    let scope = Scope::new(&held, &sample, &defines);
    let cases = fixture["cases"].as_array().unwrap();
    let mut wrong = Vec::new();
    for case in cases {
        let text = case["expression"].as_str().unwrap();
        let result = parse(text).and_then(|expression| scope.evaluate(&expression));
        match (result, case["error"] == true) {
            (Err(e), true) if e.1.is_some() => {}
            (Err(e), true) => wrong.push(format!("{text}\n  refused, not a data error: {e}")),
            (Ok(_), true) => wrong.push(format!("{text}\n  worked out, where its values are ones the recipe cannot take")),
            (Ok(ours), false) if same(&ours, &array(case)) => {}
            (Ok(ours), false) => wrong.push(format!("{text}\n  ours   {ours:?}\n  theirs {:?}", array(case))),
            (Err(e), false) => wrong.push(format!("{text}\n  failed: {e}")),
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ ({path}):\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
