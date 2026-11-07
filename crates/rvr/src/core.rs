//! The pressing core (docs/SPEC.md §2.5, §7): it takes only a Plan, reads the entries the Plan
//! names with a few workers, and writes the file. It checks nothing the validators or the
//! agreement check checked: it fails only on the data streaming in.

use crate::holes::{passing, Failed, Fault};
use crate::journal::Staged;
use crate::open::{connect, Reach};
use crate::plan::{Base, Input, Plan};
use rowver::rvr::grouped as count;
use crate::samples::remote::{second, Watch, AGAIN, QUIET};
use crate::samples::some;
use crate::take::{chosen_of, missed, ranges, sentence, take, Shared, Taken};
use rowver::rvr::derived::fields;
use rowver::study::lock::File;
use rowver::write::Spool;
use rvr_recipe::Constant;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Press what `plan` says: the number of holes the file written has.
pub fn press(plan: Plan) -> Result<usize, String> {
    let started = std::time::Instant::now();
    let staged = Staged::lock(&plan.target)?;
    // A press is carried on unless it is a reproduction, or its build names no one build.
    let mut picked = plan.carries.then(|| crate::journal::pick_up(&staged, &plan.identity)).flatten();
    let resumed = picked.as_mut().map(|p| std::mem::take(&mut p.taken)).unwrap_or_default();
    let picked_up = !resumed.is_empty();
    let watch = Arc::new(Watch::press(second() * QUIET, second() * AGAIN));
    let files: Vec<(usize, &File)> = plan.reads().iter().enumerate().filter(|(at, _)| !resumed.contains_key(at)).map(|(_, i)| (i.sample, &i.file)).collect();
    let (storage, offline) = connect(&files, &watch);
    let (spool, journal) = crate::journal::stage(&plan.target, &staged, &plan.identity, picked)?;
    announce(&plan);
    // The bars: each sample's entries chosen of what this press reads, and what it picked up.
    let samples = plan.lock.samples.len();
    let (mut chosen, mut already, mut holed) = (vec![0; samples], vec![0; samples], 0);
    for (at, input) in plan.reads().iter().enumerate() {
        chosen[input.sample] += chosen_of(input);
        if let Some(taken) = resumed.get(&at) {
            already[input.sample] += taken.entries + taken.missing;
            holed += taken.record["holes"].as_array().filter(|_| input.kept.is_none()).map_or(0, Vec::len);
        }
    }
    let names = plan.lock.samples.iter().map(|s| s.name.clone());
    let bars = names.zip(chosen).zip(already).map(|((name, chosen), done)| (name, chosen, done)).collect();
    let display = crate::progress::Display::start(bars, resumed.values().map(|t| t.rows as u64).sum(), holed);
    let reach = Reach { storage: storage.as_ref(), offline: &offline, dir: &plan.base, watch, whole: true, read: display.counts.bytes.clone() };
    let shared = Shared {
        recipe: &plan.reader,
        outputs: plan.outputs(),
        derived: &plan.derived,
        reach,
        mentions: &plan.mentions,
        checked: &plan.checked,
        spool: &spool,
        journal: &journal,
        carries: plan.carries,
        counts: &display.counts,
        tried: Mutex::new(BTreeSet::new()),
        #[cfg(debug_assertions)]
        stop_after: std::env::var("RVR_STOP_AFTER").ok().and_then(|n| n.parse().ok()),
    };
    // What was read made durable, and the folder kept when that holds any file.
    let save = || {
        let saved = plan.carries.then(|| journal.checkpoint(&spool));
        staged.keep.set(!matches!(saved, None | Some(Ok(0))));
        saved
    };
    let result = take_all(&shared, &plan, resumed);
    drop(shared);
    drop(display);
    let read = match result {
        Ok(read) => read,
        // A file that will not be read as it is, building on an earlier file (whose remedy is
        // another press), is pressed again from the start; anything else keeps what was read.
        Err(failed) if failed.fault.kind().is_some_and(|k| !passing(k)) => return Err(failed.why),
        Err(failed) => return Err(ended(failed, &plan.target, &save(), plan.reads().len())),
    };
    // What the last files gave is made durable before the file is written, when that is worth a
    // sync: a press of a few seconds is read again soon enough.
    let worth = picked_up || started.elapsed() >= crate::journal::EVERY;
    if plan.carries && worth {
        let _ = journal.checkpoint(&spool);
    }
    // Every file is read: a failure from here on (the disk, the key, the spool, a sum) keeps what
    // was read.
    let stuck = |why: String| format!("{} Mend it, then {}.", sentence(&why), again(&save(), plan.reads().len()));
    let taken = match &plan.from {
        // A fill that read none of its holes again, each failing as before, leaves the file.
        Base::Holes(fill) if read.iter().map(|t| t.missing).sum::<u64>() == plan.reads().iter().map(chosen_of).sum::<u64>() && crate::fill::same_kinds(fill, &plan.lock, plan.reads(), &read) => {
            let asked = plan.reads().iter().flat_map(|i| i.ranges.iter().flatten()).count();
            println!("None of the {} could be read: {} is as it was.", some(asked, "hole", "holes"), plan.target.display());
            return Ok(rowver::rvr::holes(fill.file.header()).len());
        }
        Base::Holes(fill) | Base::Samples(fill) => {
            let names: Vec<&str> = plan.outputs().iter().map(|((name, _), _)| name.as_str()).collect();
            crate::fill::spool_old(&spool, fill, &names, &plan.laid, plan.workers).map_err(&stuck)?;
            crate::fill::merged(&plan.work, &plan.lock, fill, &plan.laid, read, &plan.recipe)?
        }
        Base::Columns(_) if plan.reads().is_empty() => copied(&plan, &spool).map_err(&stuck)?,
        _ => read,
    };
    let done = crate::written::Done { taken, spool: &spool, started, worth, stuck: &stuck, journal: &journal };
    let holes = crate::written::write(&plan, done)?;
    drop((journal, spool));
    drop(staged);
    Ok(holes)
}

