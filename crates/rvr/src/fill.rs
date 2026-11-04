//! `rvr press --fill` (SPEC §8.7): read only a file's holes, and write it again with their rows
//! merged by entry among its own; a press of samples added builds on a file the same way.

use crate::plan::Input;
use crate::take::{missed, ranges, Taken};
use rowver::study::lock::Lock;
use rowver::write;
use rowver::write::NumericBuffer;
use rowver::choice::Events;
use rowver::rvr::{Reader, HOLE_KINDS};
use serde_json::Value;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// What a fill reads of one file, and what it keeps.
pub struct Plan {
    /// The file's rows in the old file.
    pub rows: Range<usize>,
    /// The holes read again, as `[first, last)`.
    pub read: Vec<(u64, u64)>,
    /// The holes left as they are (of kinds not asked for), as the file records them.
    pub kept: Vec<Value>,
    /// The file's record in the old file.
    pub record: Value,
    /// Whether the old file read none of it (every entry it chose is in a hole).
    pub unread: bool,
}

/// A file with holes and what filling them reads; or a file of other samples and what the samples added read.
pub struct Fill {
    /// The file being filled.
    pub file: Reader,
    /// By sample and `at`, every file the old file read, and every file of a sample added.
    pub files: HashMap<(String, String), Plan>,
    /// Each row's entry in its file.
    pub entries: Vec<i64>,
    /// For a press of other samples than the file's, not a fill: the samples it reads whole.
    pub added: Option<Vec<String>>,
}

/// The kinds `--fill` was given (`refused,unreachable`, every part a kind), or every kind.
pub fn kinds(given: Option<&str>) -> Vec<String> {
    match given {
        Some(given) => given.split(',').map(str::to_owned).collect(),
        None => HOLE_KINDS.iter().map(|k| k.to_string()).collect(),
    }
}

/// Whether `--fill` would fill a file with this header, or why not; the files it read none of, when worked out.
pub fn fits(
    header: &Value,
    shown: &str,
    texts: &crate::texts::Texts,
    lock: &Lock,
    asked: &[Events],
    kinds: &[String],
) -> Result<Option<HashSet<(String, String)>>, String> {
    // The files it read none of, worked out only when a file of the lock is another ROOT file now.
    let unread = OnceCell::new();
    let made = &header["made"];
    let text = |key: &str| made[key]["text"].as_str().unwrap_or_default();
    let holes = rowver::rvr::holes(header);
    if holes.is_empty() {
        return Err(format!("{shown} has no holes: there is nothing to fill"));
    }
    let not = |why: &str| Err(format!("{shown} cannot be filled, as {why}: rvr press presses it again"));
    if let Err(why) = crate::reuse::pressed_alike(header, texts, lock) {
        return not(&why);
    }
    if text("recipe") != texts.recipe {
        return not("recipe.toml has changed since");
    }
    let wanted: Vec<&rowver::rvr::Hole> = holes.iter().filter(|h| kinds.contains(&h.kind)).collect();
    if wanted.is_empty() {
        return Err(format!("{shown} has no holes of kind {}: its holes are {}", kinds.join(", "), holes.iter().map(|h| h.kind.as_str()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", ")));
    }
    let old = Lock::read(text("lock")).map_err(|p| format!("{shown} cannot be filled, as the samples.lock it carries does not check out ({p}): rvr press --full presses it again"))?;
    // Only a file whose holes are read may have been resolved again; one the old file holds rows
    // of must be the same ROOT file.
    let read: Vec<(&str, &str)> = wanted.iter().map(|h| (h.sample.as_str(), h.file.as_str())).collect();
    // A file with a hole this fill does not read keeps that hole as the old file found it.
    let kept: Vec<(&str, &str)> = holes.iter().filter(|h| !kinds.contains(&h.kind)).map(|h| (h.sample.as_str(), h.file.as_str())).collect();
    if old.samples.len() != lock.samples.len() {
        return not("samples.lock has other samples now");
    }
    for (was, now) in old.samples.iter().zip(&lock.samples) {
        if was.name != now.name || was.tree != now.tree || was.files.len() != now.files.len() {
            return not(&format!("samples.lock has other files of {} now", now.name));
        }
        // The rows kept were checked against the sample's branch list as it was.
        if was.branches != now.branches {
            return not(&format!("{}'s branch list has changed in samples.lock", now.name));
        }
        for (a, b) in was.files.iter().zip(&now.files) {
            let at = (now.name.as_str(), b.at.as_str());
            if a.at != b.at {
                return not(&format!("{} is now {} in samples.lock", a.at, b.at));
            }
            if a.entries != b.entries {
                return not(&format!("{} has other entries now, which changes the events chosen", b.at));
            }
            if unplanned(a) == unplanned(b) {
                continue;
            }
            if !read.contains(&at) {
                return not(&format!("{} has changed in samples.lock, and no hole of it is read", b.at));
            }
            // Another ROOT file in its place is the same file only if the old file read none of it.
            if a.uuid != b.uuid && (!unread_once(&unread, header, shown)?.contains(&(now.name.clone(), b.at.clone())) || kept.contains(&at)) {
                return not(&format!("{} is another ROOT file now (its UUID), of which the file holds rows or holes", b.at));
            }
        }
    }
    for (sample, want) in lock.samples.iter().zip(asked) {
        if made["files"][&sample.name]["events"] != want.record() {
            return Err(format!("{shown} chose other events of {} than this press would: fill it with the --events, --random and --seed it was pressed with", sample.name));
        }
    }
    Ok(unread.into_inner())
}

