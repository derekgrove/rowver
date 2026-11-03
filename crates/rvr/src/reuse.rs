//! Pressing a study again on the file a press of it wrote, which says which events its cuts kept
//! (`data.files` and `entry`). With the same cuts and the same build of rvr:
//! - with the same samples and event choice, only the columns that are new or whose definition
//!   changed are computed, over the entries a full press reads, keeping the events the earlier
//!   file kept; every other column is copied from it as it lies, compressed;
//! - with samples added, removed or reordered and the same columns, the rows of every sample the
//!   lock holds as it did are the file's, and only the other samples are read (as a fill reads
//!   holes: each column is packed again, since it is compressed whole).
//!
//! The file written is the one a full press of the recipe writes.

use crate::fill::{unplanned, Fill, Plan};
use crate::plan::Base;
use rowver::choice::Events;
use rowver::study::lock::Lock;
use rowver::rvr::Reader;
use rvr_recipe::{uses, Cut, Expression, Recipe};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

/// What an earlier press gives a new one.
pub struct Earlier {
    file: Reader,
    /// Per output column, in the recipe's order: whether it is copied from the earlier file.
    pub reused: Vec<bool>,
    /// The kept entries of each file read, by sample and the file's `at`.
    pub kept: HashMap<(String, String), Vec<u64>>,
}

/// The names the recipe defines that `roots` reach, directly or through others: the ones a press
/// evaluates.
pub(crate) fn reached(recipe: &Recipe, roots: &[&Expression]) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut todo: Vec<&Expression> = roots.to_vec();
    while let Some(expression) = todo.pop() {
        let mut named = BTreeSet::new();
        uses(expression, &mut named);
        for name in named {
            if let Some(defined) = recipe.defines.get(&name) {
                if found.insert(name) {
                    todo.push(defined);
                }
            }
        }
    }
    found.into_iter().collect()
}

/// The recipe cut down to the names it defines that `roots` reach.
fn reaching(recipe: &Recipe, roots: &[&Expression]) -> Recipe {
    let defines = reached(recipe, roots).into_iter().map(|name| {
        let defined = recipe.defines[&name].clone();
        (name, defined)
    });
    Recipe {
        defines: defines.collect(),
        ..Recipe::default()
    }
}

/// The recipe cut down to reading `columns`: no cuts, and only the defined names they reach.
pub fn only(recipe: &Recipe, columns: &[&String]) -> Recipe {
    let picked: Vec<(String, Expression)> = recipe
        .columns
        .iter()
        .filter(|(n, _)| columns.contains(&n))
        .cloned()
        .collect();
    let roots: Vec<&Expression> = picked.iter().map(|(_, e)| e).collect();
    let reached = reaching(recipe, &roots);
    Recipe {
        stored: recipe
            .stored
            .iter()
            .filter(|(n, _)| columns.contains(n))
            .map(|(n, k)| (n.clone(), *k))
            .collect(),
        columns: picked,
        ..reached
    }
}

/// The recipe cut down to its cuts and what they reach.
pub fn cuts(recipe: &Recipe) -> Recipe {
    let roots: Vec<&Expression> = recipe.keep.iter().flat_map(Cut::expressions).collect();
    Recipe {
        keep: recipe.keep.clone(),
        ..reaching(recipe, &roots)
    }
}

/// The recipe cut down to its totals and the defined names they reach.
pub fn totaled(recipe: &Recipe) -> Recipe {
    let roots: Vec<&Expression> = recipe.totals.iter().map(|total| &total.value).collect();
    Recipe {
        totals: recipe.totals.clone(),
        ..reaching(recipe, &roots)
    }
}

impl Earlier {
    /// The earlier file, which the columns it gives are copied from.
    pub fn file(&self) -> &Reader {
        &self.file
    }
}

/// Whether a file's header is a press of this study's samples by this build of rvr, or why not.
pub fn pressed_alike(header: &Value, texts: &crate::texts::Texts, lock: &Lock) -> Result<(), String> {
    if header["made"]["samples"]["text"].as_str() != Some(texts.samples.as_str()) {
        return Err("samples.toml has changed since".into());
    }
    same_samples(header, lock)?;
    same_build(&header["made"])
}

/// Whether a file holds the lock's samples, by name and in order.
fn same_samples(header: &Value, lock: &Lock) -> Result<(), String> {
    let samples = header["data"]["samples"].as_array().ok_or("it has no samples")?;
    let names: Vec<&str> = samples.iter().filter_map(|s| s["name"].as_str()).collect();
    match names == lock.samples.iter().map(|s| s.name.as_str()).collect::<Vec<_>>() {
        true => Ok(()),
        false => Err("its samples are not the lock's".into()),
    }
}

