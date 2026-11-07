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
/// Gives whether each stored column is worked out, every NaN it holds the one quiet NaN.
pub(super) fn validate(header: &Value, reads: &Reads) -> Result<Vec<bool>> {
    let (data, made) = (&header["data"], &header["made"]);
    let source = made["recipe"]["text"].as_str().unwrap_or_default();
    let recipe = Recipe::read(source).map_err(|e| {
        invalid(format!("made.recipe.text cannot be read as a recipe the recipe reference accepts ({}). {}", e.line(), super::derived::remedy(header)))
    })?;
    let stored: Vec<&str> = recipe.columns.iter().map(|(name, _)| name.as_str()).collect();
    if stored != super::snapshot::pressed(made) {
        return Err(invalid(format!("made.recipe.columns is {}, and the recipe's [columns] stores {stored:?}, in that order (docs/FORMAT.md §6): {MEND}", made["recipe"]["columns"])));
    }
    if recipe.derived != super::derived::listed(header) {
        return Err(invalid(format!("the [derived] table of made.recipe.text does not list the derived columns its header records, by the same names and expressions in the same order (docs/FORMAT.md §10): one of the two was edited after it was written. {}", super::derived::remedy(header))));
    }
    if data["chunk_rows"].as_u64() != Some(recipe.chunk_rows() as u64) {
        return Err(invalid(format!("data.chunk_rows is {}, and it is the recipe's chunk_rows, {} (65,536 when the recipe gives none; docs/FORMAT.md §3, §7): {MEND}", data["chunk_rows"], crate::rvr::grouped(recipe.chunk_rows() as u64))));
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
    columns(data, &lock, &recipe)?;
    Ok(recipe.columns.iter().map(|(name, expression)| !recipe.keeps_bits(name, expression)).collect())
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
        let events = Events::read(&source["events"])?;
        let want: Vec<&lock::File> = match events {
            Events::First(_) => {
                let entries: Vec<u64> = locked.files.iter().map(|f| f.entries).collect();
                locked.files.iter().zip(lock::reads(&entries, events)).filter(|(f, planned)| planned.read > 0 || f.entries == 0).map(|(f, _)| f).collect()
            }
            _ => locked.files.iter().collect(),
        };
        let paths: Vec<&str> = data["files"].as_array().into_iter().flatten().filter(|f| f["sample"] == name).filter_map(|f| f["path"].as_str()).collect();
        let wanted: Vec<&str> = want.iter().map(|f| f.at.as_str()).collect();
        if paths != wanted {
            return Err(invalid(format!("data.files lists of sample {name} {paths:?}, and it lists the lock's files of the sample in its order, leaving out each one the first N never reached (its stop would be 0): {wanted:?} (docs/FORMAT.md §3): {MEND}")));
        }
        // Each file's random take, against the choice the file made again (identity::choices)
        // when it is the lock's own, else made again here.
        if let Events::Random { n, seed } = locked.events {
            let counts = match events == locked.events {
                true => locked.files.iter().map(|f| match reads.get(&(name, f.at.as_str())) {
                    Some((super::identity::Read::Chosen(chosen), _)) => Ok(chosen.len() as u64),
                    _ => Err(invalid(format!("a bug in rowver: the random choice of sample {name} was made again without {} (data.files lists it): please report it", f.at))),
                }).collect::<Result<Vec<u64>>>()?,
                false => crate::choice::counts(&locked.files.iter().map(|f| f.entries).collect::<Vec<_>>(), n, seed),
            };
            locked.takes(&counts).map_err(|why| invalid(format!("made.lock.text: {why}")))?;
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

/// Problems found in several samples alike, each said once with the samples it was found in.
#[derive(Default)]
struct Found(Vec<(String, Vec<String>)>);

impl Found {
    /// `said` of `sample`.
    fn add(&mut self, said: String, sample: &str) {
        match self.0.iter_mut().find(|(was, _)| *was == said) {
            Some((_, samples)) if samples.iter().any(|s| s == sample) => {}
            Some((_, samples)) => samples.push(sample.to_owned()),
            None => self.0.push((said, vec![sample.to_owned()])),
        }
    }

    /// One a line, each naming its samples, as `within` says for one sample or for several.
    fn lines(&self, within: &dyn Fn(&str, bool) -> String) -> Vec<String> {
        self.0.iter().map(|(said, samples)| format!("{said} ({})", within(&samples.join(", "), samples.len() > 1))).collect()
    }
}

/// Whether any expression of the recipe reads `sample.file`, whose value differs by file.
fn reads_file(recipe: &Recipe) -> bool {
    let mut fields = std::collections::BTreeSet::new();
    let parsed: Vec<rvr_recipe::Expression> = recipe.derived.iter().filter_map(|(_, e)| rvr_recipe::parse(e).ok()).collect();
    let cuts = recipe.keep.iter().flat_map(rvr_recipe::Cut::expressions);
    let all = recipe.defines.values().chain(cuts).chain(recipe.columns.iter().map(|(_, e)| e)).chain(recipe.totals.iter().map(|t| &t.value)).chain(&parsed);
    all.for_each(|expression| rvr_recipe::fields_of(expression, &mut fields));
    fields.contains("file")
}

/// The paths `sample.file` takes in a sample, as the checks work it out: each of its files' when
/// the recipe reads it, else its first (an empty path for a sample of no files).
fn paths(sample: &lock::Sample, every: bool) -> Vec<&str> {
    let all: Vec<&str> = sample.files.iter().map(|f| f.at.as_str()).collect();
    match (every, all.first()) {
        (true, Some(_)) => all,
        (false, Some(first)) => vec![first],
        (_, None) => vec![""],
    }
}

/// A kind and shape in words: `f4`, or `a list of f4`.
fn shape(kind: Kind, listed: bool) -> String {
    if listed { format!("a list of {}", kind.name()) } else { kind.name().to_owned() }
}

/// A kind and shape, with the sample and file it was found in.
type Where<'a> = (Kind, bool, &'a str, &'a str);

/// Each column's kind and shape as first found, and whether another has been found since.
struct Seen<'a>(Vec<(Option<Where<'a>>, bool)>);

impl<'a> Seen<'a> {
    /// Nothing found yet of `columns` columns.
    fn new(columns: usize) -> Self {
        Seen(vec![(None, false); columns])
    }

    /// Column `at` found as `kind` in file `path` of `sample`: where it was found as another, the first time it is.
    fn found(&mut self, at: usize, kind: (Kind, bool), sample: &'a str, path: &'a str) -> Option<Where<'a>> {
        match &mut self.0[at] {
            (held @ None, _) => *held = Some((kind.0, kind.1, sample, path)),
            (Some(first), mixed) if (first.0, first.1) != kind && !*mixed => {
                *mixed = true;
                return Some(*first);
            }
            _ => {}
        }
        None
    }

    /// Each column's kind and shape as first found (`f8` for one never found).
    fn kinds(self) -> Vec<(Kind, bool)> {
        self.0.into_iter().map(|(held, _)| held.map_or((Kind::F8, false), |(kind, listed, _, _)| (kind, listed))).collect()
    }
}

