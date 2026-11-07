//! The last of a press: every file read, the rows joined in the lock's order, the file's header
//! made, its columns packed from the spool (or copied from the earlier file), its derived columns
//! worked out, signed, and put in place whole (docs/SPEC.md §7.5.1, §7.7.3, §7.8).

use crate::plan::{Base, Plan};
use rowver::rvr::grouped as count;
use crate::samples::some;
use crate::take::Taken;
use rowver::study::lock::Sample;
use rowver::write;
use rowver::write::NumericBuffer;
use serde_json::{json, Map, Value};
use std::path::Path;

/// What the core hands over once every file is read.
pub(crate) struct Done<'a> {
    /// What each file of the Plan's `work` gave, in order.
    pub taken: Vec<Taken>,
    /// Where their kept values are.
    pub spool: &'a write::Spool,
    /// When the press began.
    pub started: std::time::Instant,
    /// Whether the file is made durable before it is put in place.
    pub worth: bool,
    /// What a failure that keeps what was read says.
    pub stuck: &'a dyn Fn(String) -> String,
    /// The press's journal, told when its file is about to be put in place.
    pub journal: &'a crate::journal::Journal,
}

/// What a press counts over every file.
struct Tally {
    read: u64,
    /// Every hole, with its sample.
    holes: Vec<(String, Value)>,
    cutflow: Vec<u64>,
    /// The `data.files` records, in order.
    files: Vec<Value>,
}

/// How many events one sample kept, its file records, and the entries chosen in its holes.
fn joined(sample: &Sample, plan: &Plan, index: usize, taken: &mut impl Iterator<Item = Taken>, tally: &mut Tally) -> Result<(usize, Map<String, Value>, u64), String> {
    let (mut rows, mut records, mut missing) = (0, Map::new(), 0);
    for input in plan.work.iter().filter(|i| i.sample == index) {
        let taken = taken.next().ok_or("a file was left unread (a bug in rvr: please report it)")?;
        for (total, kept) in tally.cutflow.iter_mut().zip(&taken.cutflow) {
            *total += kept;
        }
        tally.read += taken.entries;
        missing += taken.missing;
        for hole in taken.record["holes"].as_array().into_iter().flatten() {
            tally.holes.push((sample.name.clone(), hole.clone()));
        }
        tally.files.push(json!({"sample": sample.name, "path": input.file.at, "rows": taken.rows}));
        records.insert(input.file.at.clone(), taken.record);
        rows += taken.rows;
    }
    Ok((rows, records, missing))
}

/// Every sample joined: its rows, its `made.files` record, and what it misses; and the tally.
struct Joined {
    samples: Vec<write::Sample>,
    files: Map<String, Value>,
    tally: Tally,
    missing: Vec<u64>,
}

/// The study's samples and `made.files`, the tally of every file, and what each sample misses.
fn samples(plan: &Plan, done: &mut Done) -> Result<Joined, String> {
    let mut tally = Tally { read: 0, holes: Vec::new(), cutflow: vec![0; plan.recipe.keep.len()], files: Vec::new() };
    let (mut samples, mut made_files, mut missing) = (Vec::new(), Map::new(), Vec::new());
    let mut taken = std::mem::take(&mut done.taken).into_iter();
    for (index, sample) in plan.lock.samples.iter().enumerate() {
        let (rows, records, lacks) = joined(sample, plan, index, &mut taken, &mut tally)?;
        samples.push(write::Sample { name: sample.name.clone(), label: sample.label.clone(), rows });
        let mut metadata = sample.metadata.clone();
        metadata.insert("events_read".into(), (sample.events_read - lacks).into());
        metadata.insert("events_total".into(), sample.events_total.into());
        // A sample's totals are its files' totals summed, in their order, from 0.
        for total in &plan.recipe.totals {
            let sum = records.values().filter_map(|record| record["totals"][&total.name].as_f64()).fold(0.0, |sum, more| sum + more);
            if !sum.is_finite() {
                return Err((done.stuck)(format!("[totals] {}: its sum over sample {} is not a finite number, as its files' totals add up past what a double holds: give its events smaller values", total.name, sample.name)));
            }
            metadata.insert(total.name.clone(), sum.into());
        }
        missing.push(lacks);
        made_files.insert(sample.name.clone(), json!({"events": plan.asked[index].record(), "files": records, "metadata": metadata}));
    }
    // What each sample misses is said only in a file with holes, and then on every sample.
    if !tally.holes.is_empty() {
        for (sample, lacks) in plan.lock.samples.iter().zip(&missing) {
            made_files[&sample.name]["metadata"]["events_missing"] = (*lacks).into();
        }
    }
    Ok(Joined { samples, files: made_files, tally, missing })
}

