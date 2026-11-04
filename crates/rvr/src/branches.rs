//! `rvr branches`: what a study's sample holds, as a recipe names it. Every branch of the sample's
//! branch list in the lock (its first file's tree), in alphabetical order, with the kind rvr reads it
//! as and whether it holds one value or a list per event; no file is opened. Before a lock exists,
//! the sample's first file, found through samples.toml as resolve finds it, is read instead.

use crate::options::Options;
use rowver::rvr::grouped as count;
use std::path::Path;

pub(crate) const USAGE: &str = "usage:
  rvr branches [DIR] [--sample NAME] [--find TEXT]
      the branches of the first file of the first sample (or NAME) of the study in DIR (.),
      in alphabetical order; --find shows only the branches whose name holds TEXT (in any
      case)";

/// `rvr branches` with its options: the exit status.
pub fn main(options: &Options) -> u8 {
    crate::report("branches", run(options).map(|()| 0))
}

fn run(options: &Options) -> Result<(), String> {
    let find = options.value("--find").map(str::to_lowercase);
    let dir = Path::new(options.words.first().map_or(".", String::as_str));
    let (name, tree, at, list) = match dir.join("samples.lock").exists() {
        true => {
            let lock = crate::texts::lock(dir)?;
            // It needs only the lock, but says when the lock is not of samples.toml as it is now.
            let samples = std::fs::read_to_string(dir.join("samples.toml")).unwrap_or_default();
            if crate::agree::stale(&samples, &lock) {
                println!("samples.toml has changed since samples.lock was written: these are the files of the lock ({}).", lock.resolved);
            }
            let sample = &lock.samples[crate::open::sample_in(&lock, options.value("--sample"))?];
            let list = lock.branches_of(sample).expect("the lock validator checks every sample names a branch list it holds");
            let at = sample.files.first().map_or(String::new(), |f| f.at.clone());
            (sample.name.clone(), sample.tree.clone(), at, list.clone())
        }
        false => {
            let (sample, at, list) = crate::samples::first_file(&dir.join("samples.toml"), options.value("--sample"))?;
            (sample.name, sample.tree, at, list)
        }
    };
    let first = at.rsplit('/').next().unwrap_or(&at);
    let found = |name: &str| find.as_ref().is_none_or(|text| name.to_lowercase().contains(text));
    let total = count(list.list.len() as u64);
    match &find {
        None => println!("{name}: {first}, tree {tree}, {total} branches"),
        Some(text) => {
            let n = list.list.iter().filter(|b| found(&b.name)).count();
            println!("{name}: {first}, tree {tree}, {n} of its {total} branches hold {text:?}")
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
