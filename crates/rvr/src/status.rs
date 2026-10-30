//! `rvr status`: whether a study's `.rvr` file holds the study as it is now, and what `rvr press`
//! would read to bring it up to date, from the study's texts and the file (no input is read).

use crate::press;
use crate::reuse;
use crate::samples::Choice;
use crate::press::count;
use crate::samples::some;
use rvr_recipe::Recipe;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// How `rvr status` is used.
pub const USAGE: &str = "usage:
  rvr status [DIR] [--out FILE] [--events N|all] [--random] [--seed S] [--holes]
                                           whether DIR/<its name>.rvr (or FILE) holds the study
                                           in DIR as it is now, how much of what it chose it
                                           holds (--holes: every hole), and what rvr press would
                                           read; exit status 0 when it is current and complete";

/// `rvr status …` with the words after `status`: 0 when the file holds the study as it is, whole.
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
    let (mut dir, mut out, mut every) = (None, None, false);
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
            "--holes" => every = true,
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
    let stopped = crate::journal::folder(&target);
    if crate::journal::under_way(&stopped) {
        println!("{}: a press is under way there now.", stopped.display());
    } else if let Some((press, done)) = crate::journal::unfinished(&stopped).filter(|(_, done)| !done.inputs.is_empty()) {
        let inputs = press["inputs"].as_u64().unwrap_or(0) as usize;
        println!(
            "{}: a press stopped there with {} of {} done (journal from {}). rvr press carries on from there when the study and the press are the same, and --restart starts over.",
            stopped.display(),
            done.inputs.len(),
            some(inputs, "file", "files"),
            done.when
        );
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
            let holed = !rowver::rvr::holes(&header).is_empty();
            match (reading.is_empty(), current) {
                (true, true) if holed => "It holds the study as it is, but for its holes: rvr press --fill reads them.".to_string(),
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
    // Whether --fill would fill it, to say so only when it would.
    let kinds = crate::fill::kinds(None)?;
    let shown = target.display().to_string();
    let fillable = crate::fill::fits(&header, &shown, &texts, &lock, &asked, &kinds).map(drop);
    let complete = completeness(&header, every, fillable);
    Ok(current && complete)
}

/// How much of what it chose the file holds, from its header alone: one bar per sample, then its
/// holes by kind with what to do about each kind, or (with `every`) one line per hole, and what
/// `--fill` would read (or why it cannot fill it). Whether it is complete.
fn completeness(header: &Value, every: bool, fillable: Result<(), String>) -> bool {
    let made = &header["made"];
    let holes = rowver::rvr::holes(header);
    let names: Vec<&str> = header["data"]["samples"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).collect();
    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    println!();
    for name in &names {
        let sample = &made["files"][*name];
        let read = sample["metadata"]["events_read"].as_u64().unwrap_or(0);
        let missing = sample["metadata"]["events_missing"].as_u64().unwrap_or(0);
        let part = if read + missing == 0 { 1.0 } else { read as f64 / (read + missing) as f64 };
        let full = (part * 24.0).floor() as usize;
        let full = if missing > 0 { full.min(23) } else { full };
        let bar = format!("{}{}", "█".repeat(full), "░".repeat(24 - full));
        let files = sample["files"].as_object().map_or(0, |f| f.len());
        let holed = holes.iter().filter(|h| h.sample == *name).count();
        let holed = match holed {
            0 => String::new(),
            n => format!(", {}", some(n, "hole", "holes")),
        };
        let percent = if missing > 0 { (part * 100.0).floor().min(99.0) } else { 100.0 };
        println!("  {name:width$}  {bar} {percent:>3.0}%   {}{holed}", some(files, "file", "files"));
    }
    if holes.is_empty() {
        return true;
    }
    let chosen = match rowver::rvr::chosen_in_holes(header) {
        Ok(chosen) if chosen.len() == holes.len() => chosen,
        _ => {
            println!("\nIts holes cannot be counted: the file is not valid (rvr validate says why).");
            return false;
        }
    };
    if every {
        println!("\nHoles:");
        for (hole, n) in holes.iter().zip(&chosen) {
            let name = hole.file.rsplit('/').next().unwrap_or(&hole.file);
            println!("  {}  {name}  entries {} to {} ({n} chosen)  {}: {}", hole.sample, count(hole.first), count(hole.last.saturating_sub(1)), hole.kind, hole.why);
        }
    } else {
        println!("\nHoles, by why:");
        for kind in rowver::rvr::HOLE_KINDS {
            let these: Vec<(&rowver::rvr::Hole, u64)> = holes.iter().zip(&chosen).filter(|(h, _)| h.kind == kind).map(|(h, n)| (h, *n)).collect();
            let Some((first, _)) = these.first() else { continue };
            let files: BTreeSet<(&str, &str)> = these.iter().map(|(h, _)| (h.sample.as_str(), h.file.as_str())).collect();
            let samples: BTreeSet<&str> = these.iter().map(|(h, _)| h.sample.as_str()).collect();
            let entries: u64 = these.iter().map(|(_, n)| n).sum();
            let runs = match these.len() == files.len() {
                true => some(files.len(), "file", "files"),
                false => format!("{} in {}", some(these.len(), "hole", "holes"), some(files.len(), "file", "files")),
            };
            let samples: Vec<&str> = samples.into_iter().collect();
            println!("  {kind:12} {runs}, {} entries   {}", count(entries), samples.join(", "));
            println!("      {}", first.why);
        }
        println!("(rvr status --holes lists every hole.)");
    }
    let files: BTreeSet<(&str, &str)> = holes.iter().map(|h| (h.sample.as_str(), h.file.as_str())).collect();
    match fillable {
        Ok(()) => println!("rvr press --fill would read {} entries of {}.", count(chosen.iter().sum()), some(files.len(), "file", "files")),
        Err(why) => println!("rvr press --fill would not: {why}"),
    }
    false
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
