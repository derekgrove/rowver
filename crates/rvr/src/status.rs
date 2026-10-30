//! `rvr status`: whether a study's `.rvr` file holds the study as it is now, and what `rvr press`
//! would read to bring it up to date, from the study's texts and the file (no input is read).

use crate::press;
use crate::reuse;
use crate::samples::Choice;
use rvr_recipe::Recipe;
use std::path::PathBuf;

/// How `rvr status` is used.
pub const USAGE: &str = "usage:
  rvr status [DIR] [--out FILE] [--events N|all] [--random] [--seed S]
                                           whether DIR/<its name>.rvr (or FILE) holds the study
                                           in DIR as it is now, and what rvr press would read;
                                           exit status 0 when it does";

/// `rvr status …` with the words after `status`: 0 when the file holds the study as it is.
pub fn main(args: &[&str]) -> u8 {
    match run(args) {
        Ok(current) => u8::from(!current),
        Err(why) => {
            eprintln!("rvr status: {why}");
            1
        }
    }
}

fn run(args: &[&str]) -> Result<bool, String> {
    let (mut dir, mut out) = (None, None);
    let mut choice = Choice::default();
    let mut words = args.iter();
    while let Some(&word) = words.next() {
        let mut given = || {
            words
                .next()
                .copied()
                .filter(|v| !v.starts_with("--"))
                .ok_or(format!("{word} needs a value"))
        };
        match word {
            "--events" => choice.events = Some(given()?),
            "--random" => choice.random = true,
            "--seed" => choice.seed = Some(Choice::seed(given()?)?),
            "--out" => out = Some(PathBuf::from(given()?)),
            flag if flag.starts_with("--") => {
                return Err(format!("status does not know {flag}.\n{USAGE}"))
            }
            folder if dir.is_none() => dir = Some(PathBuf::from(folder)),
            _ => return Err(format!("status takes one study folder.\n{USAGE}")),
        }
    }
    let (dir, _, target) = press::study(dir, out)?;
    let (texts, mut lock) = press::texts(&dir)?;
    let resolved = press::resolved(&texts, &lock);
    if !resolved {
        println!("samples.toml has changed since samples.lock was written: run rvr samples resolve samples.toml and review the new lock.");
    }
    if !target.exists() {
        println!("{} is not there: rvr press makes it.", target.display());
        return Ok(false);
    }
    let header = rowver::rvr::read_header(&target)
        .map_err(|e| format!("{} could not be read: {e}", target.display()))?;
    let made = &header["made"];
    let held = |key: &str| made[key]["text"].as_str().unwrap_or_default();
    println!(
        "{}: pressed {} by {}",
        target.display(),
        made["built"].as_str().unwrap_or("?"),
        made["by"].as_str().unwrap_or("?")
    );
    choice.check(&lock.sample)?;
    let asked = press::plan(&mut lock, &|s| choice.of(s))?;
    let chosen = lock
        .sample
        .iter()
        .zip(&asked)
        // A sample the file does not hold is a change of samples.toml, not of the events chosen.
        .all(|(sample, want)| {
            let held = &made["files"][&sample.name];
            held.is_null() || held["events"] == want.record()
        });
    let same = [
        held("recipe") == texts.recipe,
        held("samples") == texts.samples,
        held("lock") == texts.lock,
        chosen,
    ];
    let as_held = "as the file holds it";
    let recipe_said = match same[0] {
        true => as_held.to_string(),
        false => changes(held("recipe"), &texts.recipe),
    };
    println!("  recipe.toml   {recipe_said}");
    let said = [
        ("samples.toml", "edited since"),
        ("samples.lock", "resolved again since"),
        (
            "events",
            "the file read others (another --events, --random or --seed)",
        ),
    ];
    for ((what, other), same) in said.iter().zip(&same[1..]) {
        println!("  {what:13} {}", if *same { as_held } else { other });
    }
    let current = same.iter().all(|s| *s);
    if !resolved {
        println!("rvr press refuses until samples.lock is resolved again.");
        return Ok(false);
    }
    let new = press::read_recipe(&texts.recipe)?;
    let outputs = press::outputs(&new);
    let said = match reuse::earlier(&target, &texts, &new, &lock, &asked, &outputs) {
        Some(Ok(earlier)) => {
            let reading: Vec<&str> = outputs
                .iter()
                .zip(&earlier.reused)
                .filter(|(_, reused)| !**reused)
                .map(|((name, _, _), _)| name.as_str())
                .collect();
            match (reading.is_empty(), current) {
                (true, true) => "It is current: rvr press has nothing to read.".to_string(),
                (true, false) => "rvr press would read nothing: every column is copied from it into a file that holds the study as it is now.".into(),
                (false, _) => format!(
                    "rvr press would read only {}, for the events the file kept, and copy every other column from it.",
                    reading.join(", ")
                ),
            }
        }
        Some(Err(why)) if current => format!("It holds the study as it is now, but rvr press would press every column again, as {why}."),
        Some(Err(why)) => format!("rvr press would press every column, as {why}."),
        None => unreachable!("the file is there"),
    };
    println!("{said}");
    Ok(current)
}

/// What changed from one recipe to another, in the columns and cuts it stores and keeps.
fn changes(old: &str, now: &str) -> String {
    let (Ok(old), Ok(now)) = (Recipe::read(old), Recipe::read(now)) else {
        return "changed since (one of the two cannot be read as a recipe)".into();
    };
    let (before, after) = (press::outputs(&old), press::outputs(&now));
    let names = |outputs: &[press::Output]| -> Vec<String> {
        outputs.iter().map(|(n, _, _)| (*n).clone()).collect()
    };
    let (before, after) = (names(&before), names(&after));
    let mut said = Vec::new();
    let list = |what: &str, names: Vec<&String>| {
        let names: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        (!names.is_empty()).then(|| format!("{what} {}", names.join(", ")))
    };
    said.extend(list(
        "new:",
        after.iter().filter(|n| !before.contains(n)).collect(),
    ));
    said.extend(list(
        "changed:",
        after
            .iter()
            .filter(|n| before.contains(n) && reuse::only(&old, &[n]) != reuse::only(&now, &[n]))
            .collect(),
    ));
    said.extend(list(
        "dropped:",
        before.iter().filter(|n| !after.contains(n)).collect(),
    ));
    if reuse::cuts(&old) != reuse::cuts(&now) {
        said.push("the cuts under [keep] changed".into());
    }
    match said.is_empty() {
        true => "edited since, with every cut and column the same".into(),
        false => format!("changed since: {}", said.join("; ")),
    }
}
