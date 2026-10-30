//! `rvr press --fill`: read only the holes of a file the same study pressed, and write the file
//! again with their rows in place. Within each input file, the file's rows and the new ones are
//! merged by entry: a hole is a range of entries, so the merge is the old rows before it, its
//! rows, then the old rows after. Every column is packed again (each is compressed whole), but
//! nothing but the holes is read from storage. The study must be the one that pressed the file,
//! except that the lock may have been resolved again for the files whose holes are read (a file
//! put back where it was), and for where a dataset's copies are: every other file must be the
//! lock's as it was, and a file the old file holds rows of must be the same ROOT file (its UUID).

use crate::press::{chosen_in, ranges, Input, Output, Taken};
use crate::samples::resolve::Lock;
use rowver::write;
use rowver::NumericBuffer;
use rvr_recipe::Kind;
use crate::samples::spec::Events;
use rowver::rvr::{Reader, HOLE_KINDS};
use serde_json::Value;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

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

/// A file with holes, and what filling it reads.
pub struct Fill {
    /// The file being filled.
    pub file: Reader,
    /// By sample and `at`, every file the old file read.
    pub files: HashMap<(String, String), Plan>,
    /// Each row's entry in its file.
    pub entries: Vec<i64>,
}

/// The kinds `--fill` was given (`tape,unreachable`), or every kind.
pub fn kinds(given: Option<&str>) -> Result<Vec<String>, String> {
    let Some(given) = given else {
        return Ok(HOLE_KINDS.iter().map(|k| k.to_string()).collect());
    };
    given
        .split(',')
        .map(|kind| match HOLE_KINDS.contains(&kind.trim()) {
            true => Ok(kind.trim().to_string()),
            false => Err(format!("--fill takes kinds of hole from {}, not {kind:?}", HOLE_KINDS.join(", "))),
        })
        .collect()
}

/// Whether `--fill` would fill a file with this header (the files it read none of), or why not.
pub fn fits(
    header: &Value,
    shown: &str,
    texts: &crate::press::Texts,
    lock: &Lock,
    asked: &[Events],
    kinds: &[String],
) -> Result<std::collections::HashSet<(String, String)>, String> {
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
    let old: Lock = toml::from_str(text("lock")).map_err(|e| format!("{shown}: its samples.lock cannot be read ({e})"))?;
    // Only a file whose holes are read may have been resolved again; one the old file holds rows
    // of must be the same ROOT file.
    let read: Vec<(&str, &str)> = wanted.iter().map(|h| (h.sample.as_str(), h.file.as_str())).collect();
    // A file with a hole this fill does not read keeps that hole as the old file found it.
    let kept: Vec<(&str, &str)> = holes.iter().filter(|h| !kinds.contains(&h.kind)).map(|h| (h.sample.as_str(), h.file.as_str())).collect();
    // The entries chosen in each file's holes, to tell a file the old file read none of.
    let chosen = rowver::rvr::chosen_in_holes(header).map_err(|e| format!("{shown}: {e}"))?;
    let mut missing: HashMap<(&str, &str), u64> = HashMap::new();
    for (hole, n) in holes.iter().zip(&chosen) {
        *missing.entry((hole.sample.as_str(), hole.file.as_str())).or_default() += n;
    }
    let unread = |sample: &str, at: &str| {
        let record = &made["files"][sample]["files"][at];
        let chosen = record["take"].as_u64().or(record["stop"].as_u64()).or(record["entries"].as_u64());
        chosen.is_some_and(|n| missing.get(&(sample, at)) == Some(&n))
    };
    // A file is the same in both locks but for what the press plans of it (stop, take) and where
    // its copies are (disk, which a Rucio rule changes) and when it was written.
    let unplanned = |file: &crate::samples::resolve::File| crate::samples::resolve::File {
        stop: None,
        take: None,
        disk: None,
        modified: None,
        ..file.clone()
    };
    if old.sample.len() != lock.sample.len() {
        return not("samples.lock has other samples now");
    }
    for (was, now) in old.sample.iter().zip(&lock.sample) {
        if was.name != now.name || was.tree != now.tree || was.file.len() != now.file.len() {
            return not(&format!("samples.lock has other files of {} now", now.name));
        }
        for (a, b) in was.file.iter().zip(&now.file) {
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
            if a.uuid.is_some() && a.uuid != b.uuid && (!unread(&now.name, &b.at) || kept.contains(&at)) {
                return not(&format!("{} is another ROOT file now (its UUID), of which the file holds rows or holes", b.at));
            }
        }
    }
    for (sample, want) in lock.sample.iter().zip(asked) {
        if made["files"][&sample.name]["events"] != want.record() {
            return Err(format!("{shown} chose other events of {} than this press would: fill it with the --events, --random and --seed it was pressed with", sample.name));
        }
    }
    let read = holes.iter().map(|h| (h.sample.clone(), h.file.clone()));
    Ok(read.filter(|(sample, at)| unread(sample, at)).collect())
}

