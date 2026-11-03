//! A study's three texts, read from its folder, and the validator of its recipe.toml.

use rowver::study::lock::Lock;
use rvr_recipe::Recipe;
use std::path::{Path, PathBuf};

/// The recipe reference, which lists every function an expression can call.
const RECIPES: &str = rvr_recipe::REFERENCE;

/// A study folder's full path, its name, and the file a press writes (`--out`, or `DIR/<name>.rvr`).
pub fn study(
    given: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<(PathBuf, String, PathBuf), String> {
    let given = given.unwrap_or_else(|| PathBuf::from("."));
    let dir = given
        .canonicalize()
        .map_err(|e| format!("{}: {e}", given.display()))?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("study")
        .to_string();
    if !rowver::rvr::study_name(&name) {
        return Err(format!("{}: the study folder's name {name:?} is not one a file can record (nonempty, not . or .., with no /, \\ or control character): rename the folder", dir.display()));
    }
    let target = out.unwrap_or_else(|| match given.as_os_str() == "." {
        true => PathBuf::from(format!("{name}.rvr")),
        false => given.join(format!("{name}.rvr")),
    });
    Ok((dir, name, target))
}

/// The three texts of a study, which the file carries.
#[derive(Clone)]
pub struct Texts {
    /// recipe.toml.
    pub recipe: String,
    /// samples.toml.
    pub samples: String,
    /// samples.lock.
    pub lock: String,
}

/// One of a study's files, as text; one missing says what to do.
fn study_file(dir: &Path, file: &str) -> Result<String, String> {
    std::fs::read_to_string(dir.join(file)).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound if file == "samples.lock" => format!(
            "{} has no samples.lock: run rvr samples resolve samples.toml there first.",
            dir.display()
        ),
        std::io::ErrorKind::NotFound if file == "samples.toml" && dir.join("samples.lock").exists() => format!(
            "{} has a samples.lock but no samples.toml. A .rvr file carries both, so press needs both: put samples.toml back (git checkout samples.toml).",
            dir.display()
        ),
        std::io::ErrorKind::NotFound if file == "samples.toml" => format!(
            "{} has no samples.toml: write one, saying what the study reads (rvr new makes a study with one), then run rvr samples resolve samples.toml.",
            dir.display()
        ),
        std::io::ErrorKind::NotFound if file == "recipe.toml" => format!(
            "{} has no recipe.toml: write one (the recipe reference, {RECIPES}, says how; rvr new makes a study with one).",
            dir.display()
        ),
        _ => format!("{}: {e}", dir.join(file).display()),
    })
}

/// A study's three texts, as they are; every one missing said at once.
pub fn read(dir: &Path) -> Result<Texts, String> {
    let read = ["recipe.toml", "samples.toml", "samples.lock"].map(|file| study_file(dir, file));
    let said: Vec<&str> = read.iter().filter_map(|r| r.as_ref().err().map(String::as_str)).collect();
    if !said.is_empty() {
        return Err(said.join("\n"));
    }
    let [recipe, samples, lock] = read.map(Result::unwrap_or_default);
    Ok(Texts { recipe, samples, lock })
}

/// A study's lock, read on rails.
pub fn lock(dir: &Path) -> Result<Lock, String> {
    let text = study_file(dir, "samples.lock")?;
    Lock::read(&text).map_err(|p| format!("samples.lock does not check out: {}. Run rvr samples resolve samples.toml again.", p.to_string().replace('\n', "; ")))
}

/// The recipe a press runs, or every problem with it, one a line: what `Recipe::read` refuses,
/// any of the five tables left out, and a `[columns]` that stores nothing.
pub fn read_recipe(source: &str) -> Result<Recipe, String> {
    let mut said: Vec<String> = Vec::new();
    let recipe = match Recipe::read(source) {
        Ok(recipe) => Some(recipe),
        Err(e) => {
            said.extend(e.0.lines().map(|line| format!("recipe.toml: {line}")));
            None
        }
    };
    if let Ok(root) = source.parse::<toml::Table>() {
        let missing = crate::tables::lacking(&root);
        if !missing.is_empty() {
            said.push(crate::tables::refusal(&missing));
        }
    }
    if recipe.as_ref().is_some_and(|recipe| recipe.columns.is_empty()) {
        said.push("recipe.toml stores nothing: under [columns], list branches (branches = [\"MET_pt\"]) or name computed columns (ht = \"sum(Jet_pt)\").".into());
    }
    match (recipe, said.is_empty()) {
        (Some(recipe), true) => Ok(recipe),
        _ => Err(said.join("\n")),
    }
}
