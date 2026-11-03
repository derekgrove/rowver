//! The samples and files a header records (docs/FORMAT.md §3, §5, §6, §6.1, §6.2): their records
//! on opening; in full validation, the event choice made again with the holes and counts against
//! it, then each row's sample and entry.

use super::samples::whole as written_whole;
use super::{count, exactly, list, nonempty, object, text, whole_count, MEND};
use crate::choice::{per_file, Events};
use crate::numeric::NumericArray;
use crate::study::lock::{uuid, MOST_ENTRIES};
use crate::{invalid, Result};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Which entries of one file a press read: the first ones, or those a random choice picked.
pub(super) enum Read {
    Below(u64),
    Chosen(Vec<u64>),
}

impl Read {
    /// How many of its entries lie in `[first, last)`.
    fn within(&self, first: u64, last: u64) -> u64 {
        match self {
            Read::Below(read) => last.min(*read).saturating_sub(first),
            Read::Chosen(chosen) => (chosen.partition_point(|e| *e < last) - chosen.partition_point(|e| *e < first)) as u64,
        }
    }

    /// Whether it holds `entry`.
    fn holds(&self, entry: u64) -> bool {
        match self {
            Read::Below(read) => entry < *read,
            Read::Chosen(chosen) => chosen.binary_search(&entry).is_ok(),
        }
    }
}

/// Whether a press read a file of these holes, as `rvr::read_file` says.
pub(super) fn read_file(read: &Read, holes: &[(u64, u64)]) -> bool {
    let lost = holes.iter().map(|(first, last)| read.within(*first, *last)).sum();
    super::read_file(read.within(0, u64::MAX), lost, matches!(read, Read::Chosen(_)))
}

/// What the choice read of each file, by sample and path, with the file's holes.
pub(super) type Reads<'a> = HashMap<(&'a str, &'a str), (Read, Vec<(u64, u64)>)>;

/// A file record's holes: each exactly `{entries: [first, last], kind, why}`, `first < last`, in
/// entry order and apart, every one below `limit`, of a kind of the list and with a reason. No
/// `holes` key is no holes; an empty list is refused, as a press leaves the key out.
fn holes(record: &Value, place: &str, limit: u64) -> Result<Vec<(u64, u64)>> {
    let Some(held) = record.get("holes") else {
        return Ok(Vec::new());
    };
    let held = held.as_array().filter(|held| !held.is_empty()).ok_or_else(|| {
        invalid(format!("{place}.holes must be a list of one hole or more, and a file with no hole has no holes key (docs/FORMAT.md §6.2): {MEND}"))
    })?;
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (index, hole) in held.iter().enumerate() {
        let place = format!("{place}.holes[{index}]");
        exactly(hole, &place, &["entries", "kind", "why"], &[])?;
        let range = hole["entries"].as_array().map(|r| r.iter().map(whole_count).collect::<Vec<_>>());
        let Some([Some(first), Some(last)]) = range.as_deref().and_then(|r| <[_; 2]>::try_from(r).ok()) else {
            return Err(invalid(format!("{place}.entries must be [first, last], two whole numbers from 0 to 2^63 − 1 (docs/FORMAT.md §6.2): {MEND}")));
        };
        let kind = text(hole, &place, "kind")?;
        if !super::HOLE_KINDS.contains(&kind) {
            return Err(invalid(format!(
                "{place}.kind is {kind:?}, not one of {} (docs/FORMAT.md §6.2): a newer rowver may write it; update rowver",
                super::HOLE_KINDS.join(", ")
            )));
        }
        if text(hole, &place, "why")?.trim().is_empty() {
            return Err(invalid(format!("{place}.why must say what failed, as text that is not blank (docs/FORMAT.md §6.2): {MEND}")));
        }
        if first >= last || last > limit || out.last().is_some_and(|(_, end)| *end > first) {
            return Err(invalid(format!("{place}.entries is [{first}, {last}], and holes are ranges first < last, in entry order and apart, within what the file's choice read ({limit} entries) (docs/FORMAT.md §6.2): {MEND}")));
        }
        out.push((first, last));
    }
    Ok(out)
}

/// A sample's files in row order, as `data.files` names them, with each one's record.
fn ordered<'a>(data: &'a Value, made: &'a Value, sample: &str) -> Vec<(&'a str, &'a Value)> {
    let records = &made["files"][sample]["files"];
    let files = data["files"].as_array().into_iter().flatten();
    files.filter(|file| file["sample"] == sample).map(|file| {
        let path = file["path"].as_str().unwrap_or_default();
        (path, &records[path])
    }).collect()
}