/// Whether this build of rvr names one build: a commit that is known, and when its code had
/// changes, with a record of them (docs/SPEC.md §12.3); or why not.
pub fn identified() -> Result<(), String> {
    match env!("RVR_COMMIT") {
        "unknown" => Err("this rvr was built from no known commit (its commit is unknown), so what it made cannot be told from another build's: build it from a git checkout, or install a released rvr".into()),
        commit if commit.ends_with("-dirty") => Err(format!("this rvr was built from changed code that left no record of the changes (its commit is {commit}): build it from a clean checkout, or install a released rvr")),
        _ => Ok(()),
    }
}

/// Whether this build of rvr made a file (its `made`), or why not.
// Columns computed by another build (another version, commit or compiler) are not copied: the
// new file names this build.
pub fn same_build(made: &Value) -> Result<(), String> {
    identified()?;
    if made["toolchain"] != crate::press::toolchain() {
        return Err(format!(
            "it was made by another build of rvr ({} at {})",
            made["toolchain"]["rvr"].as_str().unwrap_or("?"),
            made["toolchain"]["commit"].as_str().unwrap_or("?")
        ));
    }
    Ok(())
}

/// What the file at `path` gives this press: nothing when there is none, or why it cannot.
pub fn earlier(
    path: &Path,
    texts: &crate::texts::Texts,
    recipe: &Recipe,
    lock: &Lock,
    asked: &[Events],
) -> Option<Result<Base, String>> {
    path.exists()
        .then(|| read(path, texts, recipe, lock, asked))
}

fn read(
    path: &Path,
    texts: &crate::texts::Texts,
    recipe: &Recipe,
    lock: &Lock,
    asked: &[Events],
) -> Result<Base, String> {
    let file = Reader::open(path).map_err(|e| format!("it could not be read ({e})"))?;
    if file.empty() {
        return Err("it is empty (rvr refill gives its columns back, checked against its checksums)".into());
    }
    let header = file.header();
    let made = &header["made"];
    let text = |key: &str| made[key]["text"].as_str();
    same_build(made)?;
    let old = Recipe::read(text("recipe").unwrap_or_default())
        .map_err(|e| format!("its recipe cannot be read ({})", e.line()))?;
    if cuts(&old) != cuts(recipe) {
        return Err("the cuts under [keep], or what they use, have changed".into());
    }
    if totaled(&old) != totaled(recipe) {
        return Err("the totals under [totals] have changed, which count every event read".into());
    }
    let reused: Vec<bool> = recipe
        .columns
        .iter()
        .map(|(name, _)| {
            file.record(name).is_some() && only(&old, &[name]) == only(recipe, &[name])
        })
        .collect();
    if text("samples") != Some(texts.samples.as_str()) || text("lock") != Some(texts.lock.as_str()) {
        return samples(file, recipe, &reused, lock, asked).map(Base::Samples);
    }
    same_samples(header, lock)?;
    let files = made["files"].as_object().ok_or("it records no files")?;
    for (sample, want) in lock.samples.iter().zip(asked) {
        if files.get(&sample.name).map(|s| &s["events"]) != Some(&want.record()) {
            return Err(format!(
                "it chose other events of {} (another --events, --random or --seed)",
                sample.name
            ));
        }
    }
    let rows = crate::fill::rows(file, &[], &HashSet::new()).map_err(|e| format!("it could not be read ({e})"))?;
    let kept = rows.files.iter().map(|(key, plan)| (key.clone(), rows.entries[plan.rows.clone()].iter().map(|&e| e as u64).collect())).collect();
    Ok(Base::Columns(Earlier { file: rows.file, reused, kept }))
}

