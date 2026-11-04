//! The `derived` section: the columns a file keeps as the expressions that compute them, each
//! with the checksums and facts of its values. The records are checked with the header; the
//! values are worked out here from the whole columns they use, a run of rows at a time, and
//! checked against the records.

use super::{exactly, list, nonempty, text, Held, Reader, MEND};
use crate::{invalid, Result};
use rvr_recipe::{parse, per_event, reads, Array, Columns, Constant, Expression, Kind, Recipe, Scope, Values};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// Check the derived section against the file's own header (docs/FORMAT.md §10): one derived
/// column or more, each record's fields, a name no stored, sample or other derived column has,
/// an expression, a kind, and the forms of its checksums and facts.
pub(super) fn validate(derived: &Value, data: &Value) -> Result<()> {
    exactly(derived, "derived", &["columns"], &[])?;
    let records = list(derived, "derived", "columns")?;
    if records.is_empty() {
        return Err(invalid("derived.columns is empty, and a file with no derived column has no derived key (docs/FORMAT.md §10): take it out"));
    }
    let samples = list(data, "data", "samples")?.len();
    let mut names: HashSet<&str> = super::samples::RESERVED.into();
    for column in list(data, "data", "cols")?.iter().chain(list(data, "data", "sample_cols")?) {
        names.insert(column["name"].as_str().unwrap_or_default());
    }
    for (index, record) in records.iter().enumerate() {
        let name = nonempty(record, &format!("derived.columns[{index}]"), "name")?;
        let place = format!("derived.columns[{name:?}]");
        if !names.insert(name) {
            return Err(invalid(format!("{place}: a derived column's name is one no stored, sample or other derived column has, nor sample, entry or file (docs/FORMAT.md §10): rename it with rvr derive")));
        }
        if text(record, &place, "expression")?.trim().is_empty() {
            return Err(invalid(format!("{place}.expression is blank, and a derived column is kept as the expression that computes it (docs/FORMAT.md §10): {MEND}")));
        }
        super::columns::record_of(record, &place, &["name", "expression"], false, samples)?;
    }
    Ok(())
}

/// How many rows are worked on at a time (fewer where their pairs would be too many): no value
/// depends on it, only how much is held at once.
const RUN: usize = 1 << 20;

/// The metadata fields an expression can read: numbers, text, and truth values.
pub fn fields(metadata: &Map<String, Value>) -> BTreeMap<String, Constant> {
    let read = |value: &Value| match value {
        Value::Bool(truth) => Some(Constant::Truth(*truth)),
        Value::String(text) => Some(Constant::Text(text.clone())),
        Value::Number(number) => match number.as_i64() {
            Some(whole) if crate::json::whole(number) => Some(Constant::Whole(whole)),
            _ => number.as_f64().map(Constant::Float),
        },
        _ => None,
    };
    metadata.iter().filter_map(|(name, value)| Some((name.clone(), read(value)?))).collect()
}

/// `metadata` with each total of `recipe` standing in as 1, as a derived column is tried before every event is read.
pub fn standing_in(metadata: &mut BTreeMap<String, Constant>, recipe: &Recipe) {
    for total in &recipe.totals {
        metadata.entry(total.name.clone()).or_insert(Constant::Float(1.0));
    }
}

/// A derived column's values over the `rows` rows of `columns`, worked out a run at a time; a data error names its row.
pub fn evaluate(expression: &Expression, columns: &dyn Columns, metadata: &BTreeMap<String, Constant>, rows: usize) -> rvr_recipe::Result<Array> {
    let defines = BTreeMap::new();
    let mut work = |run: &dyn Columns, from: usize, to: usize| {
        Scope::new(run, metadata, &defines).evaluate(expression).and_then(|value| per_event(value, to - from))
    };
    rvr_recipe::over_runs(columns, rows, &mut work).map_err(|(e, _)| e).and_then(Array::joined)
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
    /// How a column an expression asked for failed, said as it is rather than as the expression's.
    failed: RefCell<Option<crate::Error>>,
    /// Whether every column is worked out over no rows, as for an empty file.
    hollow: bool,
}

