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
    crate::report("press", run(pressing).map(|holes| if holes == 0 { 0 } else { 3 }))
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

/// Press as asked: how many holes the file at the target has.
fn run(pressing: Pressing) -> Result<usize, String> {
    let (dir, study, target) = crate::texts::study(pressing.dir, pressing.out)?;
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
    if pressing.restart {
        let stopped = crate::journal::Staged::lock(&target)?;
        if stopped.keep.get() {
            println!("Starting over: removed the press that stopped in {}.", stopped.folder.display());
        }
        stopped.keep.set(false);
    }
    let place = Place { base: dir, study, at: crate::plan::At::open(&target), target: target.clone() };
    let asked = Asked { choose: Choose::Options(pressing.choice), on: pressing.on, workers: pressing.workers, sign: true };
    let plan = crate::plan::build(place, texts, asked)?;
    if let Some(why) = &plan.why {
        println!("Pressing every column: {} cannot be built on, as {why}.", target.display());
    }
    match plan.from {
        Base::Current(0) => {
            println!("{} is current: nothing to press.", target.display());
            Ok(0)
        }
        Base::Current(holes) => {
            println!("{} holds this study as it is, with {}: rvr press --fill reads them again (rvr press --full presses everything again).", target.display(), some(holes, "hole", "holes"));
            Ok(holes)
        }
        _ => crate::core::press(plan),
    }
}

/// What built this rvr, as `made.toolchain` records it.
pub fn toolchain() -> Value {
    json!({
        "rvr": env!("CARGO_PKG_VERSION"),
        "rowver": rowver::VERSION.to_string(),
        "commit": env!("RVR_COMMIT"),
        "rustc": env!("RVR_RUSTC"),
        "target": env!("RVR_TARGET"),
    })
}
