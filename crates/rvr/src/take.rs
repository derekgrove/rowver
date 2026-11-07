//! Reading one file of a Plan (docs/SPEC.md §4.2, §7.2): opened and checked against the lock, then
//! read a run of entries at a time: the cuts and totals over its chosen entries, the stored
//! columns over its kept events, which go to the spool. A file that cannot be read is a hole; a
//! value the recipe cannot take in a chosen or kept event stops the press (a data error).

use crate::gather::{add_to, derived_tried, gather, held_lengths, kept, put_back, spooled, stored, summed, Gathered};
use crate::held::{spans, Held};
use crate::holes::{fault_of, Failed, Fault};
use crate::open::{entries as located, hint, mentioned, open, Branches, Reach};
use crate::plan::{Input, Output, Shape};
use crate::samples::remote::STOPPED;
use crate::samples::resolve;
use hep_root::{RootFile, Tree};
use rowver::study::lock::Branch;
use rowver::write;
use rvr_recipe::{Columns, Constant, Picked, Recipe, Scope};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// How many entries of a file are read and held at a time.
pub(crate) const CHUNK: u64 = 50_000;

/// `CHUNK`; in a debug build, `RVR_RUN` when set (and said, once), so the tests can press in runs of any size.
fn chunk() -> u64 {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var("RVR_RUN").ok().and_then(|n| n.parse().ok()).filter(|n| *n > 0) {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| eprintln!("rvr (a debug build): RVR_RUN: runs of {n} entries"));
        return n;
    }
    CHUNK
}

/// How many bytes of kept values a file gathers before they go to the spool on disk.
const SPOOLED: usize = 16 << 20;

/// In a local file, chosen entries further apart than this are read as runs of their own, so a
/// sparse random choice reads the baskets near its entries rather than every basket. A remote
/// file is cut only at a gap of a whole `CHUNK`: one more request costs about what 10 MB of
/// baskets do (measured on CMS Open Data, 2026-10-18).
const GAP: u64 = 2_000;

/// What every file of one press shares.
pub(crate) struct Shared<'a> {
    /// The recipe evaluated, and the stored columns read, each with its kind.
    pub recipe: &'a Recipe,
    /// The stored columns read, each with its kind.
    pub outputs: Vec<Output<'a>>,
    /// Each derived column's kind, for the trial.
    pub derived: &'a [Shape],
    /// Where its files are read from.
    pub reach: Reach<'a>,
    /// Every name the recipe mentions, to choose the branches fetched ahead.
    pub mentions: &'a BTreeSet<String>,
    /// By sample, the branches of its list its files must have as they are there.
    pub checked: &'a [HashMap<String, Branch>],
    /// Where each file's kept values go as they are gathered, by output and file.
    pub spool: &'a write::Spool,
    /// Where each file finished is noted; not written for a press that is not carried on.
    pub journal: &'a crate::journal::Journal,
    /// Whether the journal is kept.
    pub carries: bool,
    /// What the bars show.
    pub counts: &'a crate::progress::Counts,
    /// The samples whose derived columns were tried over a run of theirs.
    pub tried: Mutex<BTreeSet<usize>>,
    /// For the tests of picking up (a debug build): stop as a crash would once this many files are noted.
    #[cfg(debug_assertions)]
    pub stop_after: Option<usize>,
}

/// What one file gave; its values are in the spool.
pub(crate) struct Taken {
    /// How many events it kept.
    pub(crate) rows: usize,
    /// Its record in `made.files`, its holes included.
    pub(crate) record: Value,
    /// Entries read, before any cut.
    pub(crate) entries: u64,
    /// The entries chosen that lie in its holes.
    pub(crate) missing: u64,
    /// The events still kept after each cut.
    pub(crate) cutflow: Vec<u64>,
}

