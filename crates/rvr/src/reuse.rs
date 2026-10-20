//! Pressing a study again when only its stored columns changed. A file `rvr press` wrote already
//! says which events its cuts kept (`data.files` and `entry`), so when the study, its cuts, its
//! event choice and the build of rvr are the same, only the columns that are new or whose
//! definition changed are computed, over the entries a full press reads, keeping the events the
//! earlier file kept; every other column is copied from it. The file written is the one a full
//! press of the recipe writes.

use crate::samples::resolve::Lock;
use crate::samples::spec::Events;
use rowver::rvr::{Held, Reader};
use rowver::write;
use rowver::NumericBuffer;
use rvr_recipe::{names, Expression, Kind, Recipe};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// What an earlier press gives a new one.
pub struct Earlier {
    /// Per output column, in the recipe's order: its values in each sample, when copied.
    pub reused: Vec<Option<Vec<write::Values>>>,
    /// The kept entries of each file read, by sample and the file's `at`.
    pub kept: HashMap<(String, String), Vec<u64>>,
    /// Each sample's `entry` values, which the new reads must give again.
    pub entry: Vec<Vec<i64>>,
}

/// Every name an expression uses, the names of the functions and helpers it calls included.
fn used(expression: &Expression, found: &mut BTreeSet<String>) {
    use Expression::*;
    names(expression, found);
    match expression {
        Call(name, arguments) => {
            found.insert(name.clone());
            arguments.iter().for_each(|argument| used(argument, found));
        }
        Field(inner, _) | Negate(inner) | Not(inner) => used(inner, found),
        In(left, list) => {
            used(left, found);
            list.iter().for_each(|member| used(member, found));
        }
        Binary(_, left, right) | Pick(left, right) | Where(left, right) => {
            used(left, found);
            used(right, found);
        }
        Constant(_) | Name(_) => {}
    }
}

/// The recipe cut down to `roots` and the objects, helpers and collections they reach.
fn reaching(recipe: &Recipe, roots: &[&Expression]) -> Recipe {
    let mut found = BTreeSet::new();
    let mut todo: Vec<&Expression> = roots.to_vec();
    while let Some(expression) = todo.pop() {
        let mut named = BTreeSet::new();
        used(expression, &mut named);
        for name in named {
            if found.insert(name.clone()) {
                todo.extend(recipe.objects.get(&name));
                todo.extend(recipe.helpers.get(&name).map(|helper| &helper.value));
            }
        }
    }
    let reached = |name: &String| found.contains(name);
    Recipe {
        objects: recipe
            .objects
            .iter()
            .filter(|(n, _)| reached(n))
            .map(|(n, e)| (n.clone(), e.clone()))
            .collect(),
        collections: recipe
            .collections
            .iter()
            .filter(|(n, _)| reached(n))
            .map(|(n, c)| (n.clone(), c.clone()))
            .collect(),
        helpers: recipe
            .helpers
            .iter()
            .filter(|(n, _)| reached(n))
            .map(|(n, h)| (n.clone(), h.clone()))
            .collect(),
        ..Recipe::default()
    }
}

/// The recipe cut down to reading `columns`: no cuts, and only what those columns reach.
pub fn only(recipe: &Recipe, columns: &[&String]) -> Recipe {
    let pick = |list: &[(String, Expression)]| -> Vec<(String, Expression)> {
        list.iter()
            .filter(|(n, _)| columns.contains(&n))
            .cloned()
            .collect()
    };
    let (copy, computed) = (pick(&recipe.copy), pick(&recipe.columns));
    let roots: Vec<&Expression> = copy.iter().chain(&computed).map(|(_, e)| e).collect();
    let reached = reaching(recipe, &roots);
    Recipe {
        stored: recipe
            .stored
            .iter()
            .filter(|(n, _)| columns.contains(n))
            .map(|(n, k)| (n.clone(), *k))
            .collect(),
        copy,
        columns: computed,
        ..reached
    }
}

/// The recipe cut down to its cuts and what they reach.
fn cuts(recipe: &Recipe) -> Recipe {
    let roots: Vec<&Expression> = recipe
        .keep
        .iter()
        .flat_map(|cut| cut.when.iter().chain([&cut.pass]))
        .collect();
    Recipe {
        keep: recipe.keep.clone(),
        ..reaching(recipe, &roots)
    }
}

/// One column's values in each sample, cut from the whole column by the samples' rows.
fn by_sample(held: Held, rows: &[usize]) -> Result<Vec<write::Values>, String> {
    let width = Kind::named(&held.kind)
        .map_err(|_| format!("its column kind {} is unknown", held.kind))?
        .width();
    let mut out = Vec::new();
    let mut first = 0;
    for &n in rows {
        let last = first + n;
        let buffer = |from: usize, to: usize| NumericBuffer {
            kind: held.kind.clone(),
            count: to - from,
            bytes: held.bytes[from * width..to * width].to_vec(),
        };
        out.push(match &held.offsets {
            None => write::Values::Flat(buffer(first, last)),
            Some(offsets) => write::Values::Jagged {
                counts: offsets[first..=last]
                    .windows(2)
                    .map(|w| w[1] - w[0])
                    .collect(),
                content: buffer(offsets[first], offsets[last]),
            },
        });
        first = last;
    }
    Ok(out)
}

