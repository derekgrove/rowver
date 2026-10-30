//! Pressing a study again when only its stored columns changed. A file `rvr press` wrote already
//! says which events its cuts kept (`data.files` and `entry`), so when the study, its cuts, its
//! event choice and the build of rvr are the same, only the columns that are new or whose
//! definition changed are computed, over the entries a full press reads, keeping the events the
//! earlier file kept; every other column is copied from it as it lies, compressed. The file
//! written is the one a full press of the recipe writes.

use crate::samples::resolve::Lock;
use crate::samples::spec::Events;
use rowver::rvr::Reader;
use rvr_recipe::{uses, Cut, Expression, Recipe};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
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

impl Earlier {
    /// The earlier file, which the columns it gives are copied from.
    pub fn file(&self) -> &Reader {
        &self.file
    }
}

/// Whether a file's header is a press of this study's samples by this build of rvr, or why not.
pub fn pressed_alike(header: &Value, texts: &crate::press::Texts, lock: &Lock) -> Result<(), String> {
    let made = &header["made"];
    if made["samples"]["text"].as_str() != Some(texts.samples.as_str()) {
        return Err("samples.toml has changed since".into());
    }
    let samples = header["data"]["samples"].as_array().ok_or("it has no samples")?;
    let names: Vec<&str> = samples.iter().filter_map(|s| s["name"].as_str()).collect();
    if names != lock.sample.iter().map(|s| s.name.as_str()).collect::<Vec<_>>() {
        return Err("its samples are not the lock's".into());
    }
    same_build(made)
}

/// Whether this build of rvr made a file (its `made`), or why not.
// Columns computed by another build (another version, commit or compiler) are not copied: the
// new file names this build.
pub fn same_build(made: &Value) -> Result<(), String> {
    if made["toolchain"] != crate::press::toolchain() {
        return Err(format!(
            "it was made by another build of rvr ({} at {})",
            made["toolchain"]["rvr"].as_str().unwrap_or("?"),
            made["toolchain"]["commit"].as_str().unwrap_or("?")
        ));
    }
    // A build of changed code whose changes could not be hashed names no one build.
    if made["toolchain"]["commit"].as_str().is_none_or(|commit| commit.ends_with("-dirty")) {
        return Err("it was made by an rvr built from changed code that left no record of the changes".into());
    }
    Ok(())
}

/// What the file at `path` gives this press: nothing when there is none, or why it cannot.
pub fn earlier(
    path: &Path,
    texts: &crate::press::Texts,
    recipe: &Recipe,
    lock: &Lock,
    asked: &[Events],
) -> Option<Result<Earlier, String>> {
    path.exists()
        .then(|| read(path, texts, recipe, lock, asked))
}

fn read(
    path: &Path,
    texts: &crate::press::Texts,
    recipe: &Recipe,
    lock: &Lock,
    asked: &[Events],
) -> Result<Earlier, String> {
    let file = Reader::open(path).map_err(|e| format!("it could not be read ({e})"))?;
    let header = file.header();
    let made = &header["made"];
    let text = |key: &str| made[key]["text"].as_str();
    pressed_alike(header, texts, lock)?;
    if text("lock") != Some(texts.lock.as_str()) {
        return Err("samples.lock has changed since".into());
    }
    let old = Recipe::read(text("recipe").unwrap_or_default())
        .map_err(|e| format!("its recipe cannot be read ({})", e.0))?;
    if cuts(&old) != cuts(recipe) {
        return Err("the cuts under [keep], or what they use, have changed".into());
    }
    let files = made["files"].as_object().ok_or("it records no files")?;
    for (sample, want) in lock.sample.iter().zip(asked) {
        if files.get(&sample.name).map(|s| &s["events"]) != Some(&want.record()) {
            return Err(format!(
                "it chose other events of {} (another --events, --random or --seed)",
                sample.name
            ));
        }
    }

    let failed = |e: rowver::Error| format!("it could not be read ({e})");
    let entries = file.entry().map_err(failed)?;
    let mut kept = HashMap::new();
    let mut at = 0;
    for read in header["data"]["files"]
        .as_array()
        .ok_or("it lists no files")?
    {
        let n = read["rows"].as_u64().unwrap_or(0) as usize;
        let span = entries
            .get(at..at + n)
            .ok_or("its files hold more rows than it has")?;
        let key = (read["sample"].as_str(), read["path"].as_str());
        let (Some(sample), Some(path)) = key else {
            return Err("it lists a file without its sample or path".into());
        };
        let entries = span.iter().map(|&e| e as u64).collect();
        if kept
            .insert((sample.to_owned(), path.to_owned()), entries)
            .is_some()
        {
            return Err(format!("it read {path} twice for {sample}"));
        }
        at += n;
    }
    let reused = recipe
        .columns
        .iter()
        .map(|(name, _)| {
            file.record(name).is_some() && only(&old, &[name]) == only(recipe, &[name])
        })
        .collect();
    Ok(Earlier { file, reused, kept })
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
