//! The agreement check (docs/SPEC.md §2.3): the inputs each validator passed, taken together, and
//! every way they do not fit, one a line. The earlier file's part is `reuse` and `fill`'s.

use crate::plan::Choose;
use crate::texts::Texts;
use rowver::choice::Events;
use rvr_recipe::ADDED;
use rowver::study::lock::Lock;
use crate::plan::Shape;
use rvr_recipe::Recipe;

/// What `expressions` of a recipe quietly decide alike for every event over the lock's samples, a
/// line each: a `sample.KEY` no sample has, and text compared with a number (or a number with text).
pub fn notes<'e>(expressions: impl Iterator<Item = &'e rvr_recipe::Expression>, recipe: &Recipe, lock: &Lock) -> Vec<String> {
    use rvr_recipe::Constant;
    use serde_json::Value;
    let (mut fields, mut compared) = (std::collections::BTreeSet::new(), Vec::new());
    for expression in expressions {
        rvr_recipe::fields_of(expression, &mut fields);
        rvr_recipe::compared(expression, &mut compared);
    }
    // A key a sample holds a value of: a list or a table is none (the recipe README, "missing").
    let valued = |v: &Value| matches!(v, Value::Number(_) | Value::String(_) | Value::Bool(_));
    let keys: std::collections::BTreeSet<&str> = lock.samples.iter().flat_map(|s| s.metadata.iter().filter(|(_, v)| valued(v)).map(|(k, _)| k.as_str())).collect();
    let known = |field: &str| field == "file" || ADDED.contains(&field) || keys.contains(field) || recipe.totals.iter().any(|total| total.name == field);
    let mut said: Vec<String> = fields.iter().filter(|field| !known(field)).map(|field| {
        format!("sample.{field} is a key no sample of samples.toml has a value of, so every comparison with it is false, for every event.{}", rvr_recipe::meant(field, keys.iter().copied()))
    }).collect();
    // Numbers and true-or-false values compare as numbers; text only with text.
    for (field, unequal, values) in compared {
        let texts: Vec<bool> = lock.samples.iter().filter_map(|s| s.metadata.get(&field).filter(|v| valued(v))).map(Value::is_string).collect();
        let Some(text) = texts.first().copied().filter(|text| texts.iter().all(|t| t == text)) else { continue };
        if values.is_empty() || values.iter().any(|v| matches!(v, Constant::Missing(_)) || matches!(v, Constant::Text(_)) == text) {
            continue;
        }
        let (is, with) = if text { ("text", "a number") } else { ("a number", "text") };
        let line = format!("sample.{field} is {is} in samples.toml, and is compared with {with}: that is {} for every event.", unequal);
        if !said.contains(&line) {
            said.push(line);
        }
    }
    said
}

/// `expression` and the defined names it uses, however deep, as `notes` reads them.
pub fn with_defines<'e>(expression: &'e rvr_recipe::Expression, recipe: &'e Recipe) -> Vec<&'e rvr_recipe::Expression> {
    let (mut out, mut seen, mut at) = (vec![expression], std::collections::BTreeSet::new(), 0);
    while at < out.len() {
        let mut used = std::collections::BTreeSet::new();
        rvr_recipe::uses(out[at], &mut used);
        out.extend(used.into_iter().filter(|name| seen.insert(name.clone())).filter_map(|name| recipe.defines.get(&name)));
        at += 1;
    }
    out
}

/// Every expression of a recipe: its defined names, cuts, columns, totals and derived columns.
pub fn expressions(recipe: &Recipe) -> Vec<rvr_recipe::Expression> {
    let derived = recipe.derived.iter().filter_map(|(_, text)| rvr_recipe::parse(text).ok());
    let cuts = recipe.keep.iter().flat_map(|cut| cut.expressions());
    recipe.defines.values().chain(cuts).chain(recipe.columns.iter().map(|(_, e)| e)).chain(recipe.totals.iter().map(|t| &t.value)).cloned().chain(derived).collect()
}

/// Whether `lock` was resolved from another samples.toml than `samples`.
pub fn stale(samples: &str, lock: &Lock) -> bool {
    rowver::write::text_checksum(samples) != lock.samples_toml_sha256
}

/// The lock was resolved from this samples.toml, and its samples are what samples.toml makes.
pub fn lock(texts: &Texts, lock: &Lock) -> Vec<String> {
    if stale(&texts.samples, lock) {
        return vec![format!("samples.toml has changed since samples.lock was written ({}). Run rvr samples resolve samples.toml, review the new lock, then press.", lock.resolved)];
    }
    match rowver::study::lock::agree(&texts.samples, lock) {
        Ok(()) => Vec::new(),
        Err(problems) => problems.said.iter().map(|p| format!("samples.lock does not agree with samples.toml: {p}. Run rvr samples resolve samples.toml and review the new lock.")).collect(),
    }
}