/// What filling the file at `path` reads, or why it cannot (see `fits`).
pub fn plan(
    path: &Path,
    texts: &crate::press::Texts,
    lock: &Lock,
    asked: &[Events],
    kinds: &[String],
) -> Result<Fill, String> {
    let shown = path.display();
    if !path.exists() {
        return Err(format!("{shown} is not there: rvr press makes it, and --fill fills its holes"));
    }
    let file = Reader::open(path).map_err(|e| format!("{shown} could not be read ({e})"))?;
    let header = file.header().clone();
    let unread = fits(&header, &shown.to_string(), texts, lock, asked, kinds)?;
    let made = &header["made"];
    let entries = file.entry().map_err(|e| format!("{shown} could not be read ({e})"))?;
    if entries.len() < header["data"]["rows"].as_u64().unwrap_or(0) as usize {
        return Err(format!("{shown}: its files hold more rows than it has"));
    }
    let mut files = HashMap::new();
    let mut at = 0;
    for listed in header["data"]["files"].as_array().into_iter().flatten() {
        let n = listed["rows"].as_u64().unwrap_or(0) as usize;
        let (sample, path) = (listed["sample"].as_str().unwrap_or(""), listed["path"].as_str().unwrap_or(""));
        let record = made["files"][sample]["files"][path].clone();
        let holes = record["holes"].as_array().into_iter().flatten();
        let (asked, kept): (Vec<&Value>, Vec<&Value>) = holes.partition(|hole| kinds.iter().any(|k| hole["kind"] == k.as_str()));
        let read = asked.iter().map(|hole| crate::press::range(hole)).collect();
        let kept = kept.into_iter().cloned().collect();
        let rows = at..at + n;
        if rows.end > entries.len() {
            return Err(format!("{shown}: its files hold more rows than it has"));
        }
        let unread = unread.contains(&(sample.to_owned(), path.to_owned()));
        let plan = Plan { rows, read, kept, record, unread };
        files.insert((sample.to_owned(), path.to_owned()), plan);
        at += n;
    }
    Ok(Fill { file, files, entries })
}

/// How a fill writes one input: the old file's rows it keeps, in stretches between the holes it
/// reads, each with its place among the spool's pieces and its entries; and which of the inputs
/// read is this one, when it has holes to read.
pub(crate) struct Laid<'p> {
    old: Vec<(usize, std::ops::Range<usize>, &'p [i64])>,
    read: Option<usize>,
}

