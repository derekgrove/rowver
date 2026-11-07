//! `rvr reproduce`: write a file's study back out of it, press it again, and compare the two.

use crate::options::Options;
use crate::plan::{Asked, At, Choose, On, Place};
use crate::texts::Texts;
use rowver::study::lock::Lock;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage:
  rvr reproduce FILE [DIR] [--study FOLDER] [--workers N] [--texts]
                                           write FILE's recipe.toml, samples.toml and samples.lock
                                           into DIR (FILE's name + .reproduced when not given),
                                           press them again, and rvr diff the two files; a
                                           relative path in the lock is read from FOLDER
                                           (FILE's folder when not given); --texts writes the
                                           three texts and presses nothing";

/// The study texts a checked header carries, and its study's name.
pub(crate) fn study_texts(header: &serde_json::Value) -> (Texts, String) {
    let made = &header["made"];
    let text = |name: &str| made[name]["text"].as_str().unwrap_or_default().to_owned();
    let texts = Texts { recipe: text("recipe"), samples: text("samples"), lock: text("lock") };
    (texts, made["study"].as_str().unwrap_or_default().to_owned())
}

/// The folder a file's lock (`lock`, its text) has its relative paths from: `study`, or else the
/// folder FILE is in, or, through a link, the folder of the file it names when only that one holds
/// every relative input (press keeps a link in its study to a file on another disk).
pub(crate) fn base(file: &Path, study: Option<&Path>, lock: &str) -> Result<PathBuf, String> {
    if let Some(folder) = study {
        return Ok(folder.to_path_buf());
    }
    let own = crate::journal::parent(file).canonicalize().map_err(|e| rowver::plain_io(file, &e))?;
    let named = file.canonicalize().ok().and_then(|real| real.parent().map(Path::to_path_buf));
    let files = Lock::read(lock).map(|lock| lock.samples.into_iter().flat_map(|s| s.files).map(|f| f.at).collect()).unwrap_or_else(|_| Vec::new());
    let relative: Vec<String> = files.into_iter().filter(|at| !at.contains("://") && Path::new(at).is_relative() && !crate::samples::resolve::in_home(at)).collect();
    let holds = |folder: &Path| relative.iter().all(|at| folder.join(at).exists());
    Ok(match named {
        Some(named) if named != own && !holds(&own) && holds(&named) => named,
        _ => own,
    })
}

/// Reproduce `file` into `dir`, reading relative inputs from `study` (FILE's folder when `None`);
/// with `texts_only`, only write its texts there.
fn run(file: &Path, dir: Option<PathBuf>, study: Option<PathBuf>, workers: usize, texts_only: bool) -> Result<u8, String> {
    let shown = file.display();
    let reader = rowver::rvr::Reader::open(file).map_err(|e| rowver::plain(file, &e))?;
    let made = &reader.header()["made"];
    let (texts, name) = study_texts(reader.header());
    let name = name.as_str();
    let base = base(file, study.as_deref(), &texts.lock)?;
    let lock = Lock::read(&texts.lock).map_err(|p| format!("{shown}: its samples.lock does not check out: {p}"))?;
    let local = lock.samples.iter().flat_map(|s| &s.files);
    let local = local
        .filter(|f| !f.at.contains("://"))
        .map(|f| &f.at);
    for missing in local.filter(|_| !texts_only) {
        let at = crate::samples::resolve::local(missing, &base)?;
        if !at.exists() {
            // Only a relative path is read from the study folder.
            return Err(match Path::new(missing).is_relative() && !crate::samples::resolve::in_home(missing) {
                true => format!("{missing}, which the lock names, is not in {}. The lock's relative paths are from the study folder: name it with --study FOLDER.", base.display()),
                false => format!("{missing}, which the lock names, is not there ({}): put it back where the lock recorded it.", at.display()),
            });
        }
    }
    let dir = dir.unwrap_or_else(|| file.with_extension("reproduced"));
    let written = [("recipe.toml", &texts.recipe), ("samples.toml", &texts.samples), ("samples.lock", &texts.lock)];
    let there = in_the_way(&dir, &written);
    if !there.is_empty() {
        return Err(format!("{} already holds {}, which rvr reproduce would press over: name another folder, or move them away.", dir.display(), there.join(", ")));
    }
    std::fs::create_dir_all(&dir).map_err(|e| rowver::plain_io(&dir, &e))?;
    for (name, text) in written {
        std::fs::write(dir.join(name), text).map_err(|e| rowver::plain_io(dir.join(name), &e))?;
    }
    println!(
        "Wrote {}/ with the recipe.toml, samples.toml and samples.lock {shown} holds.",
        dir.display()
    );
    if texts_only {
        return Ok(0);
    }
    let target = dir.join(format!("{name}.rvr"));
    let place = Place { base, study: name.to_owned(), target: target.clone(), at: At::Missing, locked: None, found: None };
    let asked = Asked { choose: Choose::Recorded(made["files"].clone()), on: On::Once, workers, sign: true };
    let again = |why: String| format!("{why}\nTo press it again, remove {} and run rvr reproduce again.", dir.display());
    let plan = crate::plan::build(place, texts, asked).map_err(again)?;
    crate::core::press(plan).map_err(again)?;
    println!("Comparing the two with rvr diff:");
    Ok(crate::diff::main(
        &file.to_string_lossy(),
        &target.to_string_lossy(),
    ))
}

/// What in `dir` a reproduce would press over: any `.rvr` file, and any of the texts it writes
/// that holds other than what it would write there.
fn in_the_way(dir: &Path, written: &[(&str, &String)]) -> Vec<String> {
    let mut there: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| name.ends_with(".rvr")).collect();
    there.sort();
    for (name, text) in written {
        if dir.join(name).exists() && std::fs::read_to_string(dir.join(name)).ok().as_ref() != Some(*text) {
            there.push(format!("a {name} of its own"));
        }
    }
    there
}

/// `rvr reproduce` with its options: the exit status.
pub fn main(options: &Options) -> u8 {
    let workers = match crate::options::workers(options) {
        Ok(workers) => workers,
        Err(why) => return crate::wrong("reproduce", &why),
    };
    let file = PathBuf::from(&options.words[0]);
    let dir = options.words.get(1).map(PathBuf::from);
    crate::report("reproduce", run(&file, dir, options.value("--study").map(PathBuf::from), workers, options.has("--texts")))
}