/// Each stored column, copied from the earlier file or packed from the spool, of the Plan's kind
/// when no file kept an event of it.
fn columns<'a>(plan: &'a Plan, spool: &'a write::Spool) -> Result<Vec<(String, write::Source<'a>)>, rowver::Error> {
    let mut columns = Vec::new();
    let mut read = 0;
    for (index, ((name, _), (kind, listed))) in plan.recipe.columns.iter().zip(&plan.kinds).enumerate() {
        if let Base::Columns(earlier) = &plan.from {
            if earlier.reused[index] {
                columns.push((name.clone(), write::Source::Copied(earlier.file(), name.clone())));
                continue;
            }
        }
        let source = match spool.take(read)? {
            Some(spooled) => write::Source::Spooled(spooled),
            None => {
                let buffer = NumericBuffer { kind: kind.name().into(), count: 0, bytes: Vec::new() };
                write::Source::Values(match listed {
                    true => write::Values::Jagged { counts: Vec::new(), content: buffer },
                    false => write::Values::Flat(buffer),
                })
            }
        };
        read += 1;
        columns.push((name.clone(), source));
    }
    Ok(columns)
}

/// What failed writing the file at `path`, or the file the failure names, without what to do:
/// a press says that (`Done::stuck`).
fn what(path: &Path, error: rowver::Error) -> String {
    match error {
        e @ (rowver::Error::Io(_) | rowver::Error::File(..)) => rowver::plain_what(path, &e),
        e => e.to_string(),
    }
}

/// Where a press writes its arrays: a new file, or the file it builds on, in place (SPEC §8.12).
enum Target<'p> {
    New(write::Laid),
    InPlace(Box<rowver::change::Change<'p>>),
}

impl Target<'_> {
    /// The records of `derived` under `header`, worked out from the arrays written.
    fn derived(&self, header: &Value, derived: &[(String, String)]) -> Result<Vec<Value>, String> {
        match self {
            Target::New(laid) => laid.derived(header, derived),
            Target::InPlace(change) => change.derived(header, derived),
        }
        .map_err(|e| e.to_string())
    }
}

/// The data section of a press that only adds columns to the file `change` changes, their arrays
/// written after the file's own (SPEC §8.2, §8.12): its records in the recipe's order.
fn added(plan: &Plan, spool: &write::Spool, change: &mut rowver::change::Change) -> Result<Value, rowver::Error> {
    let mut new = Vec::new();
    let mut at = Vec::new();
    for (name, source) in columns(plan, spool)? {
        match source {
            write::Source::Copied(..) => at.push(None),
            source => {
                at.push(Some(new.len()));
                new.push((name, source));
            }
        }
    }
    let packed = change.pack(new, plan.workers)?;
    let mut data = change.header()["data"].clone();
    let old: Vec<Value> = data["cols"].as_array().cloned().unwrap_or_default();
    let cols: Vec<Value> = plan.recipe.columns.iter().zip(at).map(|((name, _), at)| match at {
        Some(at) => packed[at].clone(),
        None => old.iter().find(|c| c["name"] == name.as_str()).cloned().unwrap_or(Value::Null),
    }).collect();
    data["cols"] = cols.into();
    write::data_sealed(&mut data)?;
    Ok(data)
}

/// The entries read, or, in a debug build as `RVR_OTHER_ROWS` asks, none: so that a test sees what
/// rows unlike the file's do.
fn unlike(entry: write::Source) -> write::Source {
    #[cfg(debug_assertions)]
    if std::env::var_os("RVR_OTHER_ROWS").is_some() {
        return write::Source::Values(write::Values::Flat(write::array::<i64>(&[])));
    }
    entry
}