impl<'a> Store<'a> {
    fn new(file: &'a Reader, derived: &'a [(String, String)]) -> Self {
        let (held, working, failed) = Default::default();
        Store { file, derived, held, working, failed, hollow: false }
    }

    /// A stored column of no rows, of the kind its record gives.
    fn none_of(record: &Value) -> Held {
        let jagged = record["kind"] == "jagged";
        let kind = if jagged { &record["content"]["kind"] } else { &record["kind"] };
        Held { kind: kind.as_str().unwrap_or_default().into(), bytes: Vec::new(), offsets: jagged.then(|| vec![0]) }
    }

    /// What a refusal calls derived column `name`: its place in the header, or for one being
    /// added, its name.
    fn place(&self, name: &str) -> String {
        match listed(self.file.header()).iter().any(|(n, _)| n == name) {
            true => format!("derived.columns[{name:?}]"),
            false => format!("derived column {name}"),
        }
    }

    /// A column by name, or `None` when the file has no column of that name.
    fn get(&self, name: &str) -> Result<Option<Rc<Held>>> {
        if let Some(held) = self.held.borrow().get(name) {
            return Ok(Some(held.clone()));
        }
        let whole = if let Some(record) = self.file.record(name) {
            match self.hollow {
                true => Self::none_of(record),
                false => self.file.stored(name, record)?,
            }
        } else if let Some((_, expression)) = self.derived.iter().find(|(n, _)| n == name) {
            if !self.working.borrow_mut().insert(name.to_owned()) {
                return Err(invalid(format!("{} uses itself, directly or through other derived columns: rvr derive drops one of them", self.place(name))));
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

    /// A column the file has, stored or derived.
    fn had(&self, name: &str) -> Result<Rc<Held>> {
        Ok(self.get(name)?.expect("a derived column is worked out"))
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
    /// sample's metadata and its file, as a press gives them. A column it uses that failed is said
    /// as it failed; its own expression failing, in a column a newer rvr recorded, says to update.
    fn compute(&self, name: &str, expression: &str) -> Result<Held> {
        self.worked_out(name, expression).map_err(|e| {
            if let Some(failed) = self.failed.borrow_mut().take() {
                return failed;
            }
            let header = self.file.header();
            let recorded = listed(header).iter().any(|(n, x)| n == name && x == expression);
            match unread(header).filter(|_| recorded) {
                Some(hint) => invalid(format!("{e}. {hint}")),
                None => e,
            }
        })
    }

    /// `compute`, before its failure is told.
    fn worked_out(&self, name: &str, expression: &str) -> Result<Held> {
        let header = self.file.header();
        let place = self.place(name);
        let parsed = parse(expression).map_err(|e| invalid(format!("{place}: {}", e.0)))?;
        let mut runs = Vec::new();
        let mut first = 0;
        for read in header["data"]["files"].as_array().into_iter().flatten() {
            let rows = super::whole_count(&read["rows"]).unwrap_or(0) as usize;
            let sample = read["sample"].as_str().unwrap_or_default();
            let path = read["path"].as_str().unwrap_or_default();
            // Over no rows, the runs a whole file has, one for each file that has rows.
            if self.hollow {
                if rows > 0 {
                    runs.push((0, 0, sample, path));
                }
                continue;
            }
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
            metadata.retain(|key, _| !rvr_recipe::ADDED.contains(&key.as_str()));
            metadata.insert("file".into(), Constant::Text(path.into()));
            let run = Run { store: self, first, last, held: RefCell::default() };
            let values = evaluate(&parsed, &run, &metadata, last - first).map_err(|e| match e.1 {
                Some(at) => invalid(format!("sample {sample}: {place}, row {}: {}", first + at, e.0)),
                None => invalid(format!("sample {sample}: {place}: {}", e.0)),
            })?;
            let shape = values.shape().expect("per_event gives one value per row");
            if *seen.get_or_insert(shape) != shape {
                return Err(invalid(format!("sample {sample}: {place} is not the same kind of column for every sample: give each sample's value the same type")));
            }
            add(&mut whole, values);
        }
        Ok(whole.expect("one run or more is worked out"))
    }
}

/// For a file a newer rvr pressed than this version: what to do, said after what failed.
fn newer(header: &Value) -> Option<String> {
    let made = header["made"]["toolchain"]["rvr"].as_str()?;
    let this = env!("CARGO_PKG_VERSION");
    later(made, this).then(|| {
        format!("The file was made by rvr {made}, newer than this one ({this}), and may use what only a newer version reads: update rvr and rowver to {made} or later (pip install -U rowver updates both; or that release's rvr download, or rowver crate).")
    })
}

/// What a newer file's derived column that cannot be worked out here adds to its failure.
fn unread(header: &Value) -> Option<String> {
    newer(header).map(|update| format!("{update} Its stored columns still read, each asked for by name."))
}

/// What to do when a file's recipe or derived columns cannot be worked out here.
pub fn remedy(header: &Value) -> String {
    newer(header).unwrap_or_else(|| "Press its study again.".into())
}

/// Whether version `made` comes after `this`, each MAJOR.MINOR.PATCH (a part that is no number
/// counts as 0), by their three numbers.
fn later(made: &str, this: &str) -> bool {
    let numbers = |version: &str| version.split('.').take(3).map(|part| part.parse().unwrap_or(0)).collect::<Vec<u64>>();
    numbers(made) > numbers(this)
}

/// Rows `first..last` of a file's columns, as an expression reads them: each read from the file's
/// bytes once, when first asked for (`rvr_recipe::over_runs` slices a part of the run from it).
struct Run<'s, 'a> {
    store: &'s Store<'a>,
    first: usize,
    last: usize,
    held: RefCell<BTreeMap<String, Option<Array>>>,
}

impl Columns for Run<'_, '_> {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        if let Some(held) = self.held.borrow().get(name) {
            return Ok(held.clone());
        }
        let array = self.read(name)?;
        self.held.borrow_mut().insert(name.to_owned(), array.clone());
        Ok(array)
    }

    fn has(&self, name: &str) -> bool {
        self.store.file.record(name).is_some() || self.store.derived.iter().any(|(n, _)| n == name)
    }
}

impl Run<'_, '_> {
    /// The column's rows of the run, from the bytes the file holds of it.
    fn read(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        let got = self.store.get(name).map_err(|e| {
            let said = rvr_recipe::problem(e.to_string());
            self.store.failed.borrow_mut().get_or_insert(e);
            said
        });
        let Some(whole) = got? else {
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
}

/// Each sample's rows, in order.
fn sample_rows(header: &Value) -> Vec<usize> {
    let samples = header["data"]["samples"].as_array().into_iter().flatten();
    samples.map(|s| super::whole_count(&s["rows"]).unwrap_or(0) as usize).collect()
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
        let whole = store.had(name)?;
        let record = crate::write::derived(name, expression, &whole, &rows).map_err(|e| invalid(format!("derived column {e}")))?;
        out.push(record);
    }
    Ok(out)
}

/// Derived columns of a file, worked out and each checked against the file's record of it.
pub fn columns(file: &Reader, names: &[String]) -> Result<Vec<Held>> {
    columns_with(file, names, &[])
}

/// `columns`, worked out from the stored columns `held` as they are, reading only the others.
pub fn columns_with(file: &Reader, names: &[String], held: &[(String, Rc<Held>)]) -> Result<Vec<Held>> {
    let header = file.header();
    let derived = listed(header);
    let store = Store::new(file, &derived);
    store.held.borrow_mut().extend(held.iter().cloned());
    let rows = sample_rows(header);
    let records = header["derived"]["columns"].as_array();
    let mut wholes = Vec::new();
    for (at, name) in names.iter().enumerate() {
        store.keep_for(&names[at..]);
        let record = records
            .into_iter()
            .flatten()
            .find(|r| r["name"] == name.as_str())
            .ok_or_else(|| invalid(format!("the file has no derived column named {name}")))?;
        let place = store.place(name);
        let whole = store.had(name)?;
        let expression = record["expression"].as_str().unwrap_or_default();
        let again = crate::write::derived(name, expression, &whole, &rows).map_err(|e| invalid(format!("derived column {e}")))?;
        // A jagged record's only number is its content's count.
        let numbers = match record["kind"].as_str().unwrap_or_default() {
            "jagged" => super::facts::Numbers::Whole,
            kind => super::facts::numbers(kind),
        };
        if !super::facts::same(&again, record, numbers) {
            let differs = format!("{place}, worked out here, is not what the file records of it (its checksums or facts differ)");
            // A newer rvr's record is not to be replaced by this rowver's: it says to update too.
            if let Some(hint) = unread(header) {
                return Err(invalid(format!("{differs}: its header was changed, or a newer rvr works the expression out otherwise. {hint}")));
            }
            // Quoted for a shell: an expression may hold quotes of its own, as branch('Jet.pt').
            let quoted = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
            let (path, line) = (quoted(&file.path().display().to_string()), quoted(&format!("{name} = {expression}")));
            return Err(invalid(format!("{differs}: its header was changed, or the rowver that wrote it works the expression out differently. rvr derive {path} --drop {name} (with any derived column that uses it), then rvr derive {path} {line}, records it as this rowver works it out.")));
        }
        wholes.push(whole);
    }
    // The values are handed out, not copied, once nothing else holds them.
    drop(store);
    Ok(wholes.into_iter().map(|whole| Rc::try_unwrap(whole).unwrap_or_else(|shared| (*shared).clone())).collect())
}

/// An empty file's derived columns, worked out over no rows: each of its record's kind and shape
/// (its checksums and facts are of the rows the file no longer holds); how many.
pub(super) fn checked_empty(file: &Reader) -> Result<usize> {
    let derived = listed(file.header());
    let mut store = Store::new(file, &derived);
    store.hollow = true;
    let records = file.header()["derived"]["columns"].as_array().map_or(&[][..], Vec::as_slice);
    for ((name, _), record) in derived.iter().zip(records) {
        let whole = store.had(name)?;
        let want = Store::none_of(record);
        if (&whole.kind, whole.offsets.is_some()) != (&want.kind, want.offsets.is_some()) {
            let shape = |held: &Held| if held.offsets.is_some() { format!("jagged of {}", held.kind) } else { held.kind.clone() };
            return Err(invalid(format!("derived.columns[{name:?}] is {}, and its expression, worked out over no rows, gives {} (docs/FORMAT.md §10): {MEND}", shape(&want), shape(&whole))));
        }
    }
    Ok(derived.len())
}

/// Check a file's derived columns against the recipe it carries and its records; how many.
pub fn checked(file: &Reader) -> Result<usize> {
    let derived = listed(file.header());
    if agreed(file.header())? == 0 {
        return Ok(0);
    }
    let names: Vec<String> = derived.iter().map(|(name, _)| name.clone()).collect();
    columns(file, &names)?;
    Ok(derived.len())
}

/// Check that the recipe a header carries lists its derived columns; how many there are.
pub(super) fn agreed(header: &Value) -> Result<usize> {
    let derived = listed(header);
    // A recipe this rowver cannot read is checked only when there are derived columns to work out.
    match header["made"]["recipe"]["text"].as_str().map(Recipe::read) {
        Some(Ok(recipe)) if recipe.derived != derived => {
            return Err(invalid(format!("the [derived] table of the recipe it carries does not list the derived columns its header records: one of the two was edited after it was written. {}", remedy(header))))
        }
        Some(Err(e)) if !derived.is_empty() => {
            return Err(invalid(format!("the recipe it carries cannot be read ({}), so its derived columns cannot be worked out. {}", e.line(), remedy(header))))
        }
        None if !derived.is_empty() => return Err(invalid(format!("it has derived columns but carries no recipe to say what they are. {}", remedy(header)))),
        _ => {}
    }
    Ok(derived.len())
}

#[cfg(test)]
mod tests {
    use super::later;

    #[test]
    fn a_version_is_later_by_its_three_numbers() {
        assert!(later("15.1.0", "15.0.0") && later("16.0.0", "15.9.9") && later("15.0.10", "15.0.9"));
        assert!(!later("15.0.0", "15.0.0") && !later("15.0.0", "15.1.0"));
    }
}