/// Each `[columns]` column's kind and whether it is a list per row over every sample's branch list; every problem, one a line.
pub fn kinds(recipe: &Recipe, lock: &Lock) -> Result<Vec<(Kind, bool)>> {
    let mut said = defines_apart(recipe, lock);
    let clashes = !said.is_empty();
    let every = reads_file(recipe);
    let mut found = Found::default();
    let mut seen = Seen::new(recipe.columns.len());
    for sample in &lock.samples {
        let Some(list) = lock.branches_of(sample) else {
            said.push(format!("samples.lock sample {:?} names a branch list it does not hold: resolve again", sample.name));
            continue;
        };
        let branches = Branches(list.list.iter().filter_map(|b| Some((b.name.as_str(), (Kind::named(&b.kind).ok()?, b.shape != lock::Shape::Flat)))).collect());
        for path in paths(sample, every) {
            let mut metadata: BTreeMap<String, Constant> = super::derived::fields(&sample.metadata);
            metadata.insert("file".into(), Constant::Text(path.to_owned()));
            let scope = Scope::new(&branches, &metadata, &recipe.defines);
            let mut fails = |what: String, e: rvr_recipe::Error| {
                // A defined name a branch has too is said once, above, not again where it is used.
                if clashes && e.clash() {
                    return;
                }
                // A branch rvr cannot read is left out of the scope: said as such, not as no column.
                if let Some(b) = e.missing().and_then(|name| list.list.iter().find(|b| b.name == name && b.why.is_some())) {
                    let why = b.why.as_deref().unwrap_or_default();
                    return found.add(format!("{what}: {} is a branch of a type rvr cannot read ({why}): leave it out of the recipe.", b.name), &sample.name);
                }
                let near = e.missing().map(|name| rvr_recipe::meant(name, list.list.iter().filter(|b| b.why.is_none()).map(|b| b.name.as_str())));
                let text = if e.0.ends_with(['.', '?']) { e.0 } else { format!("{}.", e.0) };
                found.add(format!("{what}: {text}{}", near.unwrap_or_default()), &sample.name);
            };
            for cut in &recipe.keep {
                let place = format!("[keep] {}", cut.name);
                // A cut whose samples (`when`) leave this one out is not worked out for it.
                let applies = match cut.when.as_ref().map(|when| scope.evaluate(when)) {
                    None => Ok(true),
                    Some(Ok(Array::Constant(Constant::Truth(false) | Constant::Missing(_)))) => Ok(false),
                    Some(Ok(value)) => Scope::condition(value, 0).map(|_| true),
                    Some(Err(e)) => Err(e),
                };
                match applies.and_then(|applies| if applies { scope.mask(&cut.pass, 0).map(drop) } else { Ok(()) }) {
                    Ok(()) => {}
                    Err(e) => fails(place, e),
                }
            }
            for total in recipe.totals.iter().filter(|total| total.tree.is_none()) {
                match scope.evaluate(&total.value).and_then(|v| per_event(v, 0)) {
                    Err(e) => fails(format!("[totals] {}", total.name), e),
                    // A total sums one number per event: a list is refused here, not as it is read.
                    Ok(Array::Object { .. }) => {
                        let sum = match &total.value {
                            rvr_recipe::Expression::Name(name) => format!("sum({name})"),
                            _ => "sum(…)".into(),
                        };
                        fails(format!("[totals] {}", total.name), rvr_recipe::problem(format!("a total sums one number per event, and this gives a list for each: sum the list, as {} = \"{sum}\"", total.name)));
                    }
                    Ok(_) => {}
                }
            }
            for (at, (name, expression)) in recipe.columns.iter().enumerate() {
                let array = match scope.evaluate(expression).and_then(|v| per_event(v, 0)) {
                    Ok(array) => array,
                    Err(e) => {
                        fails(format!("[columns] {name}"), e);
                        continue;
                    }
                };
                let (kind, listed) = array.shape().expect("per_event gives one value per event");
                let kind = recipe.stored.get(name).copied().unwrap_or(kind);
                if let Some((first, was, before, there)) = seen.found(at, (kind, listed), &sample.name, path) {
                    said.push(match before == sample.name {
                        // Within one sample, it is sample.file that tells its files apart.
                        true => format!("[columns] {name} is {} in file {there} and {} in file {path} of sample {before}, as sample.file gives them, and a column has one kind in every file: give each value it takes there one kind, or store it as one, as {name} = {{ value = \"…\", stored = \"f8\" }}", shape(first, was), shape(kind, listed)),
                        false => format!("[columns] {name} is {} in sample {before} and {} in sample {}, as their branch lists in samples.lock give, and a column has one kind in every file: name the samples' branches apart, or convert it in the recipe, as {name} = {{ value = \"…\", stored = \"f8\" }}", shape(first, was), shape(kind, listed), sample.name),
                    });
                }
            }
        }
    }
    said.extend(found.lines(&|samples, many| match many {
        false => format!("sample {samples}, over the branches samples.lock lists for it"),
        true => format!("samples {samples}, over the branches samples.lock lists for each"),
    }));
    match said.is_empty() {
        true => Ok(seen.kinds()),
        false => Err(invalid(said.join("\n"))),
    }
}

