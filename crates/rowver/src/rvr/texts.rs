//! The study a file carries (docs/FORMAT.md §6, §6.3), in full validation: its three texts parsed
//! and checked against each other and the file — the recipe's columns, kinds, derived columns,
//! title and totals; the lock resolved from the samples.toml; the samples, labels, files, trees,
//! UUIDs, entries and metadata against the lock.

use super::identity::Reads;
use super::facts::{same, Numbers};
use super::MEND;
use crate::choice::Events;
use crate::study::lock::{self, Lock};
use crate::{invalid, Result};
use rvr_recipe::{per_event, Array, Columns, Constant, Kind, Recipe, Scope, Values};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Check the three texts and the file against each other; `reads` is what each file's choice read.
pub(super) fn validate(header: &Value, reads: &Reads) -> Result<()> {
    let (data, made) = (&header["data"], &header["made"]);
    let source = made["recipe"]["text"].as_str().unwrap_or_default();
    let recipe = Recipe::read(source).map_err(|e| {
        invalid(format!("made.recipe.text cannot be read as a recipe the recipe reference accepts ({}). {}", e.0, super::derived::remedy(header)))
    })?;
    let root: toml::Table = source.parse().map_err(|e| invalid(format!("made.recipe.text is not TOML ({e}): {MEND}")))?;
    if let Some(table) = super::RECIPE_TABLES.iter().find(|table| !root.get(**table).is_some_and(toml::Value::is_table)) {
        return Err(invalid(format!("made.recipe.text has no [{table}] table, and a recipe writes all five, even empty (docs/FORMAT.md §6): {MEND}")));
    }
    let stored: Vec<&str> = recipe.columns.iter().map(|(name, _)| name.as_str()).collect();
    if stored != super::snapshot::pressed(made) {
        return Err(invalid(format!("made.recipe.columns is {}, and the recipe's [columns] stores {stored:?}, in that order (docs/FORMAT.md §6): {MEND}", made["recipe"]["columns"])));
    }
    if recipe.derived != super::derived::listed(header) {
        return Err(invalid(format!("the [derived] table of made.recipe.text does not list the derived columns its header records, by the same names and expressions in the same order (docs/FORMAT.md §10): one of the two was edited after it was written. {}", super::derived::remedy(header))));
    }
    if recipe.title.as_deref() != made.get("title").and_then(Value::as_str) {
        return Err(invalid(format!("made.title is {}, and it is the recipe's title, there exactly when the recipe sets one (docs/FORMAT.md §6): {MEND}", made.get("title").unwrap_or(&Value::Null))));
    }
    let one_line = |p: crate::study::Problems| p.to_string().replace('\n', "; ");
    let toml_text = made["samples"]["text"].as_str().unwrap_or_default();
    crate::study::samples::read(toml_text).map_err(|p| invalid(format!("made.samples.text is not a samples.toml docs/SPEC.md §3 accepts: {}. {MEND}", one_line(p))))?;
    let lock_text = made["lock"]["text"].as_str().unwrap_or_default();
    let lock = Lock::read(lock_text).map_err(|p| invalid(format!("made.lock.text is not a samples.lock docs/FORMAT.md §6.3 accepts: {}. {MEND}", one_line(p))))?;
    lock::agree(toml_text, &lock).map_err(|p| invalid(format!("made.lock.text was not resolved from made.samples.text: {}. {MEND}", one_line(p))))?;
    samples(header, &lock, &recipe, reads)?;
    columns(data, &lock, &recipe)
}

