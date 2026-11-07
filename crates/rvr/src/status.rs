//! `rvr status`: whether a study's `.rvr` file holds the study as it is now, and what `rvr press`
//! would read to bring it up to date, from the study's texts and the file (no input is read).

use crate::options::Options;
use crate::plan::{Asked, At, Base, Choose, On, Place};
use crate::reuse;
use crate::samples::{some, Choice};
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

/// `rvr status` with its options: 0 when `rvr press` with them would have nothing to do.
pub fn main(options: &Options) -> u8 {
    let choice = match Choice::read(options) {
        Ok(choice) => choice,
        Err(why) => return crate::wrong("status", &why),
    };
    let (dir, out) = (options.words.first().map(PathBuf::from), options.value("--out").map(PathBuf::from));
    crate::report("status", run(dir, out, choice, options.has("--holes")).map(|current| u8::from(!current)))
}

fn run(dir: Option<PathBuf>, out: Option<PathBuf>, choice: Choice, every: bool) -> Result<bool, String> {
    let (dir, study, target) = crate::texts::study(dir, out)?;
    crate::plan::a_file_or_none(&target)?;
    // Through a link, the file it names, and the press staged beside it, as rvr press finds them.
    let target = rowver::change::resolved(&target);
    let texts = crate::texts::read(&dir)?;
    let lock = rowver::study::lock::Lock::read(&texts.lock).ok();
    let names: Vec<String> = lock.iter().flat_map(|lock| lock.samples.iter().map(|s| s.name.clone())).collect();
    // Said only of a lock that reads: one that does not is among what rvr press would refuse.
    let stale = lock.as_ref().is_some_and(|lock| crate::agree::stale(&texts.samples, lock));
    if stale {
        println!("samples.toml has changed since samples.lock was written: run rvr samples resolve samples.toml and review the new lock.");
    }
    let stopped = crate::journal::folder(&target);
    if crate::journal::under_way(&stopped) {
        println!("{}: a press is under way there now.", stopped.display());
    }
    // A change to it cut short is put back first, or its journal refused, as rvr press does.
    crate::restored(&target)?;
    // The file there, opened once for what is said of it here and for the Plan.
    let at = At::read(&target);
    let header = match &at {
        At::Opened(file, _) => file.header().clone(),
        // One that cannot be read stops a press, as here; so does one a change to which is under
        // way, or was cut short since it was put back.
        At::Unopened(e) if matches!(e, rowver::Error::Io(_)) || rowver::change::journal(&target).exists() => return Err(rowver::plain(&target, e)),
        // None there, or one that does not open, which a press builds nothing on (SPEC §8.12).
        _ => return anew(dir, study, target, texts, choice, at),
    };
    // The Plan rvr press would make, with these options, of the file there.
    let place = Place { base: dir, study, target: target.clone(), at, locked: None, found: None };
    let asked = Asked { choose: Choose::Options(choice.clone()), on: On::Earlier, workers: 1, sign: true };
    let plan = crate::plan::build(place, texts.clone(), asked);
    let pressing = plan.as_ref().is_ok_and(|plan| !matches!(plan.from, Base::Current(_)));
    let current = plan.as_ref().ok().filter(|plan| matches!(plan.from, Base::Current(0))).map(|plan| &plan.identity);
    if let Some(note) = stopped_note(&stopped, &target, current, pressing) {
        println!("{note}");
    }
    let made = &header["made"];
    let held = |key: &str| made[key]["text"].as_str().unwrap_or_default();
    println!("{}: pressed {} by {}", target.display(), made["built"].as_str().unwrap_or("?"), made["by"].as_str().unwrap_or("?"));
    let events = match &plan {
        Ok(plan) => Some(plan.asked.clone()),
        Err(_) => lock.and_then(|lock| crate::agree::planned(lock, &Choose::Options(choice)).ok()).map(|(_, asked)| asked),
    };
    // A sample the file does not hold is a change of samples.toml, not of the events chosen.
    let chosen = events.as_ref().is_none_or(|events| names.iter().zip(events).all(|(name, want)| made["files"][name].is_null() || made["files"][name]["events"] == want.record()));
    let same = [held("recipe") == texts.recipe, held("samples") == texts.samples, held("lock") == texts.lock, chosen];
    let as_held = "as the file holds it";
    let recipe_said = match same[0] {
        true => as_held.to_string(),
        false => changes(held("recipe"), &texts.recipe),
    };
    println!("  recipe.toml   {recipe_said}");
    let said = [("samples.toml", "edited since"), ("samples.lock", "resolved again since"), ("events", "the file read others (another --events, --random or --seed)")];
    for ((what, other), same) in said.iter().zip(&same[1..]) {
        println!("  {what:13} {}", if *same { as_held } else { other });
    }
    let plan = match plan {
        Ok(plan) => plan,
        Err(why) => {
            refused(&why);
            return Ok(false);
        }
    };
    let said = match (&plan.from, &plan.why) {
        (Base::Samples(fill), _) => {
            let added = fill.added.clone().unwrap_or_default();
            let files: usize = plan.lock.samples.iter().filter(|s| added.contains(&s.name)).map(|s| s.files.iter().filter(|f| f.stop != Some(0)).count()).sum();
            match added.is_empty() {
                true => "rvr press would read nothing: the rows of every sample are copied from it into a file that holds the study as it is now.".into(),
                false => format!("rvr press would read only the new {} ({}), and copy the rows of every other sample from it.", crate::core::named(&added), some(files, "file", "files")),
            }
        }
        (Base::Current(0), _) => "It is current: rvr press has nothing to read.".to_string(),
        (Base::Current(_), _) => "It holds the study as it is, but for its holes: rvr press --fill reads them.".to_string(),
        (Base::Columns(earlier), _) if plan.reading.is_empty() && earlier.in_place => "rvr press would read nothing: it would add the new derived columns to the file in place.".into(),
        (Base::Columns(_), _) if plan.reading.is_empty() => "rvr press would read nothing: every column is copied from it into a file that holds the study as it is now.".into(),
        (Base::Columns(earlier), _) => {
            let reading: Vec<&str> = plan.reading.iter().map(|at| plan.recipe.columns[*at].0.as_str()).collect();
            match earlier.in_place {
                true => format!("rvr press would read only {}, for the events the file kept, and add them to the file in place.", reading.join(", ")),
                false => format!("rvr press would read only {}, for the events the file kept, and copy every other column from it.", reading.join(", ")),
            }
        }
        (_, Some(both)) if plan.other_build => format!("{both} rvr press would ask whether to press every column again, as a file is built on only by the build that made it (rvr press --full presses them without asking)."),
        (_, Some(why)) if same.iter().all(|s| *s) => format!("It holds the study as it is now, but rvr press would press every column again, as {why}."),
        (_, Some(why)) => format!("rvr press would press every column, as {why}."),
        (_, None) => "rvr press would press every column.".into(),
    };
    println!("{said}");
    match (&plan.refused, &plan.anew) {
        (Some(why), _) => println!("{why}."),
        (None, Some(why)) => crate::told_anew(&target, why, true),
        (None, None) => {}
    }
    // Whether --fill would fill it, to say so only when it would.
    let kinds = crate::fill::kinds(None);
    let fillable = crate::fill::fits(&header, &target.display().to_string(), &plan.texts, &plan.lock, &plan.asked, &kinds).map(drop);
    completeness(&header, every, fillable);
    // The Plan alone says whether rvr press would have nothing to do.
    Ok(matches!(plan.from, Base::Current(0)))
}