/// What a press of every column reads: `S samples, F files, E entries`.
pub(crate) fn pressed(plan: &Plan) -> String {
    let read: u64 = plan.lock.samples.iter().map(|s| s.events_read).sum();
    format!("{}, {}, {}", some(plan.lock.samples.len(), "sample", "samples"), some(plan.reads().len(), "file", "files"), some(read as usize, "entry", "entries"))
}

/// What the press says it presses, before it reads.
fn announce(plan: &Plan) {
    let (target, reads, samples) = (plan.target.display(), plan.reads(), plan.lock.samples.len());
    let at_a_time = plan.workers.min(reads.len()).max(1);
    let entries = reads.iter().map(chosen_of).sum::<u64>();
    match &plan.from {
        Base::Samples(fill) => match fill.added.as_deref().unwrap_or_default() {
            [] => println!("{target} holds the rows of every sample as samples.lock has them.\nNothing to read: every row is copied from it."),
            added => println!(
                "{target} holds the rows of {} of the {} as samples.lock has them.\nReading only the new {}, {}, {}, {at_a_time} at a time:",
                samples - added.len(),
                some(samples, "sample", "samples"),
                named(added),
                some(reads.len(), "file", "files"),
                some(entries as usize, "entry", "entries"),
            ),
        },
        Base::Holes(_) => {
            let holes = reads.iter().flat_map(|i| i.ranges.iter().flatten()).count();
            println!("Filling {target}: reading {} of {}, {}, {at_a_time} at a time; every other row is the file's own:", some(holes, "hole", "holes"), some(reads.len(), "file", "files"), some(entries as usize, "entry", "entries"));
        }
        Base::Columns(earlier) => {
            let kept: usize = plan.work.iter().map(|i| i.kept.as_ref().map_or(0, Vec::len)).sum();
            let names: Vec<&str> = plan.reading.iter().map(|at| plan.recipe.columns[*at].0.as_str()).collect();
            let columns = plan.recipe.columns.len();
            println!("{target} has the events this recipe keeps ({}) and {} of its {columns} columns.", count(kept as u64), columns - names.len());
            match (names.is_empty(), earlier.in_place) {
                (true, true) => println!("Nothing to read: the new derived columns are added to it in place."),
                (true, false) => println!("Nothing to read: every column is copied from it."),
                (false, _) => println!("Reading only {} for those events, {}, {at_a_time} at a time:", names.join(", "), some(reads.len(), "file", "files")),
            }
        }
        Base::Nothing | Base::Current(_) => println!("Pressing {}, {at_a_time} at a time:", pressed(plan)),
    }
}

