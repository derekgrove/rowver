use super::cancellation::Cancellation;
use super::{count, list, object, string};
use crate::choice::{per_file, Events};
use crate::numeric::NumericArray;
use crate::table::Source;
use crate::{invalid, EventIdentity, Result};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Which entries of one file a press read: the first ones, or those a random choice picked.
enum Read {
    Below(u64),
    Chosen(Vec<u64>),
}

impl Read {
    /// How many of its entries lie in `[first, last)`.
    fn within(&self, first: u64, last: u64) -> u64 {
        match self {
            Read::Below(read) => last.min(*read).saturating_sub(first),
            Read::Chosen(chosen) => {
                (chosen.partition_point(|e| *e < last) - chosen.partition_point(|e| *e < first))
                    as u64
            }
        }
    }
}

/// What a press read of each file, by sample and path, and the file's holes.
type Reads<'a> = HashMap<(&'a str, &'a str), (Read, Vec<(u64, u64)>)>;

/// A file record's holes, checked: each `{entries: [first, last], kind, why}`, `first < last`,
/// in entry order and disjoint, every one below `limit`, with a kind of the list and a reason.
/// No `holes` key is no holes; an empty list is refused, as a press leaves the key out.
fn holes(record: &Value, label: &str, limit: u64) -> Result<Vec<(u64, u64)>> {
    let Some(held) = record.get("holes") else {
        return Ok(Vec::new());
    };
    let held = held
        .as_array()
        .filter(|held| !held.is_empty())
        .ok_or_else(|| invalid(format!("{label}.holes must be a list of one hole or more")))?;
    let mut out: Vec<(u64, u64)> = Vec::new();
    for hole in held {
        exactly(hole, &["entries", "kind", "why"], &[], &format!("A hole of {label}"))?;
        let range = hole["entries"].as_array().map(|r| r.iter().map(Value::as_u64).collect::<Vec<_>>());
        let Some([Some(first), Some(last)]) = range.as_deref().and_then(|r| <[_; 2]>::try_from(r).ok()) else {
            return Err(invalid(format!("{label}: a hole's entries must be [first, last], two whole numbers")));
        };
        let kind = string(hole, "kind")?;
        if !super::HOLE_KINDS.contains(&kind) {
            return Err(invalid(format!(
                "{label}: a hole's kind is {kind:?}, not one of {} (a newer rowver may write it)",
                super::HOLE_KINDS.join(", ")
            )));
        }
        if string(hole, "why")?.trim().is_empty() {
            return Err(invalid(format!("{label}: a hole must say why")));
        }
        if first >= last || last > limit || out.last().is_some_and(|(_, end)| *end > first) {
            return Err(invalid(format!("{label}: its holes must be ranges of the entries the press chose, in order and apart (entries {first} to {last} is not)")));
        }
        out.push((first, last));
    }
    Ok(out)
}

/// Require an object to hold exactly these keys, of which `optional` may be left out.
fn exactly(value: &Value, keys: &[&str], optional: &[&str], label: &str) -> Result<()> {
    let mut held = value.as_object().map(Map::keys).into_iter().flatten();
    if let Some(key) = held.find(|key| !keys.contains(&key.as_str())) {
        // A key this reader does not know may be one a newer rowver writes.
        return Err(invalid(format!(
            "{label} has a field {key} it may not have"
        )));
    }
    let missing = keys
        .iter()
        .find(|key| value.get(**key).is_none() && !optional.contains(key));
    if let Some(key) = missing {
        return Err(invalid(format!("{label} is missing {key}")));
    }
    Ok(())
}

/// A sample's files in row order, as `data.files` names them, with each one's record.
fn ordered<'a>(
    data: &'a Value,
    made: &'a Value,
    sample: &str,
) -> Result<Vec<(&'a str, &'a Value)>> {
    let records = object(object(object(made, "files")?, sample)?, "files")?;
    list(data, "files")?
        .iter()
        .filter(|file| file["sample"] == sample)
        .map(|file| {
            let path = string(file, "path")?;
            let record = records.get(path).ok_or_else(|| {
                invalid(format!(
                    "data.files names {path}, which made.files does not hold"
                ))
            })?;
            Ok((path, record))
        })
        .collect()
}