/// The runs of entries a press reads of entries `[from, to)` of a file, as [first, last): every
/// `chunk` entries, or around the entries `chosen` (those in the range), cut where two are `gap`
/// or more apart, each at most `chunk`.
fn runs(from: u64, to: u64, chosen: Option<&[u64]>, gap: u64, chunk: u64) -> Vec<(u64, u64)> {
    let Some(chosen) = chosen else {
        let starts = (from..to).step_by(chunk as usize);
        return starts.map(|first| (first, to.min(first + chunk))).collect();
    };
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for &e in chosen {
        match runs.last_mut() {
            Some((first, last)) if e - *last < gap && e + 1 - *first <= chunk => *last = e + 1,
            _ => runs.push((e, e + 1)),
        }
    }
    runs
}

/// Whether reading `input` (to `entries`) reads its other trees for their totals: a read of the
/// whole file, or of a file whose record has none yet; never building on an earlier file.
pub(crate) fn whole(input: &Input, entries: u64) -> bool {
    input.kept.is_none() && (input.unsummed || input.ranges.as_ref().is_none_or(|r| matches!(r[..], [(0, to, _)] if to >= entries)))
}

/// How many entries of `[first, last)` an input's choice chose: all, or a random choice's.
pub(crate) fn chosen_in(input: &Input, first: u64, last: u64) -> u64 {
    match &input.chosen {
        Some(chosen) => (chosen.partition_point(|e| *e < last) - chosen.partition_point(|e| *e < first)) as u64,
        None => last.saturating_sub(first),
    }
}

/// How many entries an input's choice chose in `holes`, each `[first, last)`.
pub(crate) fn missed(input: &Input, holes: &[(u64, u64)]) -> u64 {
    holes.iter().map(|(first, last)| chosen_in(input, *first, *last)).sum()
}

/// The entries the choice reads of an input's file, by the lock: below `stop`, or all.
fn limit(input: &Input) -> u64 {
    input.file.stop.unwrap_or(input.file.entries)
}

/// How many entries an input reads: of what its choice reads, or for a fill of the holes asked.
pub(crate) fn chosen_of(input: &Input) -> u64 {
    match &input.ranges {
        Some(ranges) => ranges.iter().map(|(first, last, _)| chosen_in(input, *first, *last)).sum(),
        None => chosen_in(input, 0, limit(input)),
    }
}

/// An input's record as the lock gives it: its tree, UUID and entries, and what the choice reads of it.
fn planned(input: &Input) -> Value {
    let mut record = json!({"tree": input.tree, "uuid": input.file.uuid, "entries": input.file.entries});
    if let Some(stop) = input.file.stop {
        record["stop"] = stop.into();
    }
    if let Some(chosen) = &input.chosen {
        record["take"] = chosen.len().into();
    }
    record
}

/// A failure as a hole, or what ends the press: one that is no kind of hole, or any building on an
/// earlier file (whose own holes are its only ones).
fn hole_or_end(mut failed: Failed, input: &Input) -> Result<Failed, Failed> {
    if failed.fault.kind().is_some() && input.kept.is_some() {
        failed.why += " Building on the earlier file, a file that cannot be read stops the press (rvr press --full starts over, and makes holes of it).";
        return Err(failed);
    }
    match failed.fault.kind() {
        Some(_) => Ok(failed),
        None => Err(failed),
    }
}

/// A hole as a file records it.
fn hole((first, last, failed): &(u64, u64, Failed)) -> Value {
    json!({"entries": [first, last], "kind": failed.fault.kind(), "why": failed.why})
}


/// A hole's entries, as `[first, last)`.
pub(crate) fn range(hole: &Value) -> (u64, u64) {
    let entry = |at: usize| hole["entries"][at].as_u64().unwrap_or(0);
    (entry(0), entry(1))
}

/// The holes a file's record holds, as `[first, last)`.
pub(crate) fn ranges(record: &Value) -> Vec<(u64, u64)> {
    record["holes"].as_array().into_iter().flatten().map(range).collect()
}

/// `text` ending as a sentence does.
pub(crate) fn sentence(text: &str) -> String {
    match text.ends_with(['.', '?', '!']) {
        true => text.to_string(),
        false => format!("{text}."),
    }
}