/// What the file at `path` gives this press: nothing when there is none, or why it cannot.
pub fn earlier(
    path: &Path,
    texts: &crate::press::Texts,
    recipe: &Recipe,
    lock: &Lock,
    asked: &[Events],
    outputs: &[(&String, &Expression, bool)],
) -> Option<Result<Earlier, String>> {
    path.exists()
        .then(|| read(path, texts, recipe, lock, asked, outputs))
}

fn read(
    path: &Path,
    texts: &crate::press::Texts,
    recipe: &Recipe,
    lock: &Lock,
    asked: &[Events],
    outputs: &[(&String, &Expression, bool)],
) -> Result<Earlier, String> {
    let file = Reader::open(path).map_err(|e| format!("it could not be read ({e})"))?;
    let header = file.header();
    let made = &header["made"];
    let text = |key: &str| made[key]["text"].as_str();
    if header.get("derived").is_some() {
        return Err("it is a derived file, not a press".into());
    }
    if text("lock") != Some(texts.lock.as_str()) || text("samples") != Some(texts.samples.as_str())
    {
        return Err("samples.toml or samples.lock has changed since".into());
    }
    // Columns computed by another build of rvr (another version, commit or compiler) are not
    // copied: the new file names this build.
    if made["toolchain"] != crate::press::toolchain() {
        return Err(format!(
            "it was made by another build of rvr ({} at {})",
            made["toolchain"]["rvr"].as_str().unwrap_or("?"),
            made["toolchain"]["commit"].as_str().unwrap_or("?")
        ));
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
    let samples = header["data"]["samples"]
        .as_array()
        .ok_or("it has no samples")?;
    let names: Vec<&str> = samples.iter().filter_map(|s| s["name"].as_str()).collect();
    if names
        != lock
            .sample
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
    {
        return Err("its samples are not the lock's".into());
    }
    let rows: Vec<usize> = samples
        .iter()
        .map(|s| s["rows"].as_u64().unwrap_or(0) as usize)
        .collect();
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
        kept.insert(
            (sample.to_owned(), path.to_owned()),
            span.iter().map(|&e| e as u64).collect(),
        );
        at += n;
    }
    let mut entry = Vec::new();
    let mut first = 0;
    for &n in &rows {
        entry.push(
            entries
                .get(first..first + n)
                .ok_or("its samples hold more rows than it has")?
                .to_vec(),
        );
        first += n;
    }
    let mut reused = Vec::new();
    for &(name, _, _) in outputs {
        let same = only(&old, &[name]) == only(recipe, &[name]);
        let held = match same {
            true => file.column(name).map_err(failed)?,
            false => None,
        };
        reused.push(held.map(|held| by_sample(held, &rows)).transpose()?);
    }
    Ok(Earlier {
        reused,
        kept,
        entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
format = "rvr-recipe"
version = 1

[object.good_jet]
from = "Jet"
where = "pt > 30"

[helper.jets_above]
takes = ["threshold"]
value = "count(Jet where pt > threshold)"

[keep]
two_jets = "count(good_jet) >= 2"
one_hard = "jets_above(50) >= 1"

[copy]
MET = "MET_pt"

[column]
n_jet = "count(good_jet)"
n_hard = "jets_above(80)"
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
        let added = BASE.replace("[column]", "[column]\nht = \"sum(good_jet.pt)\"");
        for column in ["MET", "n_jet", "n_hard"] {
            assert!(same(BASE, &added, column), "{column}");
        }
        assert!(!same(BASE, &added, "ht"), "a new column is read");
        assert!(same_cuts(BASE, &added));
        // A changed object changes what reaches it, and only that.
        let tighter = BASE.replace("pt > 30", "pt > 40");
        assert!(!same(BASE, &tighter, "n_jet"));
        assert!(same(BASE, &tighter, "MET"), "MET reaches no object");
        assert!(
            same(BASE, &tighter, "n_hard"),
            "n_hard reaches the helper, not the object"
        );
        assert!(!same_cuts(BASE, &tighter));
        // A changed helper, which is only ever called, changes what calls it.
        let helper = BASE.replace("pt > threshold", "pt >= threshold");
        assert!(!same(BASE, &helper, "n_hard"));
        assert!(same(BASE, &helper, "n_jet"));
        assert!(!same_cuts(BASE, &helper));
        // A column stored as another kind is changed.
        let stored = BASE.replace(
            "n_jet = \"count(good_jet)\"",
            "n_jet = { value = \"count(good_jet)\", stored = \"i2\" }",
        );
        assert!(!same(BASE, &stored, "n_jet"));
        // Reading a column needs only what it reaches.
        let hard = String::from("n_hard");
        let read = only(&Recipe::read(BASE).unwrap(), &[&hard]);
        assert!(read.keep.is_empty() && read.objects.is_empty());
        assert_eq!(read.helpers.keys().collect::<Vec<_>>(), ["jets_above"]);
    }
}