/// With no file at the target (`at`), or one that does not open: what rvr press would read (the
/// Plan's reads), or why it would refuse, then that there is no file yet, or why it does not open.
/// Never current.
fn anew(dir: PathBuf, study: String, target: PathBuf, texts: crate::texts::Texts, choice: Choice, at: At) -> Result<bool, String> {
    let unopened = matches!(at, At::Unopened(_));
    let there = match &at {
        At::Unopened(e) => format!("{} does not open: {e}", target.display()),
        _ => format!("{} is not there yet: rvr press makes it.", target.display()),
    };
    let place = Place { base: dir, study, target: target.clone(), at, locked: None, found: None };
    let asked = Asked { choose: Choose::Options(choice), on: On::Earlier, workers: 1, sign: true };
    let stopped = crate::journal::folder(&target);
    match crate::plan::build(place, texts, asked) {
        Ok(plan) => match crate::journal::read(&stopped, &plan.identity) {
            // The press stopped there is this one: what is left of it, in one statement.
            crate::journal::Found::Same(done) if !done.inputs.is_empty() => {
                let left: Vec<&crate::plan::Input> = plan.reads().iter().enumerate().filter(|(at, _)| !done.inputs.contains_key(at)).map(|(_, input)| input).collect();
                let entries: u64 = left.iter().map(|input| crate::take::chosen_of(input)).sum();
                println!(
                    "{}: a press stopped there with {} of {} done (journal from {}); rvr press carries it on, reading the {} left, {}.",
                    stopped.display(),
                    done.inputs.len(),
                    some(plan.reads().len(), "file", "files"),
                    done.when,
                    some(left.len(), "file", "files"),
                    some(entries as usize, "entry", "entries")
                );
            }
            _ => {
                if let Some(note) = stopped_note(&stopped, &target, None, true) {
                    println!("{note}");
                }
                println!("rvr press would press every column: {}.", crate::core::pressed(&plan));
            }
        },
        Err(why) => {
            if let Some(note) = stopped_note(&stopped, &target, None, false) {
                println!("{note}");
            }
            refused(&why);
            // A file there that does not open, when the refusal says it, is not said again.
            if unopened && why.contains(&format!("{} does not open", target.display())) {
                return Ok(false);
            }
        }
    }
    println!("{there}");
    Ok(false)
}

