//! The agreement check (docs/SPEC.md §2.3): the inputs each validator passed, taken together, and
//! every way they do not fit, one a line. The earlier file's part is `reuse` and `fill`'s.

use crate::plan::Choose;
use crate::texts::Texts;
use rowver::choice::Events;
use rowver::rvr::derived::ADDED;
use rowver::study::lock::Lock;
use crate::plan::Shape;
use rvr_recipe::Recipe;

/// The lock was resolved from this samples.toml, and its samples are what samples.toml makes.
pub fn lock(texts: &Texts, lock: &Lock) -> Vec<String> {
    if rowver::write::text_checksum(&texts.samples) != lock.samples_toml_sha256 {
        return vec![format!("samples.toml has changed since samples.lock was written ({}). Run rvr samples resolve samples.toml, review the new lock, then press.", lock.resolved)];
    }
    match rowver::study::lock::agree(&texts.samples, lock) {
        Ok(()) => Vec::new(),
        Err(problems) => problems.said.iter().map(|p| format!("samples.lock does not agree with samples.toml: {p}. Run rvr samples resolve samples.toml and review the new lock.")).collect(),
    }
}

/// Every storage file of the lock is on a host rvr reads through a WebDAV door (a `root://`
/// host the WLCG registry it knows lists one); each host that is not, once.
pub fn doors(lock: &Lock) -> Vec<String> {
    let mut said: Vec<(String, &str)> = Vec::new();
    for file in lock.samples.iter().flat_map(|s| &s.files).filter(|f| f.at.contains("://")) {
        if let Err(why) = crate::samples::remote::door(&file.at) {
            if !said.iter().any(|(was, _)| *was == why) {
                said.push((why, &file.at));
            }
        }
    }
    said.into_iter().map(|(why, at)| format!("samples.lock names {at}: {why}; then run rvr samples resolve samples.toml.")).collect()
}

/// The lock with each sample's event choice applied, and those choices; or every way the choice
/// does not fit the samples (`--seed` without a random sample, `--random` without a number).
pub fn planned(lock: Lock, choose: &Choose) -> Result<(Lock, Vec<Events>), Vec<String>> {
    let (lock, asked, said) = choices(lock, choose);
    match said.is_empty() {
        true => Ok((lock, asked)),
        false => Err(said),
    }
}

/// `planned`, with every problem beside a lock that the other checks can go on with: a sample
/// whose choice does not fit keeps its own.
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
        *sample = sample.planned(want);
        asked.push(want);
    }
    (lock, asked, said)
}

/// The kind of every stored and every derived column, worked out over each sample's branch list
/// and metadata; or every way the recipe does not fit the samples: a column of two kinds, a name
/// that means two things, a total over the events' own tree, or over another tree of a sample that
/// reads only some of its files.
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
