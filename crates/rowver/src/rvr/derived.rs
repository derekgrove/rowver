//! The `derived` section: the columns a file keeps as the expressions that compute them, each
//! with the checksums and facts of its values. The records are checked with the header; the
//! values are worked out here from the whole columns they use, a million rows at a time, and
//! checked against the records.

use super::identity::exactly;
use super::{checksum, count, list, object, string, Held, Reader};
use crate::numeric::width;
use crate::{invalid, Result};
use rvr_recipe::{parse, per_event, reads, Array, Columns, Constant, Expression, Kind, Recipe, Scope, Store as Numbers, Values};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// Whether a kind is one a flat column, or a jagged column's content, may have.
fn flat(kind: &str) -> bool {
    kind != "jagged" && width(kind).is_ok()
}

/// Require one checksum per sample.
fn by_sample(value: &Value, samples: usize, what: &str) -> Result<()> {
    let sums = list(value, "by_sample")?;
    if sums.len() != samples {
        return Err(invalid(format!("{what}: by_sample needs one checksum per sample")));
    }
    for sum in sums {
        checksum(&serde_json::json!({ "sha256": sum }))?;
    }
    checksum(value).map(drop)
}

/// Require a `[low, high]` pair of numbers.
fn pair(value: &Value, key: &str, what: &str) -> Result<()> {
    match value[key].as_array().map(Vec::as_slice) {
        Some([low, high]) if low.is_number() && high.is_number() => Ok(()),
        _ => Err(invalid(format!("{what}: {key} must be two numbers"))),
    }
}

/// Check the derived section against the file's own header: every record's fields, and a name
/// no stored column, sample column or other derived column has.
pub(super) fn validate(derived: &Value, data: &Value) -> Result<()> {
    exactly(derived, &["columns"], &[], "derived")?;
    let samples = list(data, "samples")?.len();
    let mut names: HashSet<&str> = super::samples::RESERVED.into();
    for column in list(data, "cols")?.iter().chain(list(data, "sample_cols")?) {
        names.insert(string(column, "name")?);
    }
    for record in list(derived, "columns")? {
        let name = string(record, "name")?;
        let what = format!("derived column {name}");
        if name.is_empty() || !names.insert(name) {
            return Err(invalid(format!("{what}: a derived column needs a name no other column has")));
        }
        if string(record, "expression")?.trim().is_empty() {
            return Err(invalid(format!("{what} has no expression")));
        }
        match string(record, "kind")? {
            "jagged" => {
                exactly(record, &["name", "expression", "kind", "offsets", "content"], &[], &what)?;
                let offsets = object(record, "offsets")?;
                exactly(offsets, &["sha256", "by_sample"], &[], &format!("{what} offsets"))?;
                by_sample(offsets, samples, &what)?;
                let content = object(record, "content")?;
                exactly(content, &["kind", "count", "sha256", "by_sample"], &[], &format!("{what} content"))?;
                if !flat(string(content, "kind")?) {
                    return Err(invalid(format!("{what}: its content's kind is not one the format defines")));
                }
                count(content, "count")?;
                by_sample(content, samples, &what)?;
            }
            kind if flat(kind) => {
                let fields = ["name", "expression", "kind", "sha256", "by_sample", "lim", "nice", "vals", "names"];
                exactly(record, &fields, &["vals", "names"], &what)?;
                by_sample(record, samples, &what)?;
                pair(record, "lim", &what)?;
                pair(record, "nice", &what)?;
            }
            _ => return Err(invalid(format!("{what}: its kind is not one the format defines"))),
        }
    }
    Ok(())
}

/// How many rows are worked on at a time.
const RUN: usize = 1 << 20;

/// The metadata a press gives every sample beside the keys of samples.toml, which no expression sees.
pub use rvr_recipe::ADDED;

/// The metadata fields an expression can read: numbers, text, and truth values.
pub fn fields(metadata: &Map<String, Value>) -> BTreeMap<String, Constant> {
    let read = |value: &Value| match value {
        Value::Bool(truth) => Some(Constant::Truth(*truth)),
        Value::String(text) => Some(Constant::Text(text.clone())),
        Value::Number(number) => match number.as_i64() {
            Some(whole) if !number.to_string().contains(['.', 'e', 'E']) => Some(Constant::Whole(whole)),
            _ => number.as_f64().map(Constant::Float),
        },
        _ => None,
    };
    metadata.iter().filter_map(|(name, value)| Some((name.clone(), read(value)?))).collect()
}