/// What a file gave when none of it was read: its record, and the entries chosen in its holes.
fn nothing(recipe: &Recipe, record: Value, missing: u64) -> Taken {
    Taken { rows: 0, record, entries: 0, missing, cutflow: vec![0; recipe.keep.len()] }
}

/// A file that cannot be opened: one hole of every entry chosen of what it reads. One of no
/// entries, which holds the totals once read, has none to make a hole of, and ends the press.
fn unread(shared: &Shared, input: &Input, to_read: &[(u64, u64, usize)], failed: Failed) -> Result<Taken, Failed> {
    let (at, chosen) = (&input.file.at, chosen_of(input));
    let failed = hole_or_end(failed, input)?;
    if !shared.recipe.totals.is_empty() && rowver::rvr::read_file(chosen, chosen, input.chosen.is_some()) {
        let why = match failed.fault {
            Fault::Gone => format!("{at} no longer exists: put it back (if it moved, run rvr samples resolve samples.toml again)"),
            _ if failed.why.contains(at.as_str()) => failed.why,
            _ => format!("{at}: {}", failed.why),
        };
        return Err(Failed::new(Fault::Needed, format!("{} The press reads it for the recipe's [totals] (each file it reads holds them, as a total over another of its trees holds that tree's sum), and as samples.lock gives it no entries, there is none to make a hole of: it cannot be left out.", sentence(&why))));
    }
    let ranges = to_read.iter().filter(|(from, to, _)| chosen_in(input, *from, *to) > 0);
    let holes: Vec<Value> = ranges.map(|&(from, to, _)| hole(&(from, to, failed.clone()))).collect();
    let mut record = planned(input);
    if !holes.is_empty() {
        record["holes"] = holes.into();
    }
    Ok(nothing(shared.recipe, record, chosen))
}

/// An input opened, its tree, and the entries its choice reads of it: checked against the lock
/// (its entries, and the branches the recipe reads as its sample's list gives them).
fn opened(shared: &Shared, input: &Input) -> Result<(RootFile<Held>, Tree, u64), Failed> {
    let at = &input.file.at;
    let then = if input.kept.is_some() { "press again" } else { "run rvr press --fill" };
    let file = open(&shared.reach, &input.file, &input.read_at, (then, "rvr press"))?;
    let tree = file.tree_at(&input.tree).map_err(|e| match e {
        hep_root::Error::KeyNotFound(_) => Failed::new(Fault::Changed, format!("{at}: it has no tree {:?}, which samples.lock recorded. Run rvr samples resolve samples.toml and review the new lock.", input.tree)),
        e => Failed::new(fault_of(&e, file.source().inner().seen()), format!("{at}: its tree {:?}: {e}", input.tree)),
    })?;
    let total = tree.entries.max(0) as u64;
    if input.file.entries != total {
        return Err(Failed::new(Fault::Changed, format!("{at}: its tree {} has {total} entries, not the {} samples.lock recorded. Run rvr samples resolve samples.toml and review the new lock.", input.tree, input.file.entries)));
    }
    // Every branch the recipe reads has here the kind and shape it has in the sample's list; the
    // others may differ (a run's trigger menu adds HLT_ branches).
    let want = &shared.checked[input.sample];
    let have = match want.is_empty() {
        true => Vec::new(),
        false => resolve::branches_named(&tree, |name| want.contains_key(name)),
    };
    let differ = resolve::differ(want.values(), have.iter());
    if !differ.is_empty() {
        return Err(Failed::new(Fault::Changed, format!("{at}: of the branches the recipe reads, its tree is not as its sample's first file is ({differ}). Name the files that differ in a sample of their own, run rvr samples resolve samples.toml, then rvr press.")));
    }
    let entries = input.file.stop.map_or(total, |stop| stop.min(total));
    Ok((file, tree, entries))
}