/// What a press read of each of a sample's files, from its `events` record and each file's
/// entries; with `choose` false, a random choice is only counted, not made again.
fn reads(
    files: &[(&str, &Value)],
    events: &Value,
    sample: &str,
    choose: bool,
) -> Result<Vec<Read>> {
    let label = format!("made.files[{sample:?}].events");
    let entries: Vec<u64> = files
        .iter()
        .map(|(_, record)| count(record, "entries").map(|n| n as u64))
        .collect::<Result<_>>()?;
    let field = |record: &Value, key: &str| match record.get(key) {
        None => Ok(None),
        Some(_) => count(record, key).map(|n| Some(n as u64)),
    };
    let wrong =
        |path: &str, what: &str| invalid(format!("{path}: its {what} is not what {label} gives"));
    let choice = Events::read(events).map_err(|e| invalid(format!("{label}: {e}")))?;
    match choice {
        Events::All => {
            let read = |((path, record), &n): (&(&str, &Value), &u64)| match (
                field(record, "stop")?,
                field(record, "take")?,
            ) {
                (None, None) => Ok(Read::Below(n)),
                _ => Err(wrong(path, "stop or take")),
            };
            files.iter().zip(&entries).map(read).collect()
        }
        Events::First(mut left) => {
            let mut read = |((path, record), &n): (&(&str, &Value), &u64)| {
                let read = n.min(left);
                left -= read;
                let stop = (read < n).then_some(read);
                if field(record, "stop")? != stop || field(record, "take")?.is_some() {
                    return Err(wrong(path, "stop"));
                }
                Ok(Read::Below(read))
            };
            files.iter().zip(&entries).map(&mut read).collect()
        }
        Events::Random { n, seed } => {
            let taken: Vec<u64> = files
                .iter()
                .map(|(path, record)| match field(record, "stop")? {
                    None => field(record, "take")?.ok_or_else(|| wrong(path, "take")),
                    Some(_) => Err(wrong(path, "stop")),
                })
                .collect::<Result<_>>()?;
            if taken.iter().sum::<u64>() != n.min(entries.iter().sum()) {
                return Err(invalid(format!(
                    "The files' take does not add up to the {n} events {label} chose"
                )));
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

/// Check the samples and files the header records against each other, without the arrays:
/// `data.samples`, `data.files` and `made.files` name the same samples and files, and every
/// sample's `events` record agrees with what each of its files says was read.
pub(super) fn records(data: &Value, made: &Value) -> Result<()> {
    let mut names = HashSet::new();
    for record in list(data, "samples")? {
        exactly(
            record,
            &["name", "label", "rows"],
            &[],
            "A data.samples record",
        )?;
        let name = string(record, "name")?;
        string(record, "label")?;
        count(record, "rows")?;
        if name.is_empty() || !names.insert(name) {
            return Err(invalid("Sample names must be nonempty and unique"));
        }
    }
    let sources = object(made, "files")?.as_object().unwrap();
    if sources.len() != names.len() || sources.keys().any(|name| !names.contains(name.as_str())) {
        return Err(invalid("made.files samples differ from data.samples"));
    }
    let mut listed = HashSet::new();
    for file in list(data, "files")? {
        exactly(
            file,
            &["sample", "path", "rows"],
            &[],
            "A data.files record",
        )?;
        let pair = (string(file, "sample")?, string(file, "path")?);
        if !sources.contains_key(pair.0) || !listed.insert(pair) {
            return Err(invalid(
                "data.files names a file twice, or a sample made.files does not hold",
            ));
        }
        count(file, "rows")?;
    }
    let mut holed = false;
    for (name, source) in sources {
        let label = format!("made.files[{name:?}]");
        exactly(source, &["events", "files", "metadata"], &[], &label)?;
        object(source, "metadata")?;
        for (path, record) in object(source, "files")?.as_object().unwrap() {
            let label = format!("{label}.files[{path:?}]");
            let fields = ["tree", "uuid", "entries", "stop", "take", "served", "holes"];
            exactly(record, &fields, &["stop", "take", "served", "holes"], &label)?;
            let limit = match record.get("stop") {
                Some(_) => count(record, "stop")?,
                None => count(record, "entries")?,
            } as u64;
            let holes = holes(record, &label, limit)?;
            holed |= !holes.is_empty();
            // A file the press could not open has no UUID of its own when the lock had none to
            // give: one hole of every entry it chose, or a file it chose no entry of.
            let unread = (limit > 0 && holes == [(0, limit)]) || limit == 0 || record.get("take") == Some(&0.into());
            if record["uuid"].is_null() && !unread {
                return Err(invalid(format!("{label}: only a file the press could not open (one hole of every entry it chose, or a file it chose none of) may have no uuid")));
            }
            if let Some(served) = record.get("served") {
                exactly(served, &["rse", "host"], &[], &format!("{label}.served"))?;
                if string(served, "rse")?.is_empty() || string(served, "host")?.is_empty() {
                    return Err(invalid(format!(
                        "{label}.served must name its rse and host"
                    )));
                }
            }
            if path.is_empty()
                || (!record["uuid"].is_null() && string(record, "uuid")?.is_empty())
                || string(record, "tree")?.is_empty()
            {
                return Err(invalid(
                    "Source files must have nonempty paths, UUIDs, and tree names",
                ));
            }
            if !listed.contains(&(name.as_str(), path.as_str())) {
                return Err(invalid(format!(
                    "{path} of {name} is in made.files but not in data.files"
                )));
            }
        }
        let files = ordered(data, made, name)?;
        reads(&files, object(source, "events")?, name, false)?;
    }
    // A pressed file (whose samples say what was read) with holes says what each sample misses.
    for (name, source) in sources {
        let metadata = &source["metadata"];
        if holed && metadata.get("events_read").is_some() && metadata.get("events_missing").is_none() {
            return Err(invalid(format!("made.files[{name:?}].metadata has no events_missing, though the file has holes")));
        }
    }
    Ok(())
}

/// How many entries the choice chose in each hole, in the order of `rvr::holes`.
pub(super) fn chosen_in_holes(data: &Value, made: &Value) -> Result<Vec<u64>> {
    // What the choice read of every file of a sample with a hole, by sample and path.
    let mut read: HashMap<(&str, &str), Read> = HashMap::new();
    let samples = object(made, "files")?.as_object().into_iter().flatten();
    let holed = samples.filter(|(_, sample)| {
        let files = sample["files"].as_object().into_iter().flat_map(|f| f.values());
        files.into_iter().any(|record| record.get("holes").is_some())
    });
    for (name, _) in holed {
        let files = ordered(data, made, name)?;
        let events = object(object(object(made, "files")?, name)?, "events")?;
        for ((path, _), what) in files.iter().zip(reads(&files, events, name, true)?) {
            read.insert((name.as_str(), path), what);
        }
    }
    let mut out = Vec::new();
    for file in list(data, "files")? {
        let (name, path) = (string(file, "sample")?, string(file, "path")?);
        let record = &made["files"][name]["files"][path];
        for (first, last) in holes(record, path, u64::MAX)? {
            let what = read.get(&(name, path)).ok_or_else(|| invalid(format!("{path} of {name} is not in made.files")))?;
            out.push(what.within(first, last));
        }
    }
    Ok(out)
}

/// Validate sample and file spans against the arrays, choose a random choice's entries again,
/// and recover each file's identity.
pub(super) fn validate(
    data: &Value,
    made: &Value,
    sample: &NumericArray,
    entry: &NumericArray,
    rows: usize,
    cancellation: Cancellation<'_>,
) -> Result<Vec<Source>> {
    let mut names = HashMap::new();
    let mut sample_end = 0_usize;
    for (index, record) in list(data, "samples")?.iter().enumerate() {
        names.insert(string(record, "name")?, index);
        let end = sample_end
            .checked_add(count(record, "rows")?)
            .filter(|end| *end <= rows)
            .ok_or_else(|| invalid("Sample rows exceed total rows"))?;
        for row in sample_end..end {
            cancellation.at(row)?;
            if sample.signed(row)? != index as i64 {
                return Err(invalid("Sample identity arrays disagree with sample spans"));
            }
        }
        sample_end = end;
    }
    if sample_end != rows {
        return Err(invalid("Sample row counts do not sum to rows"));
    }
    let sources = object(made, "files")?;
    let mut read: Reads = HashMap::new();
    for name in names.keys() {
        cancellation.check()?;
        let files = ordered(data, made, name)?;
        let events = object(object(sources, name)?, "events")?;
        let mut missing = 0;
        for ((path, record), what) in files.iter().zip(reads(&files, events, name, true)?) {
            let label = format!("made.files[{name:?}].files[{path:?}]");
            let holes = holes(record, &label, u64::MAX)?;
            for &(first, last) in &holes {
                match what.within(first, last) {
                    0 => return Err(invalid(format!("{label}: its hole of entries {first} to {last} holds no entry the press chose"))),
                    n => missing += n,
                }
            }
            read.insert((name, path), (what, holes));
        }
        // `events_read` is a press's own metadata, which `rvr validate` checks; `events_missing`
        // is the holes'.
        let said = sources[*name]["metadata"].get("events_missing").map(Value::as_u64);
        if said.is_some_and(|n| n != Some(missing)) {
            return Err(invalid(format!("sample {name}: its events_missing must be the {missing} entries chosen that lie in its holes")));
        }
    }
    let mut files = Vec::new();
    let mut start = 0_usize;
    let mut previous_sample = 0;
    for record in list(data, "files")? {
        let name = string(record, "sample")?;
        let path = string(record, "path")?;
        let index = names[name];
        if index < previous_sample {
            return Err(invalid("Input files are out of sample order"));
        }
        previous_sample = index;
        let source = &sources[name]["files"][path];
        let end = start
            .checked_add(count(record, "rows")?)
            .filter(|end| *end <= rows)
            .ok_or_else(|| invalid("Input file rows exceed total rows"))?;
        let (allowed, holes) = &read[&(name, path)];
        let mut seen_entries = HashSet::new();
        for row in start..end {
            cancellation.at(row)?;
            let source_entry = entry.signed(row)?;
            let inside = u64::try_from(source_entry).is_ok_and(|at| {
                let hole = holes.partition_point(|(_, last)| *last <= at);
                let unread = holes.get(hole).is_some_and(|(first, _)| *first <= at);
                !unread
                    && match allowed {
                        Read::Below(read) => at < *read,
                        Read::Chosen(chosen) => chosen.binary_search(&at).is_ok(),
                    }
            });
            if sample.signed(row)? != index as i64 || !inside || !seen_entries.insert(source_entry)
            {
                return Err(invalid(format!("{path}: its rows disagree with their sample, repeat an entry, or hold an entry the press did not read (or one in a hole)")));
            }
        }
        files.push(Source {
            identity: EventIdentity {
                sample: name.into(),
                file: path.into(),
                entry: 0,
                uuid: source["uuid"].as_str().unwrap_or_default().into(),
                tree: string(source, "tree")?.into(),
            },
            start,
            end,
        });
        start = end;
    }
    if start != rows {
        return Err(invalid("Input file rows do not sum to rows"));
    }
    Ok(files)
}