/// `unread` of a header, worked out the first time it is asked for.
fn unread_once<'a>(cell: &'a OnceCell<HashSet<(String, String)>>, header: &Value, shown: &str) -> Result<&'a HashSet<(String, String)>, String> {
    if let Some(set) = cell.get() {
        return Ok(set);
    }
    let set = unread(header, shown)?;
    Ok(cell.get_or_init(|| set))
}

/// By sample and path, the files with holes that a file read none of: every entry it chose of
/// them lies in a hole.
fn unread(header: &Value, shown: &str) -> Result<HashSet<(String, String)>, String> {
    let holes = rowver::rvr::holes(header);
    let chosen = rowver::rvr::chosen_in_holes(header).map_err(|e| format!("{shown}: {e}"))?;
    let mut missing: HashMap<(&str, &str), u64> = HashMap::new();
    for (hole, n) in holes.iter().zip(&chosen) {
        *missing.entry((hole.sample.as_str(), hole.file.as_str())).or_default() += n;
    }
    let files = &header["made"]["files"];
    let none = |(sample, at): &(&str, &str)| {
        let record = &files[*sample]["files"][*at];
        let chosen = record["take"].as_u64().or(record["stop"].as_u64()).or(record["entries"].as_u64());
        chosen.is_some_and(|n| missing.get(&(*sample, *at)) == Some(&n))
    };
    Ok(missing.keys().filter(|file| none(file)).map(|(sample, at)| ((*sample).to_owned(), (*at).to_owned())).collect())
}

/// A file of the lock as another lock gives the same file: but for what a press plans of it
/// (stop, take).
pub(crate) fn unplanned(file: &rowver::study::lock::File) -> rowver::study::lock::File {
    rowver::study::lock::File { stop: None, take: None, ..file.clone() }
}

/// What filling `file` reads, or why it cannot (see `fits`).
pub fn plan(
    file: Reader,
    texts: &crate::texts::Texts,
    lock: &Lock,
    asked: &[Events],
    kinds: &[String],
) -> Result<Fill, String> {
    let shown = file.path().display().to_string();
    let unread = match fits(file.header(), &shown, texts, lock, asked, kinds)? {
        Some(unread) => unread,
        None => unread(file.header(), &shown)?,
    };
    rows(file, kinds, &unread)
}