/// What a press read of each of a sample's files, from its `events` record and each file's
/// entries; with `choose` false, a random choice is only counted, not made again.
fn reads(files: &[(&str, &Value)], events: &Value, place: &str, choose: bool) -> Result<Vec<Read>> {
    let entries: Vec<u64> = files.iter().map(|(_, record)| whole_count(&record["entries"]).unwrap_or(0)).collect();
    let total = entries.iter().try_fold(0_u64, |sum, n| sum.checked_add(*n)).filter(|total| *total <= MOST_ENTRIES);
    let total = total.ok_or_else(|| invalid(format!("{place}: the sample's files hold more than 2^40 entries in all, more than a file's samples may, so that a random choice is made again in bounded time (docs/FORMAT.md §6.1): split the sample")))?;
    let field = |record: &Value, key: &str| record.get(key).and_then(whole_count);
    let wrong = |path: &str, what: &str| invalid(format!("{place}: the file {path}'s {what} is not what {place} gives (docs/FORMAT.md §6.1): {MEND}"));
    let choice = Events::read(events).map_err(|e| invalid(format!("{place}: {e} (docs/FORMAT.md §6.1): {MEND}")))?;
    match choice {
        Events::All => files.iter().zip(&entries).map(|((path, record), &n)| match (field(record, "stop"), field(record, "take")) {
            (None, None) => Ok(Read::Below(n)),
            _ => Err(wrong(path, "stop or take")),
        }).collect(),
        Events::First(mut left) => files.iter().zip(&entries).map(|((path, record), &n)| {
            let read = n.min(left);
            left -= read;
            let stop = (read < n).then_some(read);
            match field(record, "stop") != stop || field(record, "take").is_some() {
                true => Err(wrong(path, "stop")),
                false => Ok(Read::Below(read)),
            }
        }).collect(),
        Events::Random { n, seed } => {
            let mut taken = Vec::new();
            for (path, record) in files {
                match (field(record, "stop"), field(record, "take")) {
                    (None, Some(take)) if Some(take) <= whole_count(&record["entries"]) => taken.push(take),
                    _ => return Err(wrong(path, "take")),
                }
            }
            if taken.iter().sum::<u64>() != n.min(total) {
                return Err(invalid(format!("{place}: the files' take adds up to {}, and a random choice of {n} takes {} (docs/FORMAT.md §6.1): {MEND}", taken.iter().sum::<u64>(), n.min(total))));
            }
            if !choose {
                return Ok(taken.into_iter().map(|_| Read::Below(0)).collect());
            }
            let chosen = per_file(&entries, n, seed);
            for ((path, _), (chosen, take)) in files.iter().zip(chosen.iter().zip(&taken)) {
                if chosen.len() as u64 != *take {
                    return Err(wrong(path, "take"));
                }
            }
            Ok(chosen.into_iter().map(Read::Chosen).collect())
        }
    }
}

/// The refusal of counts that add up past what a count holds.
fn past(what: &str) -> crate::Error {
    invalid(format!("{what} add up past 2^64, and every count is from 0 to 2^63 − 1 (docs/FORMAT.md §1): {MEND}"))
}

/// Refuse a metadata value that is none of text, whole numbers, finite floats, `true` and
/// `false`, and lists and objects of those.
fn metadata_value(value: &Value, place: &str) -> Result<()> {
    match value {
        Value::Null => Err(invalid(format!("{place} is null, and metadata holds text, whole numbers, finite floats, true and false, and lists and objects of those (docs/FORMAT.md §6): {MEND}"))),
        Value::Number(n) if !written_whole(n) && !n.as_f64().is_some_and(f64::is_finite) => Err(invalid(format!("{place} is {n}, not a finite float (docs/FORMAT.md §6): {MEND}"))),
        Value::Array(values) => values.iter().enumerate().try_for_each(|(i, v)| metadata_value(v, &format!("{place}[{i}]"))),
        Value::Object(values) => values.iter().try_for_each(|(k, v)| metadata_value(v, &format!("{place}.{k}"))),
        _ => Ok(()),
    }
}

/// A total's value: a finite double written as one, with a `.` or an exponent.
pub(super) fn total(value: &Value, place: &str) -> Result<f64> {
    let number = value.as_number().filter(|n| !written_whole(n));
    number.and_then(serde_json::Number::as_f64).filter(|v| v.is_finite()).ok_or_else(|| {
        invalid(format!("{place} is {value}, and a total is a finite double, written with a . or an exponent (docs/FORMAT.md §6): {MEND}"))
    })
}