/// One derived column's values over a run of `rows` rows of `columns`, with its sample's
/// `metadata`: refused when they are not finite, or not of the kind and shape `seen` holds.
pub fn evaluate(name: &str, expression: &Expression, columns: &dyn Columns, metadata: &BTreeMap<String, Constant>, rows: usize, seen: &mut Option<(Kind, bool)>) -> Result<Array> {
    let defines = BTreeMap::new();
    let array = Scope::new(columns, metadata, &defines)
        .evaluate(expression)
        .and_then(|value| per_event(value, rows))
        .map_err(|e| invalid(format!("derived column {name}: {}", e.0)))?;
    let (values, listed) = match &array {
        Array::Event(values) => (values, false),
        Array::Object { values, .. } => (values, true),
        Array::Constant(_) => unreachable!("per_event gives one value per row"),
    };
    if *seen.get_or_insert((values.kind, listed)) != (values.kind, listed) {
        return Err(invalid(format!("derived column {name} is not the same kind of column for every sample: give each sample's value the same type")));
    }
    if matches!(&values.store, Numbers::Float(numbers) if numbers.iter().any(|v| !v.is_finite())) {
        return Err(invalid(format!("derived column {name}: some of its values are not finite numbers (NaN or infinity, as a division by zero gives), and a file holds only finite ones: give those rows a finite value, as with if(…)")));
    }
    Ok(array)
}

/// Add one run's values, of the kind and shape of those before, to a column over every row.
fn add(whole: &mut Option<Held>, array: Array) {
    let (values, offsets) = match array {
        Array::Event(values) => (values, None),
        Array::Object { offsets, values } => (values, Some(offsets)),
        Array::Constant(_) => unreachable!("per_event gives one value per row"),
    };
    let held = whole.get_or_insert_with(|| Held {
        kind: values.kind.name().into(),
        bytes: Vec::new(),
        offsets: offsets.as_ref().map(|_| vec![0]),
    });
    let bytes = values.to_bytes();
    match (&mut held.offsets, &offsets) {
        // A run's lists need not begin at its first value: only the values they hold are kept.
        (Some(all), Some(run)) => {
            let (from, to) = (run.first().copied().unwrap_or(0), run.last().copied().unwrap_or(0));
            let w = values.kind.width();
            held.bytes.extend_from_slice(&bytes[from * w..to * w]);
            let base = all[all.len() - 1];
            all.extend(run.iter().skip(1).map(|offset| base + offset - from));
        }
        _ => held.bytes.extend(bytes),
    }
}

/// The columns of a file an expression can name: its stored columns, read when first asked for,
/// and its derived columns, worked out when first asked for.
struct Store<'a> {
    file: &'a Reader,
    derived: &'a [(String, String)],
    held: RefCell<BTreeMap<String, Rc<Held>>>,
    /// The derived columns being worked out, which none of them may use.
    working: RefCell<BTreeSet<String>>,
}

impl<'a> Store<'a> {
    fn new(file: &'a Reader, derived: &'a [(String, String)]) -> Self {
        let (held, working) = Default::default();
        Store { file, derived, held, working }
    }

    /// A column by name, or `None` when the file has no column of that name.
    fn get(&self, name: &str) -> Result<Option<Rc<Held>>> {
        if let Some(held) = self.held.borrow().get(name) {
            return Ok(Some(held.clone()));
        }
        let whole = if self.file.record(name).is_some() {
            let Some(held) = self.file.column(name)? else {
                return Ok(None);
            };
            held
        } else if let Some((_, expression)) = self.derived.iter().find(|(n, _)| n == name) {
            if !self.working.borrow_mut().insert(name.to_owned()) {
                return Err(invalid(format!("derived column {name} uses itself, directly or through other derived columns")));
            }
            let computed = self.compute(name, expression);
            self.working.borrow_mut().remove(name);
            computed?
        } else {
            return Ok(None);
        };
        let whole = Rc::new(whole);
        self.held.borrow_mut().insert(name.to_owned(), whole.clone());
        Ok(Some(whole))
    }