/// Each total over another tree of the file, summed over every entry of that tree. A file without
/// that tree, or without a branch it reads, or with one of a type rvr cannot read, stops the
/// press (a data error); a failure reading its bytes is the file's, a hole.
fn other_totals(shared: &Shared, input: &Input, file: &RootFile<Held>, sample: &BTreeMap<String, Constant>) -> Result<Vec<(String, f64)>, Failed> {
    let at = &input.file.at;
    let seen = || file.source().inner().seen();
    let mut sums = Vec::new();
    for total in &shared.recipe.totals {
        let Some(name) = &total.tree else { continue };
        let place = format!("{at}: [totals] {}", total.name);
        let other = file.tree_at(name).map_err(|e| match e {
            hep_root::Error::KeyNotFound(_) => Failed::new(Fault::Data, format!("{place} sums the tree {name:?}, which this file does not have: total a tree every file of its samples has, or name the files that have it in a sample of their own")),
            e => Failed::new(fault_of(&e, seen()), format!("{place}: its tree {name:?}: {e}")),
        })?;
        let n = other.entries.max(0) as usize;
        let (defines, branches) = (BTreeMap::new(), Branches::over(file, &other, 0, n as i64));
        let mut sum = 0.0;
        let mut work = |run: &dyn Columns, from: usize, to: usize| {
            let array = Scope::new(run, sample, &defines).evaluate(&total.value).and_then(|v| rvr_recipe::per_event(v, to - from));
            add_to(&mut sum, &total.name, array)
        };
        if let Err((why, part)) = rvr_recipe::over_runs(&branches, n, &mut work) {
            let failure = branches.failure();
            let missing = why.missing().map(str::to_owned);
            return Err(match (failure, missing, why.1) {
                (Some(hep_root::Error::Unsupported(e)), ..) => Failed::new(Fault::Data, format!("{place}: a branch of its tree {name:?} is of a type rvr cannot read here ({e}): total a branch rvr reads in every file")),
                (Some(e), ..) => Failed::new(fault_of(&e, seen()), format!("{at}: tree {name:?}, {}: {}", located(&why, 0, &part), why.0)),
                (None, Some(branch), _) => Failed::new(Fault::Data, format!("{place}: its tree {name:?} has no branch {branch}: total a branch every file has, or name the files that have it in a sample of their own")),
                (None, None, Some(_)) => Failed::new(Fault::Data, format!("{at}: tree {name:?}, {}: {}", located(&why, 0, &part), why.0)),
                // That tree's branches are not in samples.lock, so its total is first typed here.
                (None, None, None) => Failed::new(Fault::Data, format!("{at}: tree {name:?}: {}. A total over another tree is worked out as each file is read: mend it to one number per entry of that tree", why.0.trim_end_matches('.'))),
            });
        }
        sums.push((total.name.clone(), sum));
    }
    Ok(sums)
}

/// Each stored column read, worked out over the events of `scope` and gathered.
fn columns(shared: &Shared, scope: &Scope, events: usize, gathered: &mut [Gathered]) -> rvr_recipe::Result<()> {
    for ((column, _), gathered) in shared.outputs.iter().zip(gathered) {
        let array = stored(shared.recipe, scope, column, events)?;
        gather(gathered, array).map_err(rvr_recipe::problem)?;
    }
    Ok(())
}

/// An error over picked rows as one over the run's rows: its event, and the rows of its part.
pub(crate) fn unpicked(positions: &[usize]) -> impl Fn((rvr_recipe::Error, Range<usize>)) -> (rvr_recipe::Error, Range<usize>) + '_ {
    move |(error, part)| {
        let event = error.1.map(|row| positions[row]);
        let error = error.of_event(event);
        let part = match part.is_empty() {
            true => 0..0,
            false => positions[part.start]..positions[part.end - 1] + 1,
        };
        (error, part)
    }
}

/// One run's chosen entries (`chosen`, its rows) through the totals and the cuts, and its kept
/// events' stored columns into `gathered`: the rows it kept. A run is kept whole or not at all.
fn evaluated(shared: &Shared, sample: &BTreeMap<String, Constant>, branches: &dyn Columns, rows: usize, chosen: &[usize], gathered: &mut [Gathered], read: &mut Read) -> Result<Vec<usize>, (rvr_recipe::Error, Range<usize>)> {
    let before = held_lengths(gathered);
    let done = worked_out(shared, sample, branches, rows, chosen, gathered, read);
    if done.is_err() {
        put_back(gathered, &before);
    }
    done
}

