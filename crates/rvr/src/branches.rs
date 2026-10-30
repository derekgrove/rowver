//! `rvr branches`: what a study's file holds, as a recipe can use it. The branches of the first
//! file of a sample, grouped by the prefix before their first `_`: the collections (a list per
//! event, such as `Jet`) and the groups of one value per event (such as `MET`). Read from the
//! file's metadata alone, so no events are read.

use crate::press::{self, count};
use crate::Words;
use hep_root::columns::{inventory, Shape};
use std::collections::BTreeMap;
use std::path::Path;

const USAGE: &str = "usage:
  rvr branches [DIR] [--sample NAME] [--find TEXT]
      the branches of the first file of the first sample (or NAME) of the study in DIR (.),
      grouped into the collections and values a recipe can name; --find shows only the
      branches whose name holds TEXT (in any case)";

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
    // Branches grouped as `prefix` says; each with its type and shape.
    let mut groups: BTreeMap<String, Vec<(&str, String, String)>> = BTreeMap::new();
    let mut counters: BTreeMap<String, &str> = BTreeMap::new();
    let mut unreadable = Vec::new();
    for branch in branches.iter().filter(|b| found(&b.path)) {
        let element = branch.type_name.split(['[', '<']).next().unwrap_or("");
        let (kind, shape) = match &branch.shape {
            Shape::Unreadable { reason } => {
                unreadable.push((branch.path.as_str(), branch.type_name.clone(), reason.clone()));
                continue;
            }
            Shape::Scalar => (element.to_owned(), "flat".to_owned()),
            Shape::Fixed(n) => (element.to_owned(), format!("{n} per event")),
            Shape::Jagged { .. } | Shape::JaggedFixed { .. } => (element.to_owned(), "jagged".to_owned()),
            Shape::Vector(kind) => (kind.name().to_owned(), "jagged".to_owned()),
        };
        let prefix = prefix(&branch.path);
        if let Shape::Jagged { counter } | Shape::JaggedFixed { counter, .. } = &branch.shape {
            counters.entry(prefix.to_owned()).or_insert(counter);
        }
        groups.entry(prefix.to_owned()).or_default().push((&branch.path, kind, shape));
    }
    // A collection's counter (nJet) is listed first in it; a value alone, or with no prefix, last.
    let mut other = groups.remove("").unwrap_or_default();
    for (prefix, counter) in &counters {
        let Some(group) = groups.get_mut(prefix) else { continue };
        if let Some(at) = other.iter().position(|(name, _, _)| name == counter) {
            group.insert(0, other.remove(at));
        }
    }
    let lone: Vec<String> = groups.iter().filter(|(_, b)| b.len() == 1).map(|(p, _)| p.clone()).collect();
    for prefix in lone {
        other.extend(groups.remove(&prefix).unwrap_or_default());
    }
    let jagged = |branches: &Vec<(&str, String, String)>| branches.iter().any(|(_, _, shape)| shape == "jagged");
    let (lists, values): (Vec<_>, Vec<_>) = groups.iter().partition(|(_, branches)| jagged(branches));
    for (prefix, branches) in lists.into_iter().chain(values) {
        println!("\n{prefix}:");
        print_group(branches, counters.get(prefix).copied());
    }
    if !other.is_empty() {
        other.sort_by_key(|(name, _, _)| name.to_lowercase());
        println!("\nOther:");
        print_group(&other, None);
    }
    if !unreadable.is_empty() {
        println!("\nNot readable by rvr:");
        for (name, _, reason) in unreadable {
            println!("  {name}  ({reason})");
        }
    }
    Ok(())
}

/// The group a branch belongs to: for a member of a split object (`recoMuons_muons__RECO.obj.pt_`,
/// `AnalysisJetsAuxDyn.pt`), the branch before its first `.`; otherwise the prefix before its first
/// `_` (`Jet` for `Jet_pt`); nothing when it has neither.
fn prefix(name: &str) -> &str {
    let split = match name.contains('.') {
        true => name.split_once('.'),
        false => name.split_once('_'),
    };
    match split {
        Some((prefix, rest)) if !prefix.is_empty() && !rest.is_empty() => prefix,
        _ => "",
    }
}

/// One line per branch, its counter first: name, type and shape, in columns.
fn print_group(branches: &[(&str, String, String)], counter: Option<&str>) {
    let mut sorted: Vec<&(&str, String, String)> = branches.iter().collect();
    sorted.sort_by_key(|(name, _, _)| (Some(*name) != counter, name.to_lowercase()));
    let width = sorted.iter().map(|(name, _, _)| name.len()).max().unwrap_or(0);
    let kinds = sorted.iter().map(|(_, kind, _)| kind.len()).max().unwrap_or(0);
    for (name, kind, shape) in sorted {
        println!("  {name:width$}  {kind:kinds$}  {shape}");
    }
}

#[cfg(test)]
mod tests {
    use super::prefix;

    #[test]
    fn a_branch_is_grouped_by_its_object_or_its_prefix() {
        assert_eq!(prefix("Jet_pt"), "Jet");
        assert_eq!(prefix("HLT_IsoMu24_eta2p1"), "HLT");
        assert_eq!(prefix("recoMuons_muons__RECO.obj.m_state.p4Polar_.fCoordinates.fPt"), "recoMuons_muons__RECO");
        assert_eq!(prefix("AnalysisJetsAuxDyn.pt"), "AnalysisJetsAuxDyn");
        assert_eq!(prefix("run"), "");
        assert_eq!(prefix("_odd"), "");
    }
}