/// What rvr press would refuse with, one problem a line.
fn refused(why: &str) {
    println!("rvr press would refuse:");
    for line in why.lines() {
        println!("  {line}");
    }
}

/// A press stopped in `stopped` (its staging folder), said; none, nothing. One that left it for
/// nothing (`current`: the Plan of the file there, current) is said to be removed by the next
/// press; one that saved no finished file, to start over only when a press would read (`pressing`).
fn stopped_note(stopped: &std::path::Path, target: &std::path::Path, current: Option<&serde_json::Value>, pressing: bool) -> Option<String> {
    if crate::journal::under_way(stopped) {
        return None;
    }
    if let Some(why) = crate::journal::left(stopped, target, current) {
        return Some(format!("{}: {why}; rvr press removes it.", stopped.display()));
    }
    let (press, done) = crate::journal::unfinished(stopped)?;
    if done.inputs.is_empty() {
        let what = match done.named {
            0 => "before it finished a file",
            _ => "with no finished file saved",
        };
        let then = if pressing { "; rvr press starts it over." } else { "." };
        return Some(format!("{}: a press stopped there {what} (journal from {}){then}", stopped.display(), done.when));
    }
    let inputs = press["inputs"].as_u64().unwrap_or(0) as usize;
    Some(format!(
        "{}: a press stopped there with {} of {} done (journal from {}). rvr press carries on from there when the study and the press are the same, and --restart starts over.",
        stopped.display(),
        done.inputs.len(),
        some(inputs, "file", "files"),
        done.when
    ))
}