/// Check `data` and `made.files` against the lock, sample by sample.
fn samples(header: &Value, lock: &Lock, recipe: &Recipe, reads: &Reads) -> Result<()> {
    let (data, made) = (&header["data"], &header["made"]);
    let held = data["samples"].as_array().map_or(&[][..], Vec::as_slice);
    let names = |samples: Vec<(&str, &str)>| samples.iter().map(|(n, l)| format!("{n} ({l})")).collect::<Vec<_>>().join(", ");
    let file_samples: Vec<(&str, &str)> = held.iter().map(|s| (s["name"].as_str().unwrap_or_default(), s["label"].as_str().unwrap_or_default())).collect();
    let lock_samples: Vec<(&str, &str)> = lock.samples.iter().map(|s| (s.name.as_str(), s.label.as_str())).collect();
    if file_samples != lock_samples {
        return Err(invalid(format!("data.samples holds {}, and it holds the lock's samples, in its order, with their names and labels: {} (docs/FORMAT.md §3): {MEND}", names(file_samples), names(lock_samples))));
    }
    let totals: Vec<&str> = recipe.totals.iter().map(|t| t.name.as_str()).collect();
    let holed = header["made"]["files"].as_object().into_iter().flatten().any(|(_, s)| s["files"].as_object().into_iter().flatten().any(|(_, f)| f.get("holes").is_some()));
    for locked in &lock.samples {
        let name = locked.name.as_str();
        if let Some(total) = totals.iter().find(|t| locked.metadata.contains_key(**t)) {
            return Err(invalid(format!("the recipe's total {total} has the name of a key of sample {name}'s metadata, and no key of samples.toml's metadata has a total's name (docs/FORMAT.md §5): rename the total or the key, resolve and press again")));
        }
        let place = format!("made.files[{name:?}]");
        let source = &made["files"][name];
        // A first-N choice leaves out each file with entries that none of the N was left for.
        let mut left = match Events::read(&source["events"])? {
            Events::First(n) => n,
            _ => u64::MAX,
        };
        let want: Vec<&lock::File> = locked.files.iter().filter(|f| {
            let read = f.entries.min(left);
            left -= read;
            read > 0 || f.entries == 0
        }).collect();
        let paths: Vec<&str> = data["files"].as_array().into_iter().flatten().filter(|f| f["sample"] == name).filter_map(|f| f["path"].as_str()).collect();
        let wanted: Vec<&str> = want.iter().map(|f| f.at.as_str()).collect();
        if paths != wanted {
            return Err(invalid(format!("data.files lists of sample {name} {paths:?}, and it lists the lock's files of the sample in its order, leaving out each one the first N never reached (its stop would be 0): {wanted:?} (docs/FORMAT.md §3): {MEND}")));
        }
        for file in want {
            let at = format!("{place}.files[{:?}]", file.at);
            let record = &source["files"][file.at.as_str()];
            if record["tree"] != locked.tree.as_str() {
                return Err(invalid(format!("{at}.tree is {}, and it is the lock's tree for the sample, {:?} (docs/FORMAT.md §6): {MEND}", record["tree"], locked.tree)));
            }
            if record["uuid"] != file.uuid.as_str() {
                return Err(invalid(format!("{at}.uuid is {}, and it is the UUID the lock gives the file, {:?} (docs/FORMAT.md §6): {MEND}", record["uuid"], file.uuid)));
            }
            if super::whole_count(&record["entries"]) != Some(file.entries) {
                return Err(invalid(format!("{at}.entries is {}, and it is the entries the lock gives the file, {} (docs/FORMAT.md §6): {MEND}", record["entries"], file.entries)));
            }
            let (read, holes) = &reads[&(name, file.at.as_str())];
            let some = super::identity::read_file(read, holes);
            let mut held: Vec<&str> = record.get("totals").and_then(Value::as_object).map(|t| t.keys().map(String::as_str).collect()).unwrap_or_default();
            let mut want_totals = if some { totals.clone() } else { Vec::new() };
            held.sort_unstable();
            want_totals.sort_unstable();
            if held != want_totals {
                return Err(invalid(format!("{at}.totals holds {held:?}, and a file the press read holds each total of the recipe's [totals], and one it did not read none: {want_totals:?} (docs/FORMAT.md §6): {MEND}")));
            }
        }
        let metadata = source["metadata"].as_object().cloned().unwrap_or_default();
        let mut keys: Vec<&str> = locked.metadata.keys().map(String::as_str).chain(["events_read", "events_total"]).collect();
        keys.extend(holed.then_some("events_missing"));
        keys.extend(&totals);
        let mut have: Vec<&str> = metadata.keys().map(String::as_str).collect();
        have.sort_unstable();
        keys.sort_unstable();
        if have != keys {
            return Err(invalid(format!("{place}.metadata holds the keys {have:?}, and it holds the lock's metadata of the sample, events_read, events_total, events_missing when the file has holes, and each total: {keys:?} (docs/FORMAT.md §6): {MEND}")));
        }
        if let Some((key, value)) = locked.metadata.iter().find(|(key, value)| !same(value, &metadata[key.as_str()], Numbers::Written)) {
            return Err(invalid(format!("{place}.metadata.{key} is {}, and it is the lock's value, {value} (docs/FORMAT.md §6): {MEND}", metadata[key.as_str()])));
        }
        if super::whole_count(&metadata["events_total"]) != Some(locked.events_total) {
            return Err(invalid(format!("{place}.metadata.events_total is {}, and it is the entries of all the sample's files in the lock, {} (docs/FORMAT.md §5): {MEND}", metadata["events_total"], locked.events_total)));
        }
        let read_any = source["files"].as_object().into_iter().flatten().any(|(_, f)| f.get("totals").is_some());
        for total in totals.iter().filter(|_| !read_any) {
            let at = format!("{place}.metadata.{total}");
            if super::identity::total(&metadata[*total], &at)? != 0.0 {
                return Err(invalid(format!("{at} is {}, and no file of the sample holds that total (the press read none of them), so it is 0.0 (docs/FORMAT.md §6): {MEND}", metadata[*total])));
            }
        }
    }
    Ok(())
}

/// The branches of one sample's tree, as the lock lists them, with no events: what an expression
/// of the recipe is worked out over to find the kind of each column.
struct Branches<'a>(BTreeMap<&'a str, (Kind, bool)>);

impl Columns for Branches<'_> {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        Ok(self.0.get(name).map(|&(kind, listed)| {
            let values = Values::from_bytes(kind, &[]);
            match listed {
                true => Array::Object { offsets: Arc::new(vec![0]), values },
                false => Array::Event(values),
            }
        }))
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