/// The entries of each of the files `data` lists, in order, as the press building on `earlier` read
/// them from it (each file's with its input, or left with `earlier` when it is read no more).
fn kept_entries<'p>(plan: &'p Plan, earlier: &'p crate::reuse::Earlier, data: &Value) -> Vec<&'p [u64]> {
    let inputs: std::collections::HashMap<(String, String), &[u64]> = plan.work.iter().filter_map(|input| Some(((plan.lock.samples[input.sample].name.clone(), input.file.at.clone()), input.kept.as_deref()?))).collect();
    let listed = data["files"].as_array().map_or(&[][..], Vec::as_slice);
    listed
        .iter()
        .map(|file| {
            let key = (file["sample"].as_str().unwrap_or_default().to_owned(), file["path"].as_str().unwrap_or_default().to_owned());
            inputs.get(&key).copied().or_else(|| earlier.kept.get(&key).map(Vec::as_slice)).unwrap_or_default()
        })
        .collect()
}

/// Write the file the Plan presses, from what every file gave: the number of holes it has.
pub(crate) fn write(plan: &Plan, mut done: Done) -> Result<usize, String> {
    let (target, stuck) = (&plan.target, done.stuck);
    let staged_file = plan.staging.join("pressed.rvr");
    let Joined { samples, files: made_files, tally, missing } = samples(plan, &mut done)?;
    // The entries the press read, spooled after its columns.
    let entry = |at: &Path| -> Result<write::Source, String> {
        Ok(match done.spool.take(plan.reading.len()).map_err(|e| stuck(what(at, e)))? {
            Some(spooled) => write::Source::Spooled(spooled),
            None => write::Source::Values(write::Values::Flat(write::array::<i64>(&[]))),
        })
    };
    let (mut data, into) = match &plan.from {
        Base::Columns(earlier) if earlier.in_place => {
            // The file was locked before it was read, and is held still (SPEC §8.12).
            let locked = plan.locked.as_ref().ok_or_else(|| stuck("a press in place holds no lock on its file (a bug in rvr: please report it)".into()))?;
            // Begun from the header the plan read under the lock, not read again.
            let mut change = rowver::change::Change::of(locked, earlier.file()).map_err(|e| stuck(what(target, e)))?;
            // The rows read are the file's, in order, checked before anything is written: their
            // entries against the file's, read and checked against its record as the plan was made.
            let data = &change.header()["data"];
            let read = kept_entries(plan, earlier, data);
            write::rows_agree(&samples, unlike(entry(target)?), data, &read).map_err(|e| stuck(format!("{}: the rows read are not the file's rows: {e} (a bug in rvr; the file is as it was); please report it", target.display())))?;
            let data = added(plan, done.spool, &mut change).map_err(|e| stuck(what(target, e)))?;
            (data, Target::InPlace(Box::new(change)))
        }
        _ => {
            let entry = entry(&staged_file)?;
            let rows = write::Rows { samples, files: tally.files.clone(), entry, chunk_rows: plan.recipe.chunk_rows() };
            let columns = columns(plan, done.spool).map_err(|e| stuck(what(&staged_file, e)))?;
            // One left by a crash, perhaps with the file's permissions (0444), is rvr's own, under the staging lock.
            let _ = std::fs::remove_file(&staged_file);
            let (data, laid) = write::stream(rows, columns, plan.workers, &staged_file).map_err(|e| stuck(what(&staged_file, e)))?;
            (data, Target::New(laid))
        }
    };
    let failed = |e: rowver::Error| e.to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let text = |text: &str| json!({"sha256": write::text_checksum(text), "text": text});
    let mut made = Map::new();
    made.insert("study".into(), plan.study.as_str().into());
    if let Some(title) = &plan.recipe.title {
        made.insert("title".into(), title.as_str().into());
    }
    made.insert("built".into(), voms_proxy::utc(now).into());
    made.insert("by".into(), concat!("rvr ", env!("CARGO_PKG_VERSION")).into());
    made.insert("toolchain".into(), plan.toolchain.clone());
    let mut pressed = text(&plan.texts.recipe);
    pressed["format"] = "rvr-recipe".into();
    pressed["columns"] = plan.recipe.columns.iter().map(|(n, _)| n.as_str()).collect();
    made.insert("recipe".into(), pressed);
    made.insert("samples".into(), text(&plan.texts.samples));
    made.insert("lock".into(), text(&plan.texts.lock));
    made.insert("files".into(), Value::Object(made_files));
    let mut made = Value::Object(made);
    let names: Vec<&str> = plan.lock.samples.iter().map(|s| s.name.as_str()).collect();
    let written: Vec<&str> = plan.recipe.columns.iter().map(|(n, _)| n.as_str()).collect();
    data["sample_cols"] = write::sample_cols(&names, &made["files"], &written).map_err(failed)?.into();
    made["sha256"] = write::made_checksum(&made).map_err(failed)?.into();
    let rows = data["rows"].as_u64().unwrap_or(0);
    let mut header = json!({"rowver": rowver::VERSION, "data": data, "made": made});
    let signer = plan.signer;
    // The derived columns are worked out once, from the arrays laid for the file, for their
    // records. One that fails leaves what was read: the file, without its derived columns; in
    // place, with those it held, of the same expressions.
    let mut underived = None;
    if !plan.recipe.derived.is_empty() {
        if let Err(why) = into.derived(&header, &plan.recipe.derived).and_then(|records| {
            header["derived"] = json!({ "columns": records });
            crate::derive::sealed(&mut header, None)
        }) {
            let old = match &into {
                Target::InPlace(change) => change.header()["derived"]["columns"].as_array().cloned().unwrap_or_default(),
                Target::New(_) => Vec::new(),
            };
            let had = |(name, expression): &(String, String)| old.iter().find(|r| r["name"] == name.as_str() && r["expression"] == expression.as_str()).cloned();
            let held: Vec<Value> = plan.recipe.derived.iter().filter_map(had).collect();
            let gone: Vec<String> = plan.recipe.derived.iter().map(|(name, _)| name.clone()).filter(|name| !held.iter().any(|r| r["name"] == name.as_str())).collect();
            // In place every derived column it held is the recipe's (SPEC §8.2): with none new, what
            // failed is not one of them, and the press keeps what it read.
            if matches!(into, Target::InPlace(_)) && gone.is_empty() {
                return Err(stuck(format!("{} could not be written: {why}", target.display())));
            }
            header["derived"] = json!({ "columns": held });
            crate::derive::without(&mut header, gone.clone()).map_err(|e| stuck(format!("{} could not be written: {why}; and its [derived] lines could not be taken out of the recipe it carries, to write it without them: {e}", target.display())))?;
            underived = Some((why, gone));
        }
    }
    if let Some(root) = header.as_object_mut() {
        root.extend(plan.tools.clone());
    }
    let was = match &into {
        Target::InPlace(change) => change.header().get("signature").map(|s| s["key"].as_str().unwrap_or_default().to_owned()),
        Target::New(_) => None,
    };
    let in_place = matches!(into, Target::InPlace(_));
    let unwritten = |why: String| format!("{} could not be written: {why}", target.display());
    match into {
        // Made in the file itself, under its lock and journal: put back as it was if cut short (or its header refused).
        Target::InPlace(change) => {
            crate::derive::signed(&mut header, signer.as_ref()).map_err(|why| stuck(unwritten(why)))?;
            change.finish(&header).map_err(|e| stuck(what(target, e)))?;
        }
        // A press of more than a few seconds is on disk before what would carry it on goes.
        Target::New(laid) => {
            let write = || crate::derive::sealed(&mut header, signer.as_ref()).map_err(unwritten).and_then(|()| write::write_laid(&header, laid).map_err(|e| what(&staged_file, e)));
            // Not over a file another program moved there since the press locked the one it read,
            // nor, where it holds no lock, over anything the path leads to, or anything else another
            // program put there since the press began (SPEC §8.12).
            let placed_at = std::cell::Cell::new(None);
            let ready = || {
                match &plan.locked {
                    Some(locked) => locked.there().map_err(|e| e.to_string())?,
                    None if std::fs::metadata(target).is_ok() || crate::plan::standing(target).is_some_and(|now| plan.found != Some(now)) => return Err(format!("{} was put there by another program while rvr pressed it, and is left as it is: move it away", target.display())),
                    None => {}
                }
                placed_at.set(done.journal.placing(&staged_file));
                Ok(())
            };
            // A file it replaces may have taken hours to press: the new one is on disk before it goes.
            let sync = done.worth || plan.locked.is_some();
            let put = crate::files::in_place(&staged_file, target, sync, write, ready, |at, e| what(at, e.into()));
            if let (Err(_), Some(at)) = (&put, placed_at.get()) {
                done.journal.unplaced(at);
            }
            put.map_err(stuck)?;
            if sync {
                let _ = std::fs::File::open(crate::journal::parent(target)).and_then(|f| f.sync_all());
            }
        }
    }
    match underived {
        Some((why, gone)) if in_place => return Err(format!("{why}. {} is written without the new derived columns {}, keeping those it held and what was read: put right what is said above and press again, which adds them in place and reads no input.", target.display(), gone.join(", "))),
        Some((why, _)) => return Err(format!("{why}. {} is written without its derived columns, so that what was read is kept: put right what is said above and press again, which, by the same rvr, copies every stored column from it and reads no input.", target.display())),
        None => {}
    }
    // In place, whose signature it replaces is said, when it had one or has one now.
    let signed = (in_place && (signer.is_some() || was.is_some())).then(|| crate::key::resigned(signer.as_ref(), was.as_deref()));
    said(plan, &tally, &missing, signer.as_ref(), (rows, in_place, signed), done.started);
    Ok(tally.holes.len())
}