/// A file's rows by the file each came from, each reading again its holes of `kinds`; `unread`,
/// the files it read none of.
pub(crate) fn rows(file: Reader, kinds: &[String], unread: &HashSet<(String, String)>) -> Result<Fill, String> {
    let header = file.header();
    let made = &header["made"];
    let entries = file.entry().map_err(|e| rowver::plain(file.path(), &e))?;
    let short = || format!("{}: its files hold more rows than it has: a bug in rvr, as the file was checked; please report it", file.path().display());
    if entries.len() < header["data"]["rows"].as_u64().unwrap_or(0) as usize {
        return Err(short());
    }
    let mut files = HashMap::new();
    let mut at = 0;
    for listed in header["data"]["files"].as_array().into_iter().flatten() {
        let n = listed["rows"].as_u64().unwrap_or(0) as usize;
        let (sample, path) = (listed["sample"].as_str().unwrap_or(""), listed["path"].as_str().unwrap_or(""));
        let record = made["files"][sample]["files"][path].clone();
        let holes = record["holes"].as_array().into_iter().flatten();
        let (asked, kept): (Vec<&Value>, Vec<&Value>) = holes.partition(|hole| kinds.iter().any(|k| hole["kind"] == k.as_str()));
        let read = asked.iter().map(|hole| crate::take::range(hole)).collect();
        let kept = kept.into_iter().cloned().collect();
        let rows = at..at + n;
        if rows.end > entries.len() {
            return Err(short());
        }
        let unread = unread.contains(&(sample.to_owned(), path.to_owned()));
        let plan = Plan { rows, read, kept, record, unread };
        files.insert((sample.to_owned(), path.to_owned()), plan);
        at += n;
    }
    Ok(Fill { file, files, entries, added: None })
}

/// How a fill writes one input: the old rows it keeps, each with its place in the spool, and which read input it is.
pub struct Laid {
    old: Vec<(usize, Range<usize>)>,
    read: Option<usize>,
}

/// Whether the holes a fill read failed again as they had: of the same kinds, in order.
pub(crate) fn same_kinds(filling: &Fill, lock: &Lock, reads: &[Input], read: &[Taken]) -> bool {
    let kinds = |record: &Value| -> Vec<Value> { record["holes"].as_array().into_iter().flatten().map(|h| h["kind"].clone()).collect() };
    reads.iter().zip(read).all(|(input, taken)| {
        let plan = filling.files.get(&(lock.samples[input.sample].name.clone(), input.file.at.clone()));
        let asked: Vec<(u64, u64)> = input.ranges.iter().flatten().map(|(first, last, _)| (*first, *last)).collect();
        let was: Vec<Value> = plan.map_or(Vec::new(), |plan| {
            let holes = plan.record["holes"].as_array().into_iter().flatten();
            holes.zip(ranges(&plan.record)).filter(|(_, range)| asked.contains(range)).map(|(h, _)| h["kind"].clone()).collect()
        });
        was == kinds(&taken.record)
    })
}

/// What a fill reads and keeps: for every input, how it is laid; and the files to read, each
/// with the holes it reads of them.
pub(crate) fn laid(work: &[Input], lock: &Lock, plans: &HashMap<(String, String), Plan>, entries: &[i64]) -> Result<(Vec<Input>, Vec<Laid>), String> {
    let (mut reads, mut all, mut order) = (Vec::new(), Vec::new(), 0);
    for input in work {
        let key = (lock.samples[input.sample].name.clone(), input.file.at.clone());
        let plan = plans.get(&key).ok_or_else(|| {
            format!("{} is not a file the press built on read (a bug in rvr: press with --full, and please report it)", input.file.at)
        })?;
        let (mut old, mut ranges, mut row) = (Vec::new(), Vec::new(), plan.rows.start);
        let here = &entries[plan.rows.clone()];
        let mut keep = |rows: Range<usize>, order: &mut usize| {
            old.push((*order, rows));
            *order += 1;
        };
        for &(first, last) in &plan.read {
            // No row lies in a hole: the rows before it are the file's up to there.
            let before = plan.rows.start + here.partition_point(|e| (*e as u64) < first);
            if before > row {
                keep(row..before, &mut order);
            }
            ranges.push((first, last, order));
            order += 1;
            row = before;
        }
        if plan.rows.end > row {
            keep(row..plan.rows.end, &mut order);
        }
        let read = (!ranges.is_empty()).then(|| {
            let unsummed = plan.record.get("totals").is_none();
            reads.push(Input { ranges: Some(ranges), unsummed, ..input.clone() });
            reads.len() - 1
        });
        all.push(Laid { old, read });
    }
    Ok((reads, all))
}