/// Each `[columns]` column's kind and whether it is a list per row, worked out over every sample's branch list (SPEC.md §2.3).
pub fn kinds(recipe: &Recipe, lock: &Lock) -> Result<Vec<(Kind, bool)>> {
    let mut found: Vec<Option<(Kind, bool, &str)>> = vec![None; recipe.columns.len()];
    let shape = |kind: Kind, listed: bool| if listed { format!("a list of {}", kind.name()) } else { kind.name().to_owned() };
    for sample in &lock.samples {
        let list = lock.branches_of(sample).ok_or_else(|| invalid(format!("samples.lock sample {:?} names a branch list it does not hold: resolve again", sample.name)))?;
        let branches = Branches(list.list.iter().filter_map(|b| Some((b.name.as_str(), (Kind::named(&b.kind).ok()?, b.shape != lock::Shape::Flat)))).collect());
        let mut metadata: BTreeMap<String, Constant> = super::derived::fields(&sample.metadata);
        metadata.insert("file".into(), Constant::Text(sample.files.first().map(|f| f.at.clone()).unwrap_or_default()));
        let scope = Scope::new(&branches, &metadata, &recipe.defines);
        let fails = |what: String| move |e: rvr_recipe::Error| {
            let near = e.0.strip_prefix("There is no column named ").map(|name| crate::study::meant(name, list.list.iter().map(|b| b.name.as_str())));
            let said = if e.0.ends_with(['.', '?']) { e.0 } else { format!("{}.", e.0) };
            invalid(format!("{what}: {said}{} (sample {}, over the branches samples.lock lists for it)", near.unwrap_or_default(), sample.name))
        };
        for cut in &recipe.keep {
            // A cut whose samples (`when`) leave this one out is not worked out for it.
            let applies = match &cut.when {
                None => true,
                Some(when) => !matches!(scope.evaluate(when).map_err(fails(format!("[keep] {}", cut.name)))?, Array::Constant(Constant::Truth(false) | Constant::Missing(_))),
            };
            if applies {
                scope.evaluate(&cut.pass).and_then(|v| per_event(v, 0)).map_err(fails(format!("[keep] {}", cut.name)))?;
            }
        }
        for total in recipe.totals.iter().filter(|total| total.tree.is_none()) {
            scope.evaluate(&total.value).and_then(|v| per_event(v, 0)).map_err(fails(format!("[totals] {}", total.name)))?;
        }
        for ((name, expression), held) in recipe.columns.iter().zip(&mut found) {
            let array = scope.evaluate(expression).and_then(|v| per_event(v, 0)).map_err(fails(format!("[columns] {name}")))?;
            let (values, listed) = match &array {
                Array::Event(values) => (values, false),
                Array::Object { values, .. } => (values, true),
                Array::Constant(_) => return Err(invalid(format!("[columns] {name} is a fixed value, not a column: give it a value for each event"))),
            };
            let kind = recipe.stored.get(name).copied().unwrap_or(values.kind);
            match *held {
                None => *held = Some((kind, listed, &sample.name)),
                Some((first, was, before)) if (first, was) != (kind, listed) => {
                    return Err(invalid(format!("[columns] {name} is {} in sample {before} and {} in sample {}, as their branch lists in samples.lock give, and a column has one kind in every file: name the samples' branches apart, or convert it in the recipe, as {name} = {{ value = \"…\", stored = \"f8\" }}", shape(first, was), shape(kind, listed), sample.name)));
                }
                _ => {}
            }
        }
    }
    Ok(found.into_iter().map(|held| held.map_or((Kind::F8, false), |(kind, listed, _)| (kind, listed))).collect())
}

/// Check each column's record has the kind the recipe makes it over every sample's branch list.
fn columns(data: &Value, lock: &Lock, recipe: &Recipe) -> Result<()> {
    let wanted = kinds(recipe, lock).map_err(|e| invalid(format!("made.recipe.text, over made.lock.text's branch lists: {e}")))?;
    let records = data["cols"].as_array().map_or(&[][..], Vec::as_slice);
    for (((name, _), (kind, listed)), record) in recipe.columns.iter().zip(wanted).zip(records) {
        let (have, have_listed) = match record["kind"].as_str() {
            Some("jagged") => (record["content"]["kind"].as_str().unwrap_or_default(), true),
            other => (other.unwrap_or_default(), false),
        };
        if (have, have_listed) != (kind.name(), listed) {
            let shape = |kind: &str, listed: bool| if listed { format!("jagged of {kind}") } else { kind.to_owned() };
            return Err(invalid(format!("data.cols[{name:?}] is {}, and the recipe makes it {} from the branches the lock lists: a copied branch keeps its kind, a computed column has its expression's, unless the recipe stores it as another (docs/FORMAT.md §4): {MEND}", shape(have, have_listed), shape(kind.name(), listed))));
        }
    }
    Ok(())
}