/// Whether the holes a fill read failed again as they had: of the same kinds, in order.
pub(crate) fn same_kinds(filling: &Fill, lock: &Lock, reads: &[Input], read: &[Taken]) -> bool {
    let kinds = |record: &Value| -> Vec<Value> { record["holes"].as_array().into_iter().flatten().map(|h| h["kind"].clone()).collect() };
    reads.iter().zip(read).all(|(input, taken)| {
        let plan = filling.files.get(&(lock.sample[input.sample].name.clone(), input.file.at.clone()));
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
pub(crate) fn laid<'a, 'p>(
    work: &[Input<'a>],
    lock: &Lock,
    plans: &'p HashMap<(String, String), crate::fill::Plan>,
    entries: &'p [i64],
) -> Result<(Vec<Input<'a>>, Vec<Laid<'p>>), String> {
    let (mut reads, mut all, mut order) = (Vec::new(), Vec::new(), 0);
    for input in work {
        let key = (lock.sample[input.sample].name.clone(), input.file.at.clone());
        let plan = plans.get(&key).ok_or_else(|| {
            format!("{} is not a file the press being filled read (a bug in rvr: press with --full, and please report it)", input.file.at)
        })?;
        let (mut old, mut ranges, mut row) = (Vec::new(), Vec::new(), plan.rows.start);
        let here = &entries[plan.rows.clone()];
        let mut keep = |rows: std::ops::Range<usize>, order: &mut usize| {
            old.push((*order, rows.clone(), &entries[rows]));
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
            reads.push(Input { ranges: Some(ranges), ..input.clone() });
            reads.len() - 1
        });
        all.push(Laid { old, read });
    }
    Ok((reads, all))
}

/// Put the old file's rows of every stretch a fill keeps in the spool, in its place: each
/// column, then the entries; each column's kind and whether it is a list. One column of the old
/// file is held at a time.
pub(crate) fn spool_old(
    spool: &write::Spool,
    file: &rowver::rvr::Reader,
    outputs: &[Output],
    laid: &[Laid],
) -> Result<Vec<(Kind, bool)>, String> {
    let failed = |e: rowver::Error| format!("the file being filled could not be read ({e})");
    let old: Vec<&(usize, std::ops::Range<usize>, &[i64])> =
        laid.iter().flat_map(|l| &l.old).filter(|(_, rows, _)| !rows.is_empty()).collect();
    for (order, _, entries) in &old {
        spool.add(outputs.len(), (*order, 0), write::Values::Flat(write::array(entries))).map_err(failed)?;
    }
    let mut kinds = Vec::new();
    for (index, (name, _)) in outputs.iter().enumerate() {
        let held = file.column(name).map_err(failed)?.ok_or_else(|| format!("the file being filled has no column {name}"))?;
        let kind = Kind::named(&held.kind).map_err(|_| format!("the file being filled holds {name} as {}, a kind rvr does not press", held.kind))?;
        let w = kind.width();
        kinds.push((kind, held.offsets.is_some()));
        for (order, rows, _) in &old {
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
            spool.add(index, (*order, 0), values).map_err(failed)?;
        }
    }
    Ok(kinds)
}

/// Each input's whole after a fill: the old file's rows and record, with what its holes read
/// again gave; its holes now are those not read, and those that failed again.
pub(crate) fn merged(
    work: &[Input],
    lock: &Lock,
    filling: &Fill,
    laid: &[Laid],
    read: Vec<Taken>,
    kinds: &[(Kind, bool)],
) -> Result<Vec<Taken>, String> {
    let mut out = Vec::new();
    for (input, laid) in work.iter().zip(laid) {
        let plan = &filling.files[&(lock.sample[input.sample].name.clone(), input.file.at.clone())];
        let mut record = plan.record.clone();
        let mut holes: Vec<Value> = plan.kept.clone();
        let old: usize = laid.old.iter().map(|(_, rows, _)| rows.len()).sum();
        let (mut rows, mut entries, mut cutflow) = (old, 0, Vec::new());
        if let Some(taken) = laid.read.map(|at| &read[at]) {
            if !taken.kinds.is_empty() && taken.kinds != kinds {
                return Err(format!("{}: a column is not the same kind as in the file being filled: press with --full", input.file.at));
            }
            (rows, entries, cutflow) = (old + taken.rows, taken.entries, taken.cutflow.clone());
            holes.extend(taken.record["holes"].as_array().into_iter().flatten().cloned());
            // The rows kept and the rows read must be of one ROOT file.
            match (&record["uuid"], &taken.record["uuid"]) {
                (Value::String(was), Value::String(now)) if was != now && !plan.unread => {
                    return Err(format!("{} is another ROOT file than the one the filled file read (its UUID was {was}, it is {now}): press with --full", input.file.at));
                }
                (_, now @ Value::String(_)) => record["uuid"] = now.clone(),
                _ => {}
            }
            if let Some(served) = taken.record.get("served") {
                record["served"] = served.clone();
            }
        }
        holes.sort_by_key(|hole| hole["entries"][0].as_u64());
        let object = record.as_object_mut().ok_or("a file's record is not an object")?;
        match holes.is_empty() {
            true => object.shift_remove("holes"),
            false => object.insert("holes".into(), holes.into()),
        };
        let missing = ranges(&record).iter().map(|(first, last)| chosen_in(input, *first, *last)).sum();
        out.push(Taken { kinds: kinds.to_vec(), rows, record, entries, missing, cutflow });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::press::Input;
    use crate::samples::resolve::File;

    #[test]
    fn a_fill_keeps_the_rows_around_each_hole_it_reads() {
        let file: File = toml::from_str("at = \"f.root\"\nbytes = 10\nentries = 10").unwrap();
        let other: File = toml::from_str("at = \"g.root\"\nbytes = 10\nentries = 10").unwrap();
        let lock: Lock = toml::from_str("format = \"rvr-lock\"\nversion = 1\nresolved = \"\"\nsamples_toml_sha256 = \"\"\n[[sample]]\nname = \"s\"\nlabel = \"s\"\nfrom = \"s\"\nasked = []\ntree = \"t\"\nevents = \"all\"\nevents_total = 20\nevents_read = 20\nmetadata = {}\nfile = []\n").unwrap();
        let input = |file| Input { sample: 0, tree: "t", file, chosen: None, kept: None, before: None, ranges: None, order: 0 };
        let plan = |rows: std::ops::Range<usize>, read: Vec<(u64, u64)>| crate::fill::Plan { rows, read, kept: Vec::new(), record: Value::Null, unread: false };
        let plans = HashMap::from([
            (("s".to_string(), "f.root".to_string()), plan(0..5, vec![(3, 5), (6, 8)])),
            (("s".to_string(), "g.root".to_string()), plan(5..7, vec![])),
        ]);
        let entries = [0, 1, 2, 8, 9, 0, 1];
        let (reads, all) = laid(&[input(&file), input(&other)], &lock, &plans, &entries).unwrap();
        let old: Vec<Vec<String>> = all
            .iter()
            .map(|laid| laid.old.iter().map(|(order, rows, entries)| format!("{order}: rows {rows:?} {entries:?}")).collect())
            .collect();
        assert_eq!(old, [vec!["0: rows 0..3 [0, 1, 2]", "3: rows 3..5 [8, 9]"], vec!["4: rows 5..7 [0, 1]"]]);
        assert_eq!(all.iter().map(|laid| laid.read).collect::<Vec<_>>(), [Some(0), None]);
        assert_eq!(reads.len(), 1, "one input for f.root, reading both its holes");
        assert_eq!(reads[0].ranges, Some(vec![(3, 5, 1), (6, 8, 2)]));
    }
}
