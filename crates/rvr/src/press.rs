//! `rvr press`: the options read, the study's texts read, the Plan built from them by the
//! agreement check, and the pressing core run on it (docs/SPEC.md §2, §7, §8). The core is
//! `crate::core`; reading a file is `crate::take`.

use crate::options::Options;
use crate::plan::{Asked, Base, Choose, On, Place};
use crate::samples::{some, Choice};
use serde_json::{json, Value};
use std::path::PathBuf;

/// How `rvr press` is used.
pub const USAGE: &str = "usage:
  rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N] [--full]
            [--fill [KINDS]] [--restart]
                                           press DIR/samples.lock through DIR/recipe.toml into
                                           DIR/<DIR's name>.rvr (DIR is . when not given); when
                                           only stored columns changed since that file was
                                           pressed, read just those, and when only samples were
                                           added, just those (--full reads everything);
                                           --fill reads only the file's holes (of KINDS, such as
                                           refused,unreachable) and writes it again with them; a
                                           press that stopped part way is picked up where it
                                           stopped (--restart starts it over)";

/// What `rvr press` was asked, its options read.
struct Pressing {
    dir: Option<PathBuf>,
    out: Option<PathBuf>,
    choice: Choice,
    workers: usize,
    on: On,
    restart: bool,
}

/// `rvr press` with its options: the exit status, 3 when the file at the target has holes.
pub fn main(options: &Options) -> u8 {
    let pressing = match read(options) {
        Ok(pressing) => pressing,
        Err(why) => return crate::wrong("press", &why),
    };
    crate::report("press", run(pressing))
}

/// The options of a press, or how they are wrong.
fn read(options: &Options) -> Result<Pressing, String> {
    let choice = Choice::read(options)?;
    let workers = crate::options::workers(options)?;
    let dir = options.words.first().map(PathBuf::from);
    let fill = options.has("--fill").then(|| crate::fill::kinds(options.value("--fill")));
    let on = match (fill, options.has("--full")) {
        (Some(_), true) => return Err("--fill reads only a file's holes, --full every entry: give one".into()),
        (Some(kinds), false) => On::Holes(kinds),
        (None, true) => On::Full,
        (None, false) => On::Earlier,
    };
    let out = options.value("--out").map(PathBuf::from);
    Ok(Pressing { dir, out, choice, workers, on, restart: options.has("--restart") })
}