/// `evaluated`, before what a run that failed gathered is put back.
fn worked_out(shared: &Shared, sample: &BTreeMap<String, Constant>, branches: &dyn Columns, rows: usize, chosen: &[usize], gathered: &mut [Gathered], read: &mut Read) -> Result<Vec<usize>, (rvr_recipe::Error, Range<usize>)> {
    let recipe = shared.recipe;
    let (mut sums, mut counted) = (read.totals.clone(), vec![0_u64; recipe.keep.len()]);
    let picked = Picked::new(branches, chosen, rows);
    // With no cuts every chosen entry is kept: totals and columns are worked out in one pass.
    let kept_rows = if recipe.keep.is_empty() {
        let mut work = |run: &dyn Columns, from: usize, to: usize| {
            let scope = Scope::new(run, sample, &recipe.defines);
            let (part, mut more) = (held_lengths(gathered), sums.clone());
            let done = summed(recipe, &scope, to - from, &mut more).and_then(|()| columns(shared, &scope, to - from, gathered));
            match done {
                Ok(()) => sums = more,
                Err(_) => put_back(gathered, &part),
            }
            done
        };
        rvr_recipe::over_runs(&picked, chosen.len(), &mut work).map_err(unpicked(chosen))?;
        chosen.to_vec()
    } else {
        // The defined names the cuts worked out over every chosen entry at once, for the columns.
        let mut known = Vec::new();
        let mut work = |run: &dyn Columns, from: usize, to: usize| {
            let scope = Scope::new(run, sample, &recipe.defines);
            let (mut more, mut cut) = (sums.clone(), vec![0_u64; recipe.keep.len()]);
            let keep = summed(recipe, &scope, to - from, &mut more).and_then(|()| kept(recipe, &scope, vec![true; to - from], &mut cut))?;
            sums = more;
            counted.iter_mut().zip(cut).for_each(|(all, here)| *all += here);
            if (from, to) == (0, chosen.len()) {
                known = scope.worked_out();
            }
            Ok(keep)
        };
        let keep = rvr_recipe::over_runs(&picked, chosen.len(), &mut work).map_err(unpicked(chosen))?.concat();
        // The kept events, by place among the chosen ones: each branch, and each defined name the
        // cuts worked out, is picked down to them from what the cuts had, not worked out again.
        let kept_at: Vec<usize> = keep.iter().enumerate().filter(|(_, keep)| **keep).map(|(at, _)| at).collect();
        let kept_rows: Vec<usize> = kept_at.iter().map(|at| chosen[*at]).collect();
        let known: Vec<(String, rvr_recipe::Array)> = known.into_iter().map(|(name, array)| (name, array.picked(&kept_at))).collect();
        let view = Picked::new(&picked, &kept_at, chosen.len());
        let mut work = |run: &dyn Columns, from: usize, to: usize| {
            let part = held_lengths(gathered);
            let whole = (from, to) == (0, kept_at.len());
            let seeded = known.iter().map(|(name, array)| (name.clone(), if whole { array.clone() } else { array.rows(from, to) }));
            let done = columns(shared, &Scope::new(run, sample, &recipe.defines).knowing(seeded), to - from, gathered);
            if done.is_err() {
                put_back(gathered, &part);
            }
            done
        };
        if !kept_rows.is_empty() {
            rvr_recipe::over_runs(&view, kept_rows.len(), &mut work).map_err(unpicked(&kept_rows))?;
        }
        kept_rows
    };
    read.totals = sums;
    read.cutflow.iter_mut().zip(counted).for_each(|(all, here)| *all += here);
    Ok(kept_rows)
}

/// What a file's runs gave: its rows, its holes, its cutflow and its totals over the events read.
struct Read {
    rows: usize,
    holes: Vec<(u64, u64, Failed)>,
    cutflow: Vec<u64>,
    totals: Vec<f64>,
}