/// The samples a press of other samples than the file's reads: "sample c", "samples c, d".
pub(crate) fn named(added: &[String]) -> String {
    format!("{} {}", if added.len() == 1 { "sample" } else { "samples" }, added.join(", "))
}

/// Building on columns with none to read, what each file gives: the earlier file's rows of it and
/// its record, its entries put in the spool.
fn copied(plan: &Plan, spool: &Spool) -> Result<Vec<Taken>, String> {
    let mut taken = Vec::new();
    for input in &plan.work {
        let record = input.before.clone().unwrap_or(Value::Null);
        let missing = missed(input, &ranges(&record));
        let mut entry: Vec<i64> = input.kept.iter().flatten().map(|e| *e as i64).collect();
        let rows = entry.len();
        if rows > 0 {
            crate::gather::spooled(spool, &mut [], &mut entry, (input.order, 0))?;
        }
        taken.push(Taken { rows, record, entries: chosen_of(input) - missing, missing, cutflow: vec![0; plan.recipe.keep.len()] });
    }
    Ok(taken)
}

/// Read every file the Plan reads with a few workers, but those an earlier press of this one
/// finished (`picked`, by place), noting each in the journal; what each gave, in the lock's order.
fn take_all(shared: &Shared, plan: &Plan, picked: BTreeMap<usize, Taken>) -> Result<Vec<Taken>, Failed> {
    let work = plan.reads();
    let fields: Vec<BTreeMap<String, Constant>> = plan.lock.samples.iter().map(|s| fields(&s.metadata)).collect();
    let next = AtomicUsize::new(0);
    let watch = &shared.reach.watch;
    let todo: Vec<usize> = (0..work.len()).filter(|at| !picked.contains_key(at)).collect();
    let finished = AtomicUsize::new(picked.len());
    let mut done: Vec<Option<Result<Taken, Failed>>> = work.iter().map(|_| None).collect();
    for (at, taken) in picked {
        done[at] = Some(Ok(taken));
    }
    let done = Mutex::new(done);
    let (ticking, ended) = std::sync::mpsc::channel::<()>();
    let crashed = std::thread::scope(|scope| {
        let keeper = scope.spawn(move || if shared.carries { shared.journal.keep_up(shared.spool, crate::journal::EVERY, ended) });
        let readers: Vec<_> = (0..plan.workers.min(todo.len()).max(1))
            .map(|_| {
                scope.spawn(|| loop {
                    let Some(&at) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    if watch.stop.load(Ordering::Acquire) {
                        break;
                    }
                    let input = &work[at];
                    let mut progressed = 0;
                    shared.counts.begin(input.sample);
                    let taken = take(shared, &fields[input.sample], input, &mut progressed);
                    match &taken {
                        Ok(taken) => finished_one(shared, input, at, taken, progressed, work.len(), &finished),
                        Err(_) => watch.stop.store(true, Ordering::Release),
                    }
                    done.lock().unwrap_or_else(|e| e.into_inner())[at] = Some(taken);
                })
            })
            .collect();
        // Joined by hand: a scope left with a panicked thread would panic in turn.
        let crashed = readers.into_iter().fold(false, |crashed, reader| reader.join().is_err() || crashed);
        drop(ticking);
        keeper.join().is_err() || crashed
    });
    if crashed {
        return Err(Failed::new(Fault::Bug, "a file reader stopped unexpectedly"));
    }
    let done = done.into_inner().unwrap_or_else(|e| e.into_inner());
    let errors = || done.iter().flatten().filter_map(|t| t.as_ref().err());
    // A value the recipe cannot take comes first; then the network gone, which is no file's fault.
    if let Some(data) = errors().find(|e| e.fault == Fault::Data) {
        return Err(data.clone());
    }
    if let Some(why) = watch.gone() {
        return Err(Failed::new(Fault::Save, why));
    }
    if let Some(error) = errors().find(|e| e.fault != Fault::Stopped).or_else(|| errors().next()) {
        return Err(error.clone());
    }
    done.into_iter()
        .collect::<Option<Result<Vec<_>, _>>>()
        .ok_or_else(|| Failed::new(Fault::Bug, "a file was left unread"))?
}

