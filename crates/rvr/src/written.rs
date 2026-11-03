//! The last of a press: every file read, the rows joined in the lock's order, the file's header
//! made, its columns packed from the spool (or copied from the earlier file), its derived columns
//! worked out, signed, and put in place whole (docs/SPEC.md §7.5.1, §7.7.3, §7.8).

use crate::plan::{Base, Plan};
use crate::press::count;
use crate::samples::some;
use crate::take::Taken;
use rowver::study::lock::Sample;
use rowver::write;
use rowver::NumericBuffer;
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

/// Write the file the Plan presses, from what every file gave: the number of holes it has.
pub(crate) fn write(plan: &Plan, mut done: Done) -> Result<usize, String> {
    let (target, stuck) = (&plan.target, done.stuck);
    let staged_file = plan.staging.join("pressed.rvr");
    let Joined { samples, files: made_files, tally, missing } = samples(plan, &mut done)?;
    let entry = match done.spool.take(plan.reading.len()).map_err(|e| stuck(e.to_string()))? {
        Some(spooled) => write::Source::Spooled(spooled),
        None => write::Source::Values(write::Values::Flat(write::array::<i64>(&[]))),
    };
    let rows = write::Rows { samples, files: tally.files.clone(), entry };
    let columns = columns(plan, done.spool).map_err(|e| stuck(e.to_string()))?;
    let (mut data, laid) = write::stream(rows, columns, plan.workers, &staged_file).map_err(|e| stuck(format!("{} could not be written: {e}", target.display())))?;
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
    let signer = match plan.sign {
        true => crate::key::secret().map_err(stuck)?,
        false => None,
    };
    // The derived columns are worked out once, from the arrays laid for the file, for their
    // records. One that fails leaves what was read: the file, without its derived columns.
    let mut underived = None;
    if !plan.recipe.derived.is_empty() {
        if let Err(why) = crate::derive::attach(&laid, &plan.recipe.derived, &mut header) {
            crate::derive::without(&mut header, plan.recipe.derived.iter().map(|(name, _)| name.clone()).collect()).map_err(|e| stuck(format!("{} could not be written: {why}; and its [derived] lines could not be taken out of the recipe it carries, to write it without them: {e}", target.display())))?;
            underived = Some(why);
        }
    }
    // A press of more than a few seconds is on disk before what would carry it on goes.
    let durable = |path: &Path| match done.worth {
        true => std::fs::File::open(path).and_then(|f| f.sync_all()),
        false => Ok(()),
    };
    let put = crate::derive::sealed(&mut header, signer.as_ref())
        .and_then(|()| write::write_laid(&staged_file, &header, laid).map_err(|e| e.to_string()))
        .and_then(|()| durable(&staged_file).map_err(|e| e.to_string()))
        .and_then(|()| std::fs::rename(&staged_file, target).map_err(|e| e.to_string()));
    if let Err(e) = put {
        let _ = std::fs::remove_file(&staged_file);
        return Err(stuck(format!("{} could not be written: {e}", target.display())));
    }
    if done.worth {
        let _ = std::fs::File::open(crate::journal::parent(target)).and_then(|f| f.sync_all());
    }
    if let Some(why) = underived {
        return Err(format!("{why}. {} is written without its derived columns, so that what was read is kept: put right what is said above and press again, which, by the same rvr, copies every stored column from it and reads no input.", target.display()));
    }
    said(plan, &tally, &missing, signer.as_ref(), rows, done.started);
    Ok(tally.holes.len())
}

/// What a press that wrote its file says: the cutflow, the key it signed with, the file, its holes.
fn said(plan: &Plan, tally: &Tally, missing: &[u64], signer: Option<&[u8; 32]>, rows: u64, started: std::time::Instant) {
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
    if let Some(secret) = signer {
        println!("Signed with your key {}", crate::key::short(&rowver::sign::public_key(secret)));
    }
    let with = match tally.holes.len() {
        0 => ".".to_string(),
        n => format!(", with {}:", some(n, "hole", "holes")),
    };
    let derived = match plan.recipe.derived.len() {
        0 => String::new(),
        n => format!(" and {n} derived"),
    };
    let bytes = std::fs::metadata(target).map_or(0, |m| m.len());
    println!(
        "Wrote {} ({}, {}{derived}, {:.1} MB) in {:.1} s{with}",
        target.display(),
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
