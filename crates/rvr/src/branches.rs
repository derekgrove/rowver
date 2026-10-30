//! `rvr branches`: what a study's file holds, as a recipe names it. Every branch of the first
//! file of a sample, in alphabetical order, with its type and whether it holds one value or a
//! list per event. Read from the file's metadata alone, so no events are read.

use crate::press::{self, count};
use crate::Words;
use hep_root::columns::{inventory, Shape};
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
    if crate::samples::sha256(samples.as_bytes()) != lock.samples_toml_sha256 {
        println!("samples.toml has changed since samples.lock was written: these are the files of the lock ({}).", lock.resolved);
    }
    let index = press::sample_at(&lock, words.value("--sample"))?;
    let sample = &lock.sample[index];
    let (root, file) = press::first_readable(&lock, index, dir, false)?;
    let at = &file.at;
    let tree = root.tree_at(&sample.tree).map_err(|e| format!("{at}: its tree {:?}: {e}", sample.tree))?;
    let branches = inventory(&tree);
    let found = |name: &str| find.as_ref().is_none_or(|text| name.to_lowercase().contains(text));
    let shown = at.rsplit('/').next().unwrap_or(at);
    let total = count(branches.len() as u64);
    match &find {
        None => println!("{}: {shown}, tree {}, {total} branches", sample.name, sample.tree),
        Some(text) => {
            let n = branches.iter().filter(|b| found(&b.path)).count();
            println!("{}: {shown}, tree {}, {n} of its {total} branches hold {text:?}", sample.name, sample.tree)
        }
    }
    // One line per branch, in alphabetical order whatever the case: name, type and shape.
    let mut listed: Vec<(&str, String, String)> = Vec::new();
    let mut unreadable = Vec::new();
    for branch in branches.iter().filter(|b| found(&b.path)) {
        let element = branch.type_name.split(['[', '<']).next().unwrap_or("").to_owned();
        let (kind, shape) = match &branch.shape {
            Shape::Unreadable { reason } => {
                unreadable.push((branch.path.as_str(), reason.clone()));
                continue;
            }
            Shape::Scalar => (element, "flat".to_owned()),
            Shape::Fixed(n) => (element, format!("{n} per event")),
            Shape::Jagged { .. } | Shape::JaggedFixed { .. } => (element, "jagged".to_owned()),
            Shape::Vector(kind) => (kind.name().to_owned(), "jagged".to_owned()),
        };
        listed.push((&branch.path, kind, shape));
    }
    listed.sort_by_cached_key(|(name, _, _)| name.to_lowercase());
    let width = listed.iter().map(|(name, _, _)| name.len()).max().unwrap_or(0);
    let kinds = listed.iter().map(|(_, kind, _)| kind.len()).max().unwrap_or(0);
    println!();
    for (name, kind, shape) in &listed {
        println!("  {name:width$}  {kind:kinds$}  {shape}");
    }
    if !unreadable.is_empty() {
        println!("\nNot readable by rvr:");
        for (name, reason) in unreadable {
            println!("  {name}  ({reason})");
        }
    }
    Ok(())
}