/// Check one file record of `made.files[sample].files`; whether it has holes.
fn file_record(record: &Value, place: &str) -> Result<bool> {
    let fields = ["tree", "uuid", "entries", "stop", "take", "holes", "totals"];
    exactly(record, place, &fields, &["stop", "take", "holes", "totals"])?;
    nonempty(record, place, "tree")?;
    let id = text(record, place, "uuid")?;
    if !uuid(id) {
        return Err(invalid(format!("{place}.uuid is {id:?}, and it is the UUID inside the ROOT file as the lock gives it: 36 characters, lowercase hexadecimal in groups of 8, 4, 4, 4 and 12 joined by - (docs/FORMAT.md §6): {MEND}")));
    }
    let entries = count(record, place, "entries")?;
    for key in ["stop", "take"].iter().filter(|key| record.get(**key).is_some()) {
        count(record, place, key)?;
    }
    let limit = record.get("stop").and_then(whole_count).unwrap_or(entries as u64);
    let holed = !holes(record, place, limit)?.is_empty();
    if let Some(totals) = record.get("totals") {
        let totals = totals.as_object().filter(|t| !t.is_empty()).ok_or_else(|| {
            invalid(format!("{place}.totals must be an object of one total or more, and a file without totals has no totals key (docs/FORMAT.md §6): {MEND}"))
        })?;
        for (name, value) in totals {
            total(value, &format!("{place}.totals.{name}"))?;
        }
    }
    Ok(holed)
}

/// Check the samples and files the header records against each other, without the arrays or
/// the study's texts (docs/FORMAT.md §12, "On opening").
pub(super) fn records(data: &Value, made: &Value) -> Result<()> {
    let rows = count(data, "data", "rows")?;
    let (mut names, mut seen, mut sum) = (Vec::new(), HashSet::new(), 0_usize);
    for (index, record) in list(data, "data", "samples")?.iter().enumerate() {
        let name = nonempty(record, &format!("data.samples[{index}]"), "name")?;
        let place = format!("data.samples[{name:?}]");
        exactly(record, &place, &["name", "label", "rows"], &[])?;
        nonempty(record, &place, "label")?;
        sum = sum.checked_add(count(record, &place, "rows")?).ok_or_else(|| past("data.samples' rows"))?;
        if !seen.insert(name) {
            return Err(invalid(format!("{place}: two samples have this name, and each has its own (docs/FORMAT.md §3): {MEND}")));
        }
        names.push((name, whole_count(&record["rows"]).unwrap_or(0) as usize));
    }
    if sum != rows {
        return Err(invalid(format!("data.rows is {rows}, and the samples' rows add up to {sum} (docs/FORMAT.md §3): {MEND}")));
    }
    let sources = object(made, "made", "files")?.as_object().unwrap();
    if let Some(name) = sources.keys().find(|name| !seen.contains(name.as_str())) {
        return Err(invalid(format!("made.files[{name:?}] is no sample of data.samples, and made.files holds exactly those (docs/FORMAT.md §6): {MEND}")));
    }
    let mut listed: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
    let mut paths = HashSet::new();
    let mut last = 0;
    for (index, file) in list(data, "data", "files")?.iter().enumerate() {
        let place = format!("data.files[{index}]");
        exactly(file, &place, &["sample", "path", "rows"], &[])?;
        let (sample, path, held) = (text(file, &place, "sample")?, nonempty(file, &place, "path")?, count(file, &place, "rows")?);
        let Some(at) = names.iter().position(|(name, _)| *name == sample) else {
            return Err(invalid(format!("{place}.sample is {sample:?}, which data.samples does not hold (docs/FORMAT.md §3): {MEND}")));
        };
        if at < last {
            return Err(invalid(format!("{place}: data.files lists each sample's files in the order of data.samples, and {sample} comes after {} (docs/FORMAT.md §3): {MEND}", names[last].0)));
        }
        last = at;
        if !paths.insert((sample, path)) {
            return Err(invalid(format!("{place}: data.files names {path} of {sample} twice (docs/FORMAT.md §3): {MEND}")));
        }
        listed.entry(sample).or_default().push((path, held));
    }
    let mut holed = false;
    for (name, rows) in &names {
        let place = format!("made.files[{name:?}]");
        let source = sources.get(*name).ok_or_else(|| invalid(format!("{place} is missing, and made.files holds every sample of data.samples (docs/FORMAT.md §6): {MEND}")))?;
        exactly(source, &place, &["events", "files", "metadata"], &[])?;
        let records = object(source, &place, "files")?.as_object().unwrap();
        let metadata = object(source, &place, "metadata")?.as_object().unwrap();
        metadata_value(&source["metadata"], &format!("{place}.metadata"))?;
        let files = listed.remove(name).unwrap_or_default();
        let held = files.iter().try_fold(0_usize, |sum, (_, n)| sum.checked_add(*n)).ok_or_else(|| past(&format!("the rows of {place}'s files in data.files")))?;
        if held != *rows {
            return Err(invalid(format!("data.samples[{name:?}].rows is {rows}, and its files' rows in data.files add up to {held} (docs/FORMAT.md §3): {MEND}")));
        }
        let (keys, paths): (Vec<&str>, Vec<&str>) = (records.keys().map(String::as_str).collect(), files.iter().map(|(p, _)| *p).collect());
        if keys != paths {
            return Err(invalid(format!("{place}.files holds {keys:?}, and it holds the sample's files of data.files, in that order: {paths:?} (docs/FORMAT.md §6): {MEND}")));
        }
        for (path, record) in records {
            holed |= file_record(record, &format!("{place}.files[{path:?}]"))?;
        }
        reads(&ordered(data, made, name), &source["events"], &format!("{place}.events"), false)?;
        sums(records, metadata, &paths, &place)?;
        for key in ["events_read", "events_total"] {
            if metadata.get(key).and_then(whole_count).is_none() {
                return Err(invalid(format!("{place}.metadata.{key} must be a whole number, which rvr press gives every sample (docs/FORMAT.md §5): {MEND}")));
            }
        }
    }
    for (name, _) in &names {
        let missing = made["files"][*name]["metadata"].get("events_missing");
        match (holed, missing.map(whole_count)) {
            (true, Some(Some(_))) | (false, None) => {}
            (true, _) => return Err(invalid(format!("made.files[{name:?}].metadata.events_missing must be a whole number, as the file has holes and so every sample says what it misses (docs/FORMAT.md §5): {MEND}"))),
            (false, Some(_)) => return Err(invalid(format!("made.files[{name:?}].metadata has events_missing, and only a file with holes has it (docs/FORMAT.md §5): {MEND}"))),
        }
    }
    Ok(())
}