/// Where each lock file is read (its path, or its door's HTTPS URL), or each host with no door.
pub fn doors(lock: &Lock) -> Result<Vec<Vec<String>>, Vec<String>> {
    let mut said: Vec<(String, &str)> = Vec::new();
    let mut read_at = Vec::new();
    for sample in &lock.samples {
        let mut files = Vec::new();
        for file in &sample.files {
            match file.at.contains("://") {
                false => files.push(file.at.clone()),
                true => match crate::samples::remote::https(&file.at) {
                    Ok(url) => files.push(url),
                    Err(why) if !said.iter().any(|(was, _)| *was == why) => said.push((why, &file.at)),
                    Err(_) => {}
                },
            }
        }
        read_at.push(files);
    }
    match said.is_empty() {
        true => Ok(read_at),
        false => Err(said.into_iter().map(|(why, at)| format!("samples.lock names {at}: {why}; then run rvr samples resolve samples.toml.")).collect()),
    }
}

/// The lock with each sample's event choice applied, and those choices; or every way the choice does not fit.
pub fn planned(lock: Lock, choose: &Choose) -> Result<(Lock, Vec<Events>), Vec<String>> {
    let (lock, asked, said) = choices(lock, choose);
    match said.is_empty() {
        true => Ok((lock, asked)),
        false => Err(said),
    }
}

/// `planned`, with every problem beside a lock the other checks can go on with (a misfit choice kept as its own).
pub fn choices(mut lock: Lock, choose: &Choose) -> (Lock, Vec<Events>, Vec<String>) {
    let mut said = Vec::new();
    if let Choose::Options(choice) = choose {
        said.extend(choice.check(&lock.samples).err());
    }
    let mut asked = Vec::new();
    for sample in &mut lock.samples {
        let want = choose.of(sample).unwrap_or_else(|why| {
            said.push(why);
            sample.events
        });
        let planned = sample.planned(want);
        // The lock's own random choice, made again as it is planned: each file takes its share.
        if want == sample.events && matches!(want, Events::Random { .. }) {
            let counts: Vec<u64> = planned.files.iter().map(|f| f.take.unwrap_or(0)).collect();
            said.extend(sample.takes(&counts).err().map(|why| format!("samples.lock does not check out: {why}.")));
        }
        *sample = planned;
        asked.push(want);
    }
    (lock, asked, said)
}

/// The kind of every stored and derived column over each sample's branch list and metadata; or every misfit.
pub fn recipe(recipe: &Recipe, lock: &Lock, asked: &[Events]) -> Result<[Vec<Shape>; 2], Vec<String>> {
    let mut said: Vec<String> = Vec::new();
    let lines = |e: rowver::Error| e.to_string().lines().map(str::to_owned).collect::<Vec<_>>();
    let kinds = rowver::rvr::kinds(recipe, lock).map_err(|e| said.extend(lines(e))).ok();
    let derived = kinds.as_ref().and_then(|kinds| rowver::rvr::derived_kinds(recipe, lock, kinds).map_err(|e| said.extend(lines(e))).ok());
    let keyed = |name: &str| ADDED.contains(&name) || lock.samples.iter().any(|s| s.metadata.contains_key(name));
    for total in recipe.totals.iter().filter(|total| keyed(&total.name)) {
        said.push(format!("[totals] {} has the name of a key of samples.toml, or of one rvr press gives every sample ({}): give the total another", total.name, ADDED.join(", ")));
    }
    for (name, _) in recipe.derived.iter().filter(|(name, _)| keyed(name)) {
        said.push(format!("[derived] {name} has the name of a key of samples.toml, or of one rvr press gives every sample ({}): give the derived column another", ADDED.join(", ")));
    }
    for total in &recipe.totals {
        let Some(tree) = &total.tree else { continue };
        if let Some(sample) = lock.samples.iter().find(|sample| sample.tree == *tree) {
            said.push(format!("[totals] {} names the tree {tree}, the one {} reads its events from: a total over the events read is written without tree, as {} = \"…\"", total.name, sample.name, total.name));
        }
        if let Some((sample, _)) = lock.samples.iter().zip(asked).find(|(_, want)| !matches!(want, Events::All)) {
            said.push(format!("[totals] {} sums the tree {tree}, whose entries count every event of a file, and {} reads only some of its events: read its files whole (events = \"all\"), or total a branch of the events read", total.name, sample.name));
        }
    }
    match (kinds, derived, said.is_empty()) {
        (Some(kinds), Some(derived), true) => Ok([kinds, derived]),
        _ => Err(said),
    }
}