/// How much of what it chose the file holds, from its header: one bar per sample with the files its
/// choice read, then its holes by kind, or (with `every`) one a line, and what `--fill` would read.
fn completeness(header: &Value, every: bool, fillable: Result<(), String>) {
    let made = &header["made"];
    let holes = rowver::rvr::holes(header);
    let names: Vec<&str> = header["data"]["samples"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).collect();
    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    println!();
    for name in &names {
        let sample = &made["files"][*name];
        let read = sample["metadata"]["events_read"].as_u64().unwrap_or(0);
        let missing = sample["metadata"]["events_missing"].as_u64().unwrap_or(0);
        let (bar, percent) = crate::progress::cells(read, read + missing, 24);
        // A file the choice took no entry of (`stop` or `take` 0) is not counted, as rvr samples plan counts.
        let files = sample["files"].as_object().into_iter().flatten().filter(|(_, f)| f["stop"] != 0 && f["take"] != 0).count();
        let holed = holes.iter().filter(|h| h.sample == *name).count();
        let holed = match holed {
            0 => String::new(),
            n => format!(", {}", some(n, "hole", "holes")),
        };
        println!("  {name:width$}  {bar} {percent:>3.0}%   {}{holed}", some(files, "file", "files"));
    }
    if holes.is_empty() {
        return;
    }
    let chosen = match rowver::rvr::chosen_in_holes(header) {
        Ok(chosen) if chosen.len() == holes.len() => chosen,
        _ => {
            println!("\nIts holes cannot be counted: the file is not valid (rvr validate says why).");
            return;
        }
    };
    if every {
        println!("\nHoles:");
        for (hole, n) in holes.iter().zip(&chosen) {
            println!("  {}  {}  {} ({n} chosen)  {}: {}", hole.sample, hole.file, rowver::rvr::span(hole.first, hole.last), hole.kind, hole.why);
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
            println!("  {kind:12} {runs}, {}   {}", some(entries as usize, "entry", "entries"), samples.join(", "));
            println!("      {}", first.why);
        }
        println!("(rvr status --holes lists every hole.)");
    }
    let files: BTreeSet<(&str, &str)> = holes.iter().map(|h| (h.sample.as_str(), h.file.as_str())).collect();
    match fillable {
        Ok(()) => println!("rvr press --fill would read {} of {}.", some(chosen.iter().sum::<u64>() as usize, "entry", "entries"), some(files.len(), "file", "files")),
        Err(why) => println!("rvr press --fill would not: {why}"),
    }
}

/// What changed from one recipe to another, in the columns and cuts it stores and keeps.
fn changes(old: &str, now: &str) -> String {
    let (Ok(old), Ok(now)) = (Recipe::read(old), Recipe::read(now)) else {
        return "changed since (one of the two cannot be read as a recipe)".into();
    };
    let names = |recipe: &Recipe| -> Vec<String> {
        recipe.columns.iter().map(|(n, _)| n.clone()).collect()
    };
    let (before, after) = (names(&old), names(&now));
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
    if reuse::totaled(&old) != reuse::totaled(&now) {
        said.push("the totals under [totals] changed".into());
    }
    // A derived column is its name and expression: one with another expression is changed.
    let derived = |recipe: &Recipe| -> Vec<String> { recipe.derived.iter().map(|(n, _)| n.clone()).collect() };
    let (was, is) = (derived(&old), derived(&now));
    let expression = |recipe: &Recipe, name: &str| recipe.derived.iter().find(|(n, _)| n == name).map(|(_, e)| e.clone());
    said.extend(list("new derived:", is.iter().filter(|n| !was.contains(n)).collect()));
    said.extend(list(
        "changed derived:",
        is.iter().filter(|n| was.contains(n) && expression(&old, n) != expression(&now, n)).collect(),
    ));
    said.extend(list("dropped derived:", was.iter().filter(|n| !is.contains(n)).collect()));
    match said.is_empty() {
        true => "edited since, with every cut and column the same".into(),
        false => format!("changed since: {}", said.join("; ")),
    }
}