/// Press as asked: the exit status, 3 when the file at the target has holes.
fn run(pressing: Pressing) -> Result<u8, String> {
    let (dir, study, target) = crate::texts::study(pressing.dir, pressing.out)?;
    crate::plan::a_file_or_none(&target)?;
    // Through a link, the file it names is pressed, staged beside it and moved over it.
    let target = rowver::change::resolved(&target);
    let mut texts = crate::texts::read(&dir)?;
    // Options that do not fit the samples are refused before recipe.toml is offered tables.
    if crate::at_terminal() {
        if let Ok(lock) = rowver::study::lock::Lock::read(&texts.lock) {
            let (_, _, said) = crate::agree::choices(lock, &Choose::Options(pressing.choice.clone()));
            if !said.is_empty() {
                return Err(said.join("\n"));
            }
        }
    }
    // A fill takes the recipe the file was pressed from, as it was: it is not offered tables.
    if !matches!(pressing.on, On::Holes(_)) && crate::tables::offer(&dir, &texts.recipe, "press", "continue pressing")? {
        texts = crate::texts::read(&dir)?;
    }
    // Held from its first read of the file there until its new file is in place (SPEC §8.12), and
    // read through the descriptor that holds the lock; nothing there, or a link to nothing, has none.
    // The file's lock is taken before the staging folder's, as every writer takes them.
    // Checked again after the questions, what stands there is taken first: whatever comes after
    // is another program's (SPEC §8.12).
    crate::plan::a_file_or_none(&target)?;
    let found = crate::plan::standing(&target);
    let locked = match std::fs::metadata(&target) {
        Ok(there) if there.is_file() => Some(target_locked(&target)?),
        _ => None,
    };
    crate::hold();
    // Refused before its staging folder is made beside the file.
    let unwritable = || locked.as_ref().map_or_else(|| rowver::change::probe(&target).0, rowver::change::Locked::unwritable);
    if let Some(why) = pressing.restart.then(|| crate::plan::read_only(&target, unwritable())).flatten() {
        return Err(why);
    }
    crate::journal::tidy(&target, None);
    if pressing.restart {
        let stopped = crate::journal::Staged::lock(&target)?;
        if stopped.keep.get() {
            println!("Starting over: removed the press that stopped in {}.", stopped.folder.display());
        }
        stopped.keep.set(false);
    }
    let at = locked.as_ref().map_or_else(|| crate::plan::At::open(&target), crate::plan::At::held);
    // The entries the file there read, when it opens: a press that reads fewer asks first.
    let held = match &at {
        crate::plan::At::Opened(file, Ok(())) => Some(crate::entries_read(&file.header()["made"]).0),
        _ => None,
    };
    let place = Place { base: dir, study, at, target: target.clone(), locked, found };
    let asked = Asked { choose: Choose::Options(pressing.choice), on: pressing.on, workers: pressing.workers, sign: true };
    let plan = crate::plan::build(place, texts, asked)?;
    if let Some(why) = &plan.refused {
        return Err(why.clone());
    }
    match (&plan.why, plan.other_build) {
        // Only another build made it: pressing every column again is asked first (SPEC §8.6).
        (Some(both), true) => {
            println!("{both}\nA file is built on only by the build that made it, so pressing with this build presses every column again.");
            if !crate::at_terminal() {
                eprintln!("rvr press: nothing was pressed: no one is at a terminal to answer whether to press every column. Give --full to press every column without asking.");
                return Ok(2);
            }
            if !crate::yes("Press every column? [y/N] ")? {
                return Err(format!("{} is as it was: nothing was pressed.", target.display()));
            }
        }
        (Some(why), false) => {
            println!("Pressing every column: {} cannot be built on, as {why}.", target.display());
            // A file of fewer events in place of a fuller one is asked first (SPEC §8.5).
            let read: u64 = plan.lock.samples.iter().map(|s| s.events_read).sum();
            if let Some(held) = held.filter(|held| *held > read) {
                let (held, read) = (rowver::rvr::grouped(held), rowver::rvr::grouped(read));
                println!("It read {held} entries, and this press reads {read}: the file it writes takes its place.");
                if !crate::at_terminal() {
                    eprintln!("rvr press: nothing was pressed: no one is at a terminal to answer whether to replace a file of more events. Give --full to replace it without asking, or --out FILE to press into another file.");
                    return Ok(2);
                }
                if !crate::yes("Replace it? [y/N] ")? {
                    return Err(format!("{} is as it was: nothing was pressed.", target.display()));
                }
            }
        }
        (None, _) => {}
    }
    let holes = match plan.from {
        Base::Current(0) => {
            println!("{} is current: nothing to press.", target.display());
            crate::journal::tidy(&target, Some(&plan.identity));
            0
        }
        Base::Current(holes) => {
            println!("{} holds this study as it is, with {}: rvr press --fill reads them again (rvr press --full presses everything again).", target.display(), some(holes, "hole", "holes"));
            holes
        }
        _ => {
            for note in crate::agree::notes(crate::agree::expressions(&plan.recipe).iter(), &plan.recipe, &plan.lock) {
                eprintln!("Note: recipe.toml: {note}");
            }
            crate::core::press(plan)?
        }
    };
    Ok(if holes == 0 { 0 } else { 3 })
}

/// The press's target locked: one it may not open stops the press, for what it holds is carried over.
fn target_locked(target: &std::path::Path) -> Result<rowver::change::Locked, String> {
    let locked = rowver::change::Locked::take(target).map_err(|e| match &e {
        rowver::Error::Io(io) | rowver::Error::File(_, io) if io.kind() == std::io::ErrorKind::PermissionDenied => {
            let shown = target.display();
            format!("{}, and a press carries over what it holds (its tool keys), so it stops: chmod u+rw {shown}, or remove it", rowver::plain_what(target, &e))
        }
        _ => rowver::plain(target, &e),
    })?;
    crate::told(target, locked.restored());
    Ok(locked)
}

/// What built this rvr, as `made.toolchain` records it.
pub fn toolchain() -> Value {
    json!({
        "program": "rvr",
        "version": env!("CARGO_PKG_VERSION"),
        "rowver": rowver::VERSION.to_string(),
        "commit": env!("RVR_COMMIT"),
        "compiler": env!("RVR_RUSTC"),
        "target": env!("RVR_TARGET"),
    })
}