/// Each `[derived]` column's kind and shape over every sample's stored columns (`kinds`) and metadata; every problem, one a line.
pub fn derived_kinds(recipe: &Recipe, lock: &Lock, kinds: &[(Kind, bool)]) -> Result<Vec<(Kind, bool)>> {
    let mut said = Vec::new();
    let mut found = Found::default();
    let mut seen = Seen::new(recipe.derived.len());
    let parsed: Vec<Option<rvr_recipe::Expression>> = recipe.derived.iter().map(|(_, e)| rvr_recipe::parse(e).ok()).collect();
    let stored: BTreeMap<&str, (Kind, bool)> = recipe.columns.iter().map(|(n, _)| n.as_str()).zip(kinds.iter().copied()).collect();
    let defines = BTreeMap::new();
    let every = reads_file(recipe);
    for sample in &lock.samples {
        for path in paths(sample, every) {
            let mut columns = Branches(stored.clone());
            let mut metadata: BTreeMap<String, Constant> = super::derived::fields(&sample.metadata);
            super::derived::standing_in(&mut metadata, recipe);
            metadata.insert("file".into(), Constant::Text(path.to_owned()));
            let mut failed = std::collections::BTreeSet::new();
            for (at, ((name, _), expression)) in recipe.derived.iter().zip(&parsed).enumerate() {
                let Some(expression) = expression else { continue };
                let mut used = std::collections::BTreeSet::new();
                rvr_recipe::reads(expression, &mut used);
                if used.iter().any(|u| failed.contains(u)) {
                    failed.insert(name.clone());
                    continue;
                }
                let array = Scope::new(&columns, &metadata, &defines).evaluate(expression).and_then(|v| per_event(v, 0));
                let (kind, listed) = match &array {
                    Ok(array) => array.shape().expect("per_event gives one value per row"),
                    Err(e) => {
                        let text = if e.0.ends_with(['.', '?']) { e.0.clone() } else { format!("{}.", e.0) };
                        found.add(format!("[derived] {name}: {text}"), &sample.name);
                        failed.insert(name.clone());
                        continue;
                    }
                };
                columns.0.insert(name.as_str(), (kind, listed));
                if let Some((first, was, before, there)) = seen.found(at, (kind, listed), &sample.name, path) {
                    said.push(match before == sample.name {
                        true => format!("[derived] {name} is {} in file {there} and {} in file {path} of sample {before}, as sample.file gives them, and a derived column has one kind in every file: give each value it takes there one kind, as stored(…, \"f8\")", shape(first, was), shape(kind, listed)),
                        false => format!("[derived] {name} is {} in sample {before} and {} in sample {}, and a derived column has one kind in every sample: give each sample's value the same kind, as stored(…, \"f8\")", shape(first, was), shape(kind, listed), sample.name),
                    });
                }
            }
        }
    }
    said.extend(found.lines(&|samples, many| match many {
        false => format!("sample {samples}, over its stored columns and metadata"),
        true => format!("samples {samples}, over their stored columns and metadata"),
    }));
    match said.is_empty() {
        true => Ok(seen.kinds()),
        false => Err(invalid(said.join("\n"))),
    }
}

