//! `rvr reproduce`: write a file's study back out of it, press it again, and compare the two.

use crate::options::Options;
use crate::plan::{Asked, Choose, On, Place};
use crate::texts::Texts;
use rowver::study::lock::Lock;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage:
  rvr reproduce FILE [DIR] [--study FOLDER] [--workers N]
                                           write FILE's recipe.toml, samples.toml and samples.lock
                                           into DIR (FILE's name + .reproduced when not given),
                                           press them again, and rvr diff the two files; a
                                           relative path in the lock is read from FOLDER
                                           (FILE's folder when not given)";

/// The study texts a file's header carries, and its study's name.
pub(crate) fn study_texts(header: &serde_json::Value, file: &Path) -> Result<(Texts, String), String> {
    let made = &header["made"];
    let text = |name: &str| made[name]["text"].as_str().unwrap_or_default().to_owned();
    let texts = Texts {
        recipe: text("recipe"),
        samples: text("samples"),
        lock: text("lock"),
    };
    let name = made["study"].as_str().unwrap_or_default();
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return Err(format!("{} names its study {name:?}, which is not a folder name; it cannot be written as one.", file.display()));
    }
    Ok((texts, name.to_owned()))
}

/// The folder a file's lock's relative paths are from: `study`, or else where press wrote the
/// file, unless it has moved since.
pub(crate) fn base(file: &Path, study: Option<&Path>) -> Result<PathBuf, String> {
    match study {
        Some(folder) => Ok(folder.to_path_buf()),
        None => Ok(file
            .canonicalize()
            .map_err(|e| format!("{}: {e}", file.display()))?
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()),
    }
}

/// Reproduce `file` into `dir`, reading relative inputs from `study` (FILE's folder when `None`).
fn run(
    file: &Path,
    dir: Option<PathBuf>,
    study: Option<PathBuf>,
    workers: usize,
) -> Result<u8, String> {
    let shown = file.display();
    let reader = rowver::rvr::Reader::open(file).map_err(|e| format!("{shown}: {e}"))?;
    let made = &reader.header()["made"];
    let (texts, name) = study_texts(reader.header(), file)?;
    let name = name.as_str();
    let base = base(file, study.as_deref())?;
    let lock = Lock::read(&texts.lock).map_err(|p| format!("{shown}: its samples.lock does not check out: {p}"))?;
    let local = lock.samples.iter().flat_map(|s| &s.files);
    let local = local
        .filter(|f| !f.at.contains("://"))
        .map(|f| &f.at);
    if let Some(missing) = local.into_iter().find(|at| !crate::samples::resolve::local(at, &base).exists()) {
        return Err(format!("{missing}, which the lock names, is not in {}. The lock's relative paths are from the study folder: name it with --study FOLDER.", base.display()));
    }
    let dir = dir.unwrap_or_else(|| file.with_extension("reproduced"));
    for name in ["recipe.toml", "samples.toml", "samples.lock"] {
        if dir.join(name).exists() {
            return Err(format!(
                "{} already holds {name}; name another folder, or remove it.",
                dir.display()
            ));
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, text) in [
        ("recipe.toml", &texts.recipe),
        ("samples.toml", &texts.samples),
        ("samples.lock", &texts.lock),
    ] {
        std::fs::write(dir.join(name), text)
            .map_err(|e| format!("{}: {e}", dir.join(name).display()))?;
    }
    println!(
        "Wrote {}/ with the recipe.toml, samples.toml and samples.lock {shown} holds.",
        dir.display()
    );
    let target = dir.join(format!("{name}.rvr"));
    let place = Place { base, study: name.to_owned(), target: target.clone() };
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

/// `rvr reproduce` with its options: the exit status.
pub fn main(options: &Options) -> u8 {
    let workers = match crate::options::workers(options) {
        Ok(workers) => workers,
        Err(why) => return crate::wrong("reproduce", &why),
    };
    let file = PathBuf::from(&options.words[0]);
    let dir = options.words.get(1).map(PathBuf::from);
    match run(&file, dir, options.value("--study").map(PathBuf::from), workers) {
        Ok(code) => code,
        Err(why) => {
            eprintln!("rvr reproduce: {why}");
            1
        }
    }
}