    /// Let go of every column held that working out `rest` does not need.
    fn keep_for(&self, rest: &[String]) {
        let mut needed = BTreeSet::new();
        let mut todo: Vec<String> = rest.to_vec();
        while let Some(name) = todo.pop() {
            if !needed.insert(name.clone()) || self.held.borrow().contains_key(&name) {
                continue;
            }
            if let Some(parsed) = self.derived.iter().find(|(n, _)| *n == name).and_then(|(_, e)| parse(e).ok()) {
                let mut used = BTreeSet::new();
                reads(&parsed, &mut used);
                todo.extend(used);
            }
        }
        self.held.borrow_mut().retain(|name, _| needed.contains(name));
    }

    /// Work one derived column out over every row, a file's rows at a time, each with its
    /// sample's metadata and its file, as a press gives them.
    fn compute(&self, name: &str, expression: &str) -> Result<Held> {
        let parsed = parse(expression).map_err(|e| invalid(format!("derived column {name}: {}", e.0)))?;
        let header = self.file.header();
        let mut runs = Vec::new();
        let mut first = 0;
        for read in header["data"]["files"].as_array().into_iter().flatten() {
            let rows = read["rows"].as_u64().unwrap_or(0) as usize;
            let sample = read["sample"].as_str().unwrap_or_default();
            let path = read["path"].as_str().unwrap_or_default();
            let mut at = first;
            while at < first + rows {
                let to = (at + RUN).min(first + rows);
                runs.push((at, to, sample, path));
                at = to;
            }
            first += rows;
        }
        // A file with no rows still says what kind of column it is.
        if runs.is_empty() {
            let sample = header["data"]["samples"][0]["name"].as_str().unwrap_or_default();
            runs.push((0, 0, sample, ""));
        }
        let (mut whole, mut seen) = (None, None);
        for (first, last, sample, path) in runs {
            let metadata = &header["made"]["files"][sample]["metadata"];
            let mut metadata = metadata.as_object().map(fields).unwrap_or_default();
            metadata.retain(|key, _| !ADDED.contains(&key.as_str()));
            metadata.insert("file".into(), Constant::Text(path.into()));
            let run = Run { store: self, first, last };
            let values = evaluate(name, &parsed, &run, &metadata, last - first, &mut seen);
            add(&mut whole, values.map_err(|e| invalid(format!("sample {sample}: {e}")))?);
        }
        whole.ok_or_else(|| invalid(format!("derived column {name} has no values")))
    }
}

/// Rows `first..last` of a file's columns, as an expression reads them.
struct Run<'s, 'a> {
    store: &'s Store<'a>,
    first: usize,
    last: usize,
}

impl Columns for Run<'_, '_> {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        let Some(whole) = self.store.get(name).map_err(|e| rvr_recipe::Error(e.to_string()))? else {
            return Ok(None);
        };
        let kind = Kind::named(&whole.kind)?;
        let w = kind.width();
        Ok(Some(match &whole.offsets {
            None => Array::Event(Values::from_bytes(kind, &whole.bytes[self.first * w..self.last * w])),
            Some(offsets) => {
                let (from, to) = (offsets[self.first], offsets[self.last]);
                let lists = offsets[self.first..=self.last].iter().map(|o| o - from);
                Array::Object {
                    offsets: Arc::new(lists.collect()),
                    values: Values::from_bytes(kind, &whole.bytes[from * w..to * w]),
                }
            }
        }))
    }

    fn has(&self, name: &str) -> bool {
        self.store.file.record(name).is_some() || self.store.derived.iter().any(|(n, _)| n == name)
    }
}

/// Each sample's rows, in order.
fn sample_rows(header: &Value) -> Vec<usize> {
    let samples = header["data"]["samples"].as_array().into_iter().flatten();
    samples.map(|s| s["rows"].as_u64().unwrap_or(0) as usize).collect()
}

/// A file's derived columns, by name and expression, as its header records them.
pub fn listed(header: &Value) -> Vec<(String, String)> {
    let records = header["derived"]["columns"].as_array().into_iter().flatten();
    let text = |r: &Value, key: &str| r[key].as_str().unwrap_or_default().to_owned();
    records.map(|r| (text(r, "name"), text(r, "expression"))).collect()
}