/// Put the old file's rows of every stretch a fill keeps in the spool, in its place: each of the
/// `columns` read, then the entries. One column of the old file is held at a time, its stretches
/// spooled by `workers` threads.
pub(crate) fn spool_old(spool: &write::Spool, fill: &Fill, columns: &[&str], laid: &[Laid], workers: usize) -> Result<(), String> {
    let file = &fill.file;
    let failed = |e: rowver::Error| rowver::plain(file.path(), &e);
    let old: Vec<&(usize, Range<usize>)> = laid.iter().flat_map(|l| &l.old).filter(|(_, rows)| !rows.is_empty()).collect();
    rowver::parallel::map(&old, workers, |(order, rows)| {
        spool.add(columns.len(), (*order, 0), write::Values::Flat(write::array(&fill.entries[rows.clone()]))).map_err(failed)
    })?;
    for (index, name) in columns.iter().enumerate() {
        let held = file.column(name).map_err(failed)?.expect("the Plan copies only the columns the file has");
        let w = rvr_recipe::Kind::named(&held.kind).expect("the file validator checked its kinds").width();
        rowver::parallel::map(&old, workers, |(order, rows)| {
            let (from, to, counts) = match &held.offsets {
                Some(offsets) => {
                    let counts = (rows.start..rows.end).map(|r| offsets[r + 1] - offsets[r]).collect();
                    (offsets[rows.start], offsets[rows.end], Some(counts))
                }
                None => (rows.start, rows.end, None),
            };
            let content = NumericBuffer { kind: held.kind.clone(), count: to - from, bytes: held.bytes[from * w..to * w].to_vec() };
            let values = match counts {
                Some(counts) => write::Values::Jagged { counts, content },
                None => write::Values::Flat(content),
            };
            spool.add(index, (*order, 0), values).map_err(failed)
        })?;
    }
    Ok(())
}

