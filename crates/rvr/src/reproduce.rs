//! `rvr reproduce`: write a file's study back out of it, press it again, and compare the two.

use crate::press::{self, Texts};
use crate::samples::resolve::{Lock, Locked};
use rowver::choice::Events;
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
    let lock: Lock = toml::from_str(&texts.lock)
        .map_err(|e| format!("{shown}: its samples.lock does not read: {e}"))?;
    let local = lock.sample.iter().flat_map(|s| &s.file);
    let local = local
        .filter(|f| !f.at.contains("://"))
        .map(|f| &f.at);
    if let Some(missing) = local.into_iter().find(|at| !base.join(at).exists()) {
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
    let files = made["files"].clone();
    let asked = |sample: &Locked| {
        Events::read(&files[&sample.name]["events"]).map_err(|e| format!("{}: {e}", sample.name))
    };
    press::press(
        &press::inputs(&dir)?,
        &base,
        name,
        &target,
        &asked,
        workers,
        press::On::Once,
    )
    .map_err(|why| format!("{why}\nTo press it again, remove {} and run rvr reproduce again.", dir.display()))?;
    println!("Comparing the two with rvr diff:");
    Ok(crate::diff::main(
        &file.to_string_lossy(),
        &target.to_string_lossy(),
    ))
}

/// `rvr reproduce …` with the words after `reproduce`: the exit status.
pub fn main(args: &[&str]) -> u8 {
    let mut words = args.iter();
    let (mut paths, mut workers, mut study) = (Vec::new(), None, None);
    while let Some(&word) = words.next() {
        match word {
            "--workers" => match words
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| *n > 0)
            {
                Some(n) => workers = Some(n),
                None => {
                    eprintln!(
                        "--workers is how many files are read at once, a whole number from 1"
                    );
                    return 2;
                }
            },
            "--study" => match words.next() {
                Some(folder) => study = Some(PathBuf::from(folder)),
                None => {
                    eprintln!("--study needs the study folder the lock's relative paths are from");
                    return 2;
                }
            },
            flag if flag.starts_with("--") => {
                eprintln!("reproduce does not know {flag}.\n{USAGE}");
                return 2;
            }
            path => paths.push(PathBuf::from(path)),
        }
    }
    let workers =
        workers.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let (file, dir) = match paths.as_slice() {
        [file] => (file.clone(), None),
        [file, dir] => (file.clone(), Some(dir.clone())),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match run(&file, dir, study, workers) {
        Ok(code) => code,
        Err(why) => {
            eprintln!("rvr reproduce: {why}");
            1
        }
    }
}