/// Read a file's ranges a run at a time, keeping the events the cuts pass and sending their values
/// to the spool, and counting each run's entries in `progressed` as the bars are told of it.
#[allow(clippy::too_many_arguments)]
fn read_runs(shared: &Shared, input: &Input, file: &RootFile<Held>, tree: &Tree, sample: &BTreeMap<String, Constant>, entries: u64, to_read: &[(u64, u64, usize)], progressed: &mut u64) -> Result<Read, Failed> {
    let (recipe, at) = (shared.recipe, &input.file.at);
    let names: Vec<String> = tree.walk().into_iter().map(|(_, b)| b.name.clone()).collect();
    let remote = at.contains("://");
    let wanted: Vec<&hep_root::Branch> = match remote {
        true => tree.walk().into_iter().map(|(_, b)| b).filter(|b| hep_root::columns::Kind::of(b).is_ok() && mentioned(shared.mentions, sample, &b.name)).collect(),
        false => Vec::new(),
    };
    let mut gathered: Vec<Gathered> = shared.outputs.iter().map(|((name, _), kind)| Gathered::new(name, *kind)).collect();
    let mut read = Read { rows: 0, holes: Vec::new(), cutflow: vec![0; recipe.keep.len()], totals: vec![0.0; recipe.totals.iter().filter(|t| t.tree.is_none()).count()] };
    let mut entry: Vec<i64> = Vec::new();
    let mut tried = recipe.derived.is_empty();
    let gap = if remote { chunk() } else { GAP };
    // Building on an earlier file, the events it kept and no others, without the cuts.
    let picks = input.kept.as_deref().or(input.chosen.as_deref());
    'ranges: for (at_range, &(from, to, order)) in to_read.iter().enumerate() {
        let (to, mut piece, mut ended) = (to.min(entries), 0, false);
        let here = picks.map(|c| &c[c.partition_point(|e| *e < from)..c.partition_point(|e| *e < to)]);
        for (first, last) in runs(from, to, here, gap, chunk()) {
            if shared.reach.watch.stop.load(Ordering::Acquire) {
                return Err(Failed::new(Fault::Stopped, STOPPED));
            }
            let count = (last - first) as usize;
            let chosen: Vec<usize> = match picks {
                Some(picks) => picks[picks.partition_point(|e| *e < first)..picks.partition_point(|e| *e < last)].iter().map(|e| (e - first) as usize).collect(),
                None => (0..count).collect(),
            };
            let in_run = chosen_in(input, first, last);
            if chosen.is_empty() {
                shared.counts.ran(input.sample, in_run, 0);
                *progressed += in_run;
                continue;
            }
            if remote {
                // Fetching ahead only saves round trips; a branch that cannot be fetched is said
                // when an expression asks for it. What failed before the run's reads says nothing.
                let _ = file.source().inner().hold(spans(&wanted, first as i64, last as i64));
                if let Some(seen) = file.source().inner().seen() {
                    seen.clear();
                }
            }
            let before = held_lengths(&gathered);
            let branches = Branches::over(file, tree, first as i64, last as i64);
            let kept_rows = match evaluated(shared, sample, &branches, count, &chosen, &mut gathered, &mut read) {
                Ok(kept_rows) => kept_rows,
                Err((error, part)) => {
                    // A branch that could not be read is the file's; a value one cannot take is the data's.
                    let fault = match branches.failure() {
                        Some(e) => fault_of(&e, file.source().inner().seen()),
                        None if error.1.is_some() => Fault::Data,
                        None => Fault::Bug,
                    };
                    let mut failed = Failed::new(fault, format!("{at}, tree {}, {}: {}{}", input.tree, located(&error, first, &part), error.0, hint(&error, &names, recipe)));
                    // A local file that cannot be opened now at all (a mount gone) is not the file's
                    // fault; one that can, but not read there, is damaged there.
                    if !remote && matches!(fault, Fault::Damaged | Fault::Save) {
                        failed.fault = match std::fs::File::open(resolve::local(at, shared.reach.dir)) {
                            Ok(_) => Fault::Damaged,
                            Err(_) => Fault::Save,
                        };
                    }
                    let failed = hole_or_end(failed, input)?;
                    // Only a damaged basket leaves the rest of the file worth reading: otherwise the
                    // rest of this range, and every range after it, is a hole.
                    if failed.fault.local() {
                        read.holes.push((first, last, failed));
                        continue;
                    }
                    let rest = to_read[at_range + 1..].iter().map(|&(from, to, _)| (from, to.min(entries), failed.clone()));
                    let rest: Vec<_> = rest.filter(|(from, to, _)| chosen_in(input, *from, *to) > 0).collect();
                    read.holes.push((first, to, failed));
                    read.holes.extend(rest);
                    ended = true;
                    break;
                }
            };
            // The derived columns are worked out over the events one run of each sample kept, so
            // that a value one cannot take stops the press now, not once every file is read.
            if !tried && !kept_rows.is_empty() {
                tried = true;
                if shared.tried.lock().unwrap_or_else(|e| e.into_inner()).insert(input.sample) {
                    let kept_entries: Vec<u64> = kept_rows.iter().map(|row| first + *row as u64).collect();
                    derived_tried(recipe, shared.derived, &gathered, &before, sample, &kept_entries).map_err(|mut failed| {
                        failed.why = format!("{at}, tree {}, {}", input.tree, failed.why);
                        failed
                    })?;
                }
            }
            let kept_now = kept_rows.len();
            entry.extend(kept_rows.iter().map(|row| (first + *row as u64) as i64));
            shared.counts.ran(input.sample, in_run, kept_now as u64);
            *progressed += in_run;
            let held: usize = gathered.iter().map(|g| g.bytes.len() + g.counts.as_ref().map_or(0, |c| c.len() * 8)).sum();
            if held + entry.len() * 8 >= SPOOLED {
                spool(shared, input, &mut gathered, &mut entry, &mut read.rows, (order, piece))?;
                piece += 1;
            }
        }
        if !entry.is_empty() {
            spool(shared, input, &mut gathered, &mut entry, &mut read.rows, (order, piece))?;
        }
        if ended {
            break 'ranges;
        }
    }
    if input.kept.as_ref().is_some_and(|kept| kept.len() != read.rows) {
        return Err(Failed::new(Fault::Bug, format!("{at}: {} events were read again of the {} the earlier file kept", read.rows, input.kept.as_ref().map_or(0, Vec::len))));
    }
    Ok(read)
}