/// Each input's whole after a fill: the old file's rows and record, with what its holes read
/// again gave; its holes now are those not read, and those that failed again.
pub(crate) fn merged(
    work: &[Input],
    lock: &Lock,
    filling: &Fill,
    laid: &[Laid],
    read: Vec<Taken>,
    recipe: &rvr_recipe::Recipe,
) -> Result<Vec<Taken>, String> {
    let mut out = Vec::new();
    for (input, laid) in work.iter().zip(laid) {
        let plan = &filling.files[&(lock.samples[input.sample].name.clone(), input.file.at.clone())];
        let taken = laid.read.map(|at| &read[at]);
        // A file the old file did not read (a sample added) is recorded as it was read now.
        let mut record = match (&plan.record, taken) {
            (Value::Null, Some(taken)) => taken.record.clone(),
            _ => plan.record.clone(),
        };
        let mut holes: Vec<Value> = plan.kept.clone();
        let old: usize = laid.old.iter().map(|(_, rows)| rows.len()).sum();
        let (mut rows, mut entries, mut cutflow) = (old, 0, Vec::new());
        if let Some(taken) = taken {
            (rows, entries, cutflow) = (old + taken.rows, taken.entries, taken.cutflow.clone());
            holes.extend(taken.record["holes"].as_array().into_iter().flatten().cloned());
            // The rows kept and the rows read are of one ROOT file, as the agreement check found.
            match (&record["uuid"], &taken.record["uuid"]) {
                (Value::String(was), Value::String(now)) if was != now && !plan.unread => {
                    return Err(format!("{} is another ROOT file than the one the filled file read (its UUID was {was}, it is {now}): a bug in rvr, as the lock was checked against the file; please report it", input.file.at));
                }
                (_, now @ Value::String(_)) => record["uuid"] = now.clone(),
                _ => {}
            }
            // The events read now add to the file's totals over them; one over another tree,
            // read with the whole file, is the one it had, if it had one.
            // (A file the old file did not read is its read alone, and one not read now keeps what it had.)
            if !recipe.totals.is_empty() && !plan.record.is_null() && taken.record.get("totals").is_some() {
                let (was, now) = (record["totals"].clone(), &taken.record["totals"]);
                let mut sums = serde_json::Map::new();
                for total in &recipe.totals {
                    let (was, now) = (was[&total.name].as_f64(), now[&total.name].as_f64());
                    let sum = match (&total.tree, was, now) {
                        (Some(_), Some(was), _) => was,
                        (Some(_), None, Some(now)) => now,
                        (Some(_), None, None) => return Err(format!("{}: [totals] {} is not in its record, and a fill of some of its entries does not read the tree it sums: press with --full", input.file.at, total.name)),
                        (None, ..) => was.unwrap_or(0.0) + now.unwrap_or(0.0),
                    };
                    if !sum.is_finite() {
                        return Err(format!("{}: [totals] {}: its sum is not a finite number: give those events a finite value, as with if(…)", input.file.at, total.name));
                    }
                    sums.insert(total.name.clone(), sum.into());
                }
                record["totals"] = Value::Object(sums);
            }
        }
        holes.sort_by_key(|hole| hole["entries"][0].as_u64());
        let object = record.as_object_mut().expect("a checked file record is an object");
        match holes.is_empty() {
            true => object.shift_remove("holes"),
            false => object.insert("holes".into(), holes.into()),
        };
        let missing = missed(input, &ranges(&record));
        out.push(Taken { rows, record, entries, missing, cutflow });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Input;
    use rowver::study::lock::File;

    #[test]
    fn a_fill_keeps_the_rows_around_each_hole_it_reads() {
        let pin = |at: &str| File { at: at.into(), bytes: 10, adler32: None, uuid: String::new(), entries: 10, stop: None, take: None };
        let (file, other) = (pin("f.root"), pin("g.root"));
        let sample = rowver::study::lock::Sample {
            name: "s".into(),
            label: "s".into(),
            asked: Vec::new(),
            tree: "t".into(),
            branches: String::new(),
            events: Events::All,
            events_total: 20,
            events_read: 20,
            metadata: Default::default(),
            files: Vec::new(),
        };
        let lock = Lock { resolved: String::new(), samples_toml_sha256: String::new(), samples: vec![sample], branches: Vec::new() };
        let input = |file: &File| Input { sample: 0, tree: "t".into(), file: file.clone(), read_at: file.at.clone(), chosen: None, kept: None, before: None, ranges: None, order: 0, unsummed: false };
        let plan = |rows: std::ops::Range<usize>, read: Vec<(u64, u64)>| crate::fill::Plan { rows, read, kept: Vec::new(), record: Value::Null, unread: false };
        let plans = HashMap::from([
            (("s".to_string(), "f.root".to_string()), plan(0..5, vec![(3, 5), (6, 8)])),
            (("s".to_string(), "g.root".to_string()), plan(5..7, vec![])),
        ]);
        let entries = [0, 1, 2, 8, 9, 0, 1];
        let (reads, all) = laid(&[input(&file), input(&other)], &lock, &plans, &entries).unwrap();
        let old: Vec<Vec<String>> = all
            .iter()
            .map(|laid| laid.old.iter().map(|(order, rows)| format!("{order}: rows {rows:?} {:?}", &entries[rows.clone()])).collect())
            .collect();
        assert_eq!(old, [vec!["0: rows 0..3 [0, 1, 2]", "3: rows 3..5 [8, 9]"], vec!["4: rows 5..7 [0, 1]"]]);
        assert_eq!(all.iter().map(|laid| laid.read).collect::<Vec<_>>(), [Some(0), None]);
        assert_eq!(reads.len(), 1, "one input for f.root, reading both its holes");
        assert_eq!(reads[0].ranges, Some(vec![(3, 5, 1), (6, 8, 2)]));
        // A file with no totals yet, all of whose chosen entries lay in holes, has its other
        // trees summed by the fill that first reads some of it, though it reads only some holes.
        assert!(reads[0].unsummed && crate::take::whole(&reads[0], 10));
        let mut summed = plans;
        summed.get_mut(&("s".to_string(), "f.root".to_string())).unwrap().record = serde_json::json!({"totals": {"w": 1.0}});
        let (reads, _) = laid(&[input(&file), input(&other)], &lock, &summed, &entries).unwrap();
        assert!(!reads[0].unsummed && !crate::take::whole(&reads[0], 10));
    }
}