/// When the lock's samples changed since the file was pressed, its rows of the samples the lock
/// holds as it did (the same files, tree, metadata and event choice); the others are read whole.
/// Its columns must be the recipe's.
fn samples(file: Reader, recipe: &Recipe, reused: &[bool], lock: &Lock, asked: &[Events]) -> Result<Fill, String> {
    let changed: Vec<&str> = recipe.columns.iter().zip(reused).filter(|(_, same)| !**same).map(|((n, _), _)| n.as_str()).collect();
    if !changed.is_empty() {
        return Err(format!("samples.lock has changed since, and so have the columns ({} new or changed): a press builds on a file for a change of either, not of both", changed.join(", ")));
    }
    let made = &file.header()["made"];
    let old = Lock::read(made["lock"]["text"].as_str().unwrap_or_default()).map_err(|p| format!("its samples.lock does not check out ({p})"))?;
    let names = |samples: &Value| -> Vec<String> { samples.as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_owned)).collect() };
    if names(&file.header()["data"]["samples"]) != old.samples.iter().map(|s| s.name.clone()).collect::<Vec<_>>() {
        return Err("its samples are not its own lock's".into());
    }
    let (mut added, mut plans) = (Vec::new(), Vec::new());
    for (sample, want) in lock.samples.iter().zip(asked) {
        let Some(was) = old.samples.iter().find(|s| s.name == sample.name) else {
            for f in &sample.files {
                let read = vec![(0, f.stop.unwrap_or(f.entries))];
                let plan = Plan { rows: 0..0, read, kept: Vec::new(), record: Value::Null, unread: true };
                plans.push(((sample.name.clone(), f.at.clone()), plan));
            }
            added.push(sample.name.clone());
            continue;
        };
        let files = |s: &rowver::study::lock::Sample| s.files.iter().map(unplanned).collect::<Vec<_>>();
        if was.tree != sample.tree || was.metadata != sample.metadata || files(was) != files(sample) {
            return Err(format!("{} is another sample in samples.lock now (its files, tree or metadata)", sample.name));
        }
        if made["files"][&sample.name]["events"] != want.record() {
            return Err(format!("it chose other events of {} (another --events, --random or --seed)", sample.name));
        }
    }
    let mut fill = crate::fill::rows(file, &[], &HashSet::new())?;
    fill.files.extend(plans);
    fill.added = Some(added);
    Ok(fill)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
format = "rvr-recipe"
version = 1

[define]
good_jet = "Jet_pt > 30"
hard_jet = "Jet_pt > 80"

[keep]
two_jets = "count(good_jet) >= 2"
one_hard = "count(Jet_pt > 50) >= 1"

[columns]
MET = "MET_pt"
n_jet = "count(good_jet)"
n_hard = "count(hard_jet)"

[derived]
"#;

    fn same(old: &str, new: &str, column: &str) -> bool {
        let (old, new) = (Recipe::read(old).unwrap(), Recipe::read(new).unwrap());
        let name = column.to_string();
        only(&old, &[&name]) == only(&new, &[&name])
    }

    fn same_cuts(old: &str, new: &str) -> bool {
        cuts(&Recipe::read(old).unwrap()) == cuts(&Recipe::read(new).unwrap())
    }

    #[test]
    fn a_column_or_a_cut_is_the_same_only_with_everything_it_reaches() {
        let added = BASE.replace("[columns]", "[columns]\nht = \"sum(Jet_pt[good_jet])\"");
        for column in ["MET", "n_jet", "n_hard"] {
            assert!(same(BASE, &added, column), "{column}");
        }
        assert!(!same(BASE, &added, "ht"), "a new column is read");
        assert!(same_cuts(BASE, &added));
        // A changed defined name changes what reaches it, and only that.
        let tighter = BASE.replace("Jet_pt > 30", "Jet_pt > 40");
        assert!(!same(BASE, &tighter, "n_jet"));
        assert!(same(BASE, &tighter, "MET"), "MET reaches no defined name");
        assert!(same(BASE, &tighter, "n_hard"), "n_hard reaches hard_jet, not good_jet");
        assert!(!same_cuts(BASE, &tighter));
        let harder = BASE.replace("Jet_pt > 80", "Jet_pt >= 80");
        assert!(!same(BASE, &harder, "n_hard"));
        assert!(same(BASE, &harder, "n_jet"));
        assert!(same_cuts(BASE, &harder), "no cut reaches hard_jet");
        // A column stored as another kind is changed.
        let stored = BASE.replace(
            "n_jet = \"count(good_jet)\"",
            "n_jet = { value = \"count(good_jet)\", stored = \"i2\" }",
        );
        assert!(!same(BASE, &stored, "n_jet"));
        // Reading a column needs only what it reaches.
        let hard = String::from("n_hard");
        let read = only(&Recipe::read(BASE).unwrap(), &[&hard]);
        assert!(read.keep.is_empty());
        assert_eq!(read.defines.keys().collect::<Vec<_>>(), ["hard_jet"]);
    }
}