/// Send what a file has gathered to the spool, after checking, building on an earlier file, that its entries are the ones it kept.
fn spool(shared: &Shared, input: &Input, gathered: &mut [Gathered], entry: &mut Vec<i64>, rows: &mut usize, order: write::Order) -> Result<(), Failed> {
    let at = &input.file.at;
    if let Some(kept) = &input.kept {
        let same = kept.get(*rows..*rows + entry.len()).is_some_and(|kept| kept.iter().zip(entry.iter()).all(|(k, e)| *k as i64 == *e));
        if !same {
            return Err(Failed::new(Fault::Bug, format!("{at}: the events read again are not the ones the earlier file kept")));
        }
    }
    *rows += entry.len();
    spooled(shared.spool, gathered, entry, order).map_err(|e| Failed::new(Fault::Save, format!("{at}: {e}")))
}

/// Read one file a run of entries at a time, keeping the events the cuts pass. A file that cannot
/// be opened, and a run of it that cannot be read, are holes; the error is what ends the press (a
/// data error, a failure that is not the file's own, or another file's).
pub(crate) fn take(shared: &Shared, sample: &BTreeMap<String, Constant>, input: &Input, progressed: &mut u64) -> Result<Taken, Failed> {
    let recipe = shared.recipe;
    // A file a random choice takes nothing from is recorded from the lock, not opened.
    if input.chosen.as_ref().is_some_and(Vec::is_empty) {
        return Ok(nothing(recipe, planned(input), 0));
    }
    // Building on an earlier file, the holes that file has in it are this file's too.
    let limit = limit(input);
    let carried = input.before.as_ref().map(ranges).unwrap_or_default();
    // A file the earlier one could not open, chose nothing of, or kept no event of, is not opened again.
    let none_kept = input.kept.as_ref().is_some_and(Vec::is_empty);
    let carried_missed = missed(input, &carried);
    if let (Some(before), true) = (&input.before, none_kept || carried_missed == chosen_in(input, 0, limit)) {
        return Ok(Taken { entries: chosen_in(input, 0, limit) - carried_missed, ..nothing(recipe, before.clone(), carried_missed) });
    }
    let to_read = input.ranges.clone().unwrap_or_else(|| vec![(0, limit, input.order)]);
    let (file, tree, entries) = match opened(shared, input) {
        Ok(opened) => opened,
        Err(failed) => return unread(shared, input, &to_read, failed),
    };
    let mut sample = sample.clone();
    sample.insert("file".into(), Constant::Text(input.file.at.clone()));
    // Totals over another tree, read before its events (so that one that fails makes the whole
    // file a hole), when it is read whole or for the first time.
    let others = match whole(input, entries) {
        true => match other_totals(shared, input, &file, &sample) {
            Ok(others) => others,
            Err(failed) => return unread(shared, input, &to_read, failed),
        },
        false => Vec::new(),
    };
    let read = read_runs(shared, input, &file, &tree, &sample, entries, &to_read, progressed)?;
    let mut record = planned(input);
    let chosen = chosen_of(input);
    // Building on an earlier file, its holes are this file's: what it kept was read around them.
    let missing = match input.before.as_ref().filter(|_| !carried.is_empty()) {
        Some(before) => {
            record["holes"] = before["holes"].clone();
            carried_missed
        }
        None => {
            if !read.holes.is_empty() {
                record["holes"] = read.holes.iter().map(hole).collect();
            }
            read.holes.iter().map(|(first, last, _)| chosen_in(input, *first, *last)).sum()
        }
    };
    // Its totals, when it was read: over its events read, and over every entry of another tree of
    // it; building on an earlier file, which reads only the events it kept, the earlier file's.
    let earlier = input.before.as_ref().filter(|_| input.kept.is_some()).map(|before| before["totals"].clone());
    if let Some(totals) = earlier.filter(|totals| !totals.is_null()) {
        record["totals"] = totals;
    } else if !recipe.totals.is_empty() && input.kept.is_none() && rowver::rvr::read_file(chosen, missing, input.chosen.is_some()) {
        let mut sums = Map::new();
        let mut events = read.totals.iter();
        for total in &recipe.totals {
            let sum = match &total.tree {
                None => events.next().copied(),
                Some(_) => others.iter().find(|(name, _)| *name == total.name).map(|(_, sum)| *sum),
            };
            if let Some(sum) = sum {
                sums.insert(total.name.clone(), sum.into());
            }
        }
        record["totals"] = Value::Object(sums);
    }
    Ok(Taken { rows: read.rows, record, entries: chosen - missing, missing, cutflow: read.cutflow })
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC 4.2.1
    #[test]
    fn runs_are_every_chunk_or_the_chosen_entries_cut_at_gaps() {
        assert_eq!(runs(0, 120_000, None, GAP, CHUNK), [(0, 50_000), (50_000, 100_000), (100_000, 120_000)]);
        assert_eq!(runs(0, 0, None, GAP, CHUNK), []);
        assert_eq!(runs(70_000, 125_000, None, GAP, CHUNK), [(70_000, 120_000), (120_000, 125_000)]);
        let chosen = [3, 10, 5_000, 5_001, 60_000];
        assert_eq!(runs(0, 70_000, Some(&chosen), GAP, CHUNK), [(3, 11), (5_000, 5_002), (60_000, 60_001)]);
        assert_eq!(runs(0, 70_000, Some(&chosen), CHUNK, CHUNK), [(3, 5_002), (60_000, 60_001)]);
        let dense: Vec<u64> = (0..120_000).step_by(10).collect();
        assert_eq!(runs(0, 120_000, Some(&dense), GAP, CHUNK), [(0, 49_991), (50_000, 99_991), (100_000, 119_991)]);
    }
}