/// The records of `derived`: one of `before` with the same name and expression, or worked out.
pub fn records(file: &Reader, derived: &[(String, String)], before: &[Value]) -> Result<Vec<Value>> {
    let store = Store::new(file, derived);
    let rows = sample_rows(file.header());
    let names: Vec<String> = derived.iter().map(|(name, _)| name.clone()).collect();
    let mut out = Vec::new();
    for (at, (name, expression)) in derived.iter().enumerate() {
        store.keep_for(&names[at..]);
        let same = |r: &&Value| r["name"] == name.as_str() && r["expression"] == expression.as_str();
        if let Some(kept) = before.iter().find(same) {
            out.push(kept.clone());
            continue;
        }
        let whole = store.get(name)?.ok_or_else(|| invalid(format!("derived column {name} has no values")))?;
        let record = crate::write::derived(name, expression, &whole, &rows).map_err(|e| invalid(format!("derived column {e}")))?;
        out.push(record);
    }
    Ok(out)
}

/// Derived columns of a file, worked out and each checked against the file's record of it.
pub fn columns(file: &Reader, names: &[String]) -> Result<Vec<Held>> {
    columns_while(file, names, &|| Ok(()))
}

/// `columns`, asking `go` before each one whether to carry on.
fn columns_while(file: &Reader, names: &[String], go: &dyn Fn() -> Result<()>) -> Result<Vec<Held>> {
    let header = file.header();
    let derived = listed(header);
    let store = Store::new(file, &derived);
    let rows = sample_rows(header);
    let records = header["derived"]["columns"].as_array();
    let mut wholes = Vec::new();
    for (at, name) in names.iter().enumerate() {
        go()?;
        store.keep_for(&names[at..]);
        let record = records
            .into_iter()
            .flatten()
            .find(|r| r["name"] == name.as_str())
            .ok_or_else(|| invalid(format!("the file has no derived column named {name}")))?;
        let whole = store.get(name)?.ok_or_else(|| invalid(format!("derived column {name} has no values")))?;
        let expression = record["expression"].as_str().unwrap_or_default();
        let again = crate::write::derived(name, expression, &whole, &rows).map_err(|e| invalid(format!("derived column {e}")))?;
        if again != *record {
            // Quoted for a shell: an expression may hold quotes of its own, as branch('Jet.pt').
            let quoted = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
            let (path, line) = (quoted(&file.path().display().to_string()), quoted(&format!("{name} = {expression}")));
            return Err(invalid(format!("derived column {name}, worked out here, is not what the file records of it (its checksums or facts differ): its header was changed, or the rowver that wrote it works the expression out differently. rvr derive {path} --drop {name} (with any derived column that uses it), then rvr derive {path} {line}, records it as this rowver works it out.")));
        }
        wholes.push(whole);
    }
    // The values are handed out, not copied, once nothing else holds them.
    drop(store);
    Ok(wholes.into_iter().map(|whole| Rc::try_unwrap(whole).unwrap_or_else(|shared| (*shared).clone())).collect())
}

/// Check a file's derived columns against the recipe it carries and its records; how many.
pub fn checked(file: &Reader) -> Result<usize> {
    checked_while(file, &|| Ok(()))
}

/// `checked`, asking `go` before each derived column whether to carry on.
pub(super) fn checked_while(file: &Reader, go: &dyn Fn() -> Result<()>) -> Result<usize> {
    let derived = listed(file.header());
    if agreed(file.header())? == 0 {
        return Ok(0);
    }
    let names: Vec<String> = derived.iter().map(|(name, _)| name.clone()).collect();
    columns_while(file, &names, go)?;
    Ok(derived.len())
}

/// Check that the recipe a header carries lists its derived columns; how many there are.
pub(super) fn agreed(header: &Value) -> Result<usize> {
    let derived = listed(header);
    // A recipe this rowver cannot read is checked only when there are derived columns to work out.
    match header["made"]["recipe"]["text"].as_str().map(Recipe::read) {
        Some(Ok(recipe)) if recipe.derived != derived => {
            return Err(invalid("the [derived] table of the recipe it carries does not list the derived columns its header records: one of the two was edited after it was written. Press its study again."))
        }
        Some(Err(e)) if !derived.is_empty() => {
            return Err(invalid(format!("the recipe it carries cannot be read ({}), so its derived columns cannot be worked out: press its study again", e.0)))
        }
        None if !derived.is_empty() => return Err(invalid("it has derived columns but carries no recipe to say what they are")),
        _ => {}
    }
    Ok(derived.len())
}