/// Check a sample's totals: its metadata holds each its files hold, summed from +0.0 in the order
/// of `data.files` (a file without totals adds nothing).
fn sums(records: &Map<String, Value>, metadata: &Map<String, Value>, paths: &[&str], place: &str) -> Result<()> {
    let mut sums: Vec<(&str, f64)> = Vec::new();
    for path in paths {
        for (name, value) in records[*path].get("totals").and_then(Value::as_object).into_iter().flatten() {
            let value = value.as_f64().unwrap_or_default();
            match sums.iter_mut().find(|(n, _)| n == name) {
                Some((_, sum)) => *sum += value,
                None => sums.push((name, 0.0 + value)),
            }
        }
    }
    for (name, sum) in sums {
        let held = metadata.get(name).ok_or_else(|| invalid(format!("{place}.metadata has no total {name}, which its files hold (docs/FORMAT.md §6): {MEND}")))?;
        if total(held, &format!("{place}.metadata.{name}"))? != sum {
            return Err(invalid(format!("{place}.metadata.{name} is {held}, and its files' totals of {name}, added from +0.0 in the order of data.files, are {sum:?} (docs/FORMAT.md §6): {MEND}")));
        }
    }
    Ok(())
}

/// The event choice made again for every sample, each hole holding a chosen entry, totals only on
/// a file the press read an entry of, and `events_read` and `events_missing` exact: what each
/// file's rows may hold.
pub(super) fn choices(header: &Value) -> Result<Reads<'_>> {
    let (data, made) = (&header["data"], &header["made"]);
    let mut out = Reads::new();
    for sample in data["samples"].as_array().into_iter().flatten() {
        let name = sample["name"].as_str().unwrap_or_default();
        let place = format!("made.files[{name:?}]");
        let files = ordered(data, made, name);
        let (mut chosen, mut missing) = (0_u64, 0_u64);
        for ((path, record), read) in files.iter().zip(reads(&files, &made["files"][name]["events"], &format!("{place}.events"), true)?) {
            let at = format!("{place}.files[{path:?}]");
            let holes = holes(record, &at, u64::MAX)?;
            let all = read.within(0, u64::MAX);
            let mut lost = 0;
            for (index, &(first, last)) in holes.iter().enumerate() {
                match read.within(first, last) {
                    0 => return Err(invalid(format!("{at}.holes[{index}] holds {}, none of which the event choice chose, and a hole holds at least one (docs/FORMAT.md §6.2): {MEND}", super::span(first, last)))),
                    n => lost += n,
                }
            }
            if record.get("totals").is_some() && !read_file(&read, &holes) {
                return Err(invalid(format!("{at} has totals, and the press did not read the file (it took none of its entries, or every one it chose lies in a hole), so it has none (docs/FORMAT.md §6): {MEND}")));
            }
            chosen += all;
            missing += lost;
            out.insert((name, *path), (read, holes));
        }
        let metadata = &made["files"][name]["metadata"];
        let said = |key: &str| whole_count(&metadata[key]);
        if said("events_read") != Some(chosen - missing) {
            return Err(invalid(format!("{place}.metadata.events_read is {}, and the sample's event choice chose {chosen} entries, {missing} of them in holes, so it is {} (docs/FORMAT.md §5): {MEND}", metadata["events_read"], chosen - missing)));
        }
        if metadata.get("events_missing").is_some() && said("events_missing") != Some(missing) {
            return Err(invalid(format!("{place}.metadata.events_missing is {}, and {missing} entries the sample's event choice chose lie in its holes (docs/FORMAT.md §5): {MEND}", metadata["events_missing"])));
        }
    }
    Ok(out)
}