/// What a press that wrote its file says: the cutflow, the key it signed with, the file (in
/// place, with whose signature it replaced), its holes.
fn said(plan: &Plan, tally: &Tally, missing: &[u64], signer: Option<&[u8; 32]>, (rows, in_place, signed): (u64, bool, Option<String>), started: std::time::Instant) {
    let target = &plan.target;
    let added = match &plan.from {
        Base::Samples(fill) => fill.added.as_deref(),
        _ => None,
    };
    // Building on a file of other samples, what was read is the new samples'.
    let read_now = |name: &String| added.is_none_or(|added| added.contains(name));
    let built_on = matches!(plan.from, Base::Columns(_));
    if !built_on && added.is_none_or(|added| !added.is_empty()) {
        let lacks: u64 = plan.lock.samples.iter().zip(missing).filter(|(s, _)| read_now(&s.name)).map(|(_, m)| m).sum();
        let entries = |n: u64| some(n as usize, "entry", "entries");
        let read = match lacks {
            _ if matches!(plan.from, Base::Holes(_)) => format!("{} in the holes", entries(tally.read)),
            0 => entries(tally.read),
            lacks => format!("{} of {}", count(tally.read), entries(tally.read + lacks)),
        };
        let read = match added {
            Some(added) => format!("{read} of the new {}", crate::core::named(added)),
            None => read,
        };
        match plan.recipe.keep.is_empty() {
            true => println!("Read {read}; with no cuts, every one is kept."),
            false => println!("Read {read}; kept after each cut:"),
        }
        for (cut, kept) in plan.recipe.keep.iter().zip(&tally.cutflow) {
            println!("  {:30} {:>13}", cut.name, count(*kept));
        }
    }
    match (&signed, signer) {
        (Some(signed), _) => println!("{}{}.", signed[..1].to_uppercase(), &signed[1..]),
        (None, Some(secret)) => println!("Signed with your key {}", crate::key::short(&rowver::sign::public_key(secret))),
        (None, None) => {}
    }
    let with = match tally.holes.len() {
        0 => ".".to_string(),
        n => format!(", with {}:", some(n, "hole", "holes")),
    };
    let derived = match plan.recipe.derived.len() {
        0 => String::new(),
        n => format!(" and {n} derived"),
    };
    if let Some(why) = &plan.anew {
        crate::told_anew(target, why, false);
    }
    let bytes = std::fs::metadata(target).map_or(0, |m| m.len());
    println!(
        "Wrote {}{} ({}, {}{derived}, {:.1} MB) in {:.1} s{with}",
        target.display(),
        if in_place { " in place" } else { "" },
        some(rows as usize, "row", "rows"),
        some(plan.recipe.columns.len(), "column", "columns"),
        bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    for (sample, hole) in &tally.holes {
        let kind = hole["kind"].as_str().unwrap_or("");
        println!("  {kind:11} {sample}  {}", hole["why"].as_str().unwrap_or(""));
    }
    if !tally.holes.is_empty() {
        println!("rvr press --fill reads them again once they can be read.");
    }
}
