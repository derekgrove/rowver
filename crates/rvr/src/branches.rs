//! `rvr branches`: what a study's sample holds, as a recipe names it. Every branch of the sample's
//! branch list in the lock (its first file's tree), in alphabetical order, with the kind rvr reads it
//! as and whether it holds one value or a list per event. No file is opened.

use crate::press::{self, count};
use crate::Words;
use std::path::Path;

const USAGE: &str = "usage:
  rvr branches [DIR] [--sample NAME] [--find TEXT]
      the branches of the first file of the first sample (or NAME) of the study in DIR (.),
      in alphabetical order; --find shows only the branches whose name holds TEXT (in any
      case)";

/// `rvr branches …` with the words after `branches`: the exit status (2 for a wrong use of it).
pub fn main(args: &[&str]) -> u8 {
    match run(args) {
        Ok(()) => 0,
        Err(error) if error.ends_with(USAGE) => {
            eprintln!("rvr branches: {error}");
            2
        }
        Err(error) => {
            eprintln!("rvr branches: {error}");
            1
        }
    }
}

fn run(args: &[&str]) -> Result<(), String> {
    let words = Words::read(args, &["--sample", "--find"], &[], USAGE)?;
    let find = words.value("--find").map(str::to_lowercase);
    let dir = match words.plain[..] {
        [] => Path::new("."),
        [dir] => Path::new(dir),
        _ => return Err(format!("branches takes at most one study folder.\n{USAGE}")),
    };
    let (_, lock) = press::lock(dir)?;
    // It needs only the lock, but says when the lock is not of samples.toml as it is now.
    let samples = std::fs::read_to_string(dir.join("samples.toml")).unwrap_or_default();
    if rowver::write::text_checksum(&samples) != lock.samples_toml_sha256 {
        println!("samples.toml has changed since samples.lock was written: these are the files of the lock ({}).", lock.resolved);
    }
    let index = press::sample_at(&lock, words.value("--sample"))?;
    let sample = &lock.samples[index];
    let list = lock.branches_of(sample).ok_or_else(|| format!("samples.lock has no branch list for {}: run rvr samples resolve samples.toml again", sample.name))?;
    let first = sample.files.first().map_or("", |f| f.at.rsplit('/').next().unwrap_or(&f.at));
    let found = |name: &str| find.as_ref().is_none_or(|text| name.to_lowercase().contains(text));
    let total = count(list.list.len() as u64);
    match &find {
        None => println!("{}: {first}, tree {}, {total} branches", sample.name, sample.tree),
        Some(text) => {
            let n = list.list.iter().filter(|b| found(&b.name)).count();
            println!("{}: {first}, tree {}, {n} of its {total} branches hold {text:?}", sample.name, sample.tree)
        }
    }
    // One line per branch, in alphabetical order whatever the case: name, kind and shape.
    let mut listed: Vec<(&str, &str, String)> = Vec::new();
    let mut unreadable = Vec::new();
    for branch in list.list.iter().filter(|b| found(&b.name)) {
        let shape = branch.shape.to_string();
        match &branch.why {
            Some(why) => unreadable.push((branch.name.as_str(), why.as_str())),
            None => listed.push((&branch.name, &branch.kind, shape)),
        }
    }
    listed.sort_by_cached_key(|(name, _, _)| name.to_lowercase());
    let width = listed.iter().map(|(name, _, _)| name.len()).max().unwrap_or(0);
    println!();
    for (name, kind, shape) in &listed {
        println!("  {name:width$}  {kind:2}  {shape}");
    }
    if !unreadable.is_empty() {
        println!("\nNot readable by rvr:");
        for (name, why) in unreadable {
            println!("  {name}  ({why})");
        }
    }
    Ok(())
}