/// How many entries the choice chose in each hole, in the order of `rvr::holes`.
pub(super) fn chosen_in_holes(header: &Value) -> Result<Vec<u64>> {
    let (data, made) = (&header["data"], &header["made"]);
    let mut out = Vec::new();
    let samples = data["samples"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str());
    for name in samples {
        let files = ordered(data, made, name);
        if !files.iter().any(|(_, record)| record.get("holes").is_some()) {
            continue;
        }
        let place = format!("made.files[{name:?}]");
        for ((path, record), read) in files.iter().zip(reads(&files, &made["files"][name]["events"], &format!("{place}.events"), true)?) {
            for (first, last) in holes(record, &format!("{place}.files[{path:?}]"), u64::MAX)? {
                out.push(read.within(first, last));
            }
        }
    }
    Ok(out)
}


/// Check each row against the samples and files: the sample index running through the samples in
/// order, and each file's entries strictly increasing, each chosen and in no hole.
pub(super) fn rows(header: &Value, sample: &NumericArray, entry: &NumericArray, reads: &Reads) -> Result<()> {
    let data = &header["data"];
    for (at, (start, end)) in super::samples::spans(data)?.into_iter().enumerate() {
        for row in start..end {
            if sample.signed(row)? != at as i64 {
                return Err(invalid(format!("data.sample: row {row} holds {}, and the rows of data.samples[{at}] hold {at} (docs/FORMAT.md §3): {MEND}", sample.signed(row)?)));
            }
        }
    }
    let mut start = 0_usize;
    for (at, record) in data["files"].as_array().into_iter().flatten().enumerate() {
        let (name, path) = (record["sample"].as_str().unwrap_or_default(), record["path"].as_str().unwrap_or_default());
        let end = start.checked_add(whole_count(&record["rows"]).unwrap_or(0) as usize).ok_or_else(|| past("data.files' rows"))?;
        let (read, holes) = &reads[&(name, path)];
        let mut previous: Option<i64> = None;
        for row in start..end {
            let held = entry.signed(row)?;
            if previous.is_some_and(|previous| held <= previous) {
                return Err(invalid(format!("data.entry: row {row} holds entry {held} of data.files[{at}] after entry {}, and within a file rows are in strictly increasing order of their entries (docs/FORMAT.md §3): {MEND}", previous.unwrap_or_default())));
            }
            previous = Some(held);
            let inside = u64::try_from(held).is_ok_and(|e| {
                let hole = holes.partition_point(|(_, last)| *last <= e);
                read.holds(e) && holes.get(hole).is_none_or(|(first, _)| *first > e)
            });
            if !inside {
                return Err(invalid(format!("data.entry: row {row} holds entry {held} of data.files[{at}] ({path}), which the event choice did not choose or a hole holds (docs/FORMAT.md §3, §6.1, §6.2): {MEND}")));
            }
        }
        start = end;
    }
    Ok(())
}