/// A file read: told to the bars, noted in the journal unless a hole of it may pass, and printed.
fn finished_one(shared: &Shared, input: &Input, at: usize, taken: &Taken, progressed: u64, files: usize, finished: &AtomicUsize) {
    // What its runs did not tell the bars: a hole, a file not read.
    let rest = (taken.entries + taken.missing).saturating_sub(progressed);
    shared.counts.ran(input.sample, rest, 0);
    // Building on an earlier file, its holes are carried, not found again.
    let holes = taken.record["holes"].as_array().filter(|_| input.kept.is_none());
    let holes: &[Value] = holes.map_or(&[], Vec::as_slice);
    // A file holed for a passing reason is read again by a press that carries on.
    if shared.carries && !holes.iter().any(|hole| passing(hole["kind"].as_str().unwrap_or(""))) {
        let orders: Vec<usize> = match &input.ranges {
            Some(ranges) => ranges.iter().map(|(_, _, order)| *order).collect(),
            None => vec![input.order],
        };
        shared.journal.finished(&crate::journal::line(at, taken, shared.spool.held(&orders)));
    }
    let n = finished.fetch_add(1, Ordering::Relaxed) + 1;
    let name = input.file.at.rsplit('/').next().unwrap_or(&input.file.at);
    crate::progress::file(shared.counts, &format!(
        "  {n:>w$}/{files}  {:>11} {:7}  {:>9} kept   {name}",
        count(taken.entries),
        if taken.entries == 1 { "entry" } else { "entries" },
        count(taken.rows as u64),
        w = files.to_string().len()
    ));
    for hole in holes {
        shared.counts.holed();
        let (kind, why) = (hole["kind"].as_str().unwrap_or(""), hole["why"].as_str().unwrap_or(""));
        crate::progress::say(&format!("    hole ({kind}): {why}"));
    }
    #[cfg(debug_assertions)]
    if shared.stop_after == Some(n) {
        crate::progress::say(&match shared.journal.checkpoint(shared.spool) {
            Ok(n) => format!("Stopped with {n} files done, as RVR_STOP_AFTER asks."),
            Err(why) => format!("Stopped as RVR_STOP_AFTER asks, but {why}."),
        });
        std::process::exit(1);
    }
}

/// What pressing again does after a press stopped: carry on from the files `saved` of `files`.
pub(crate) fn again(saved: &Option<Result<usize, String>>, files: usize) -> String {
    match saved {
        None | Some(Ok(0)) => "press again".into(),
        Some(Ok(done)) => format!("press again, which carries on from the {done} of {} done (rvr press --restart starts over)", some(files, "file", "files")),
        Some(Err(lost)) => format!("press again, which carries on from what this press saved before, or starts over if it saved nothing ({lost}; rvr press --restart starts over)"),
    }
}

/// What a press that stopped part way says: why, and what pressing again does.
fn ended(failed: Failed, target: &Path, saved: &Option<Result<usize, String>>, files: usize) -> String {
    let why = sentence(&failed.why);
    let then = match failed.fault {
        Fault::Save => format!("{why} It is not a file's fault, so no file was made a hole of it: fix it, then {}.", again(saved, files)),
        // Pressing again would meet it again: it is reported, not pressed past.
        Fault::Bug => format!("{why} This is a bug in rvr: please report it, with this message and the command you ran. What was read is kept."),
        Fault::Data => match saved {
            Some(Ok(done)) if *done > 0 => format!("{why} The recipe cannot take what this file holds: a press of the same study carries on from the {done} of {} done, and stops here again; mend the recipe (or the file), and the press starts over.", some(files, "file", "files")),
            _ => format!("{why} The recipe cannot take what this file holds: mend the recipe (or the file), then press again."),
        },
        _ => format!("{why} Once it can be read, {}.", again(saved, files)),
    };
    format!("{then} {} was not written.", target.display())
}