/// Each `[define]` named like a branch of a sample's list, with the samples whose lists have it:
/// a name means one thing (SPEC.md §2.3).
fn defines_apart(recipe: &Recipe, lock: &Lock) -> Vec<String> {
    let names: Vec<(&str, std::collections::HashSet<&str>)> = lock.samples.iter().map(|s| {
        let list = lock.branches_of(s).map(|list| list.list.iter().map(|b| b.name.as_str()).collect()).unwrap_or_default();
        (s.name.as_str(), list)
    }).collect();
    let mut said = Vec::new();
    for name in recipe.defines.keys() {
        let samples: Vec<&str> = names.iter().filter(|(_, list)| list.contains(name.as_str())).map(|(sample, _)| *sample).collect();
        if !samples.is_empty() {
            said.push(format!("[define] {name} is also the name of a branch of sample {} in samples.lock, and a name means one thing: rename the defined name, as {name}_sel, where the recipe uses it", samples.join(", ")));
        }
    }
    said
}

/// Check each column's record has the kind the recipe makes it over every sample's branch list.
fn columns(data: &Value, lock: &Lock, recipe: &Recipe) -> Result<()> {
    let wanted = kinds(recipe, lock).map_err(|e| invalid(format!("made.recipe.text, over made.lock.text's branch lists: {}", e.to_string().replace('\n', "; "))))?;
    // A derived column is of one kind in every sample, those that kept no row too (docs/FORMAT.md §10).
    derived_kinds(recipe, lock, &wanted).map_err(|e| invalid(format!("made.recipe.text's [derived], over made.lock.text: {}. {}", e.to_string().replace('\n', "; ").trim_end_matches('.'), super::DERIVED_MEND)))?;
    let records = data["cols"].as_array().map_or(&[][..], Vec::as_slice);
    for (((name, _), &(kind, listed)), record) in recipe.columns.iter().zip(&wanted).zip(records) {
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
