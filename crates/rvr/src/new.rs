//! `rvr new DIR`: a study folder to start from, with a recipe and samples that work as they are.

use std::path::Path;

const SAMPLES: &str = include_str!("../templates/samples.toml");
const RECIPE: &str = include_str!("../templates/recipe.toml");
const GITIGNORE: &str = include_str!("../templates/gitignore");

/// Write `samples.toml`, `recipe.toml` and `.gitignore` into `dir`, making it if need be; nothing
/// that is already there is overwritten.
pub fn main(dir: &str) -> Result<(), String> {
    let folder = Path::new(dir);
    // Named as a press names the file: after the folder's own name, `.`'s the current folder's.
    let named = folder.canonicalize().ok().or_else(|| std::env::current_dir().ok().map(|here| here.join(folder)));
    let study = named.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !rowver::rvr::study_name(&study) {
        return Err(format!("{dir}: the folder's name {study:?} is not one a study can have (nonempty, not . or .., with no /, \\ or control character): name another folder"));
    }
    let files = [
        ("samples.toml", SAMPLES.to_string()),
        ("recipe.toml", RECIPE.replace("{study}", &study)),
        (".gitignore", GITIGNORE.to_string()),
    ];
    let there: Vec<&str> = files
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| folder.join(name).exists())
        .collect();
    if !there.is_empty() {
        return Err(format!("{dir} already has {}; rvr new writes nothing over a file. Use another folder, or move those away.", there.join(" and ")));
    }
    std::fs::create_dir_all(folder).map_err(|e| rowver::plain_io(folder, &e))?;
    for (name, text) in &files {
        std::fs::write(folder.join(name), text)
            .map_err(|e| rowver::plain_io(format!("{dir}/{name}"), &e))?;
    }
    println!(
        "Made {dir}/ with
  samples.toml   what to read: two CMS Open Data samples, which need no grid proxy
  recipe.toml    what to keep and store: a muon and tau selection for them
  .gitignore     keeps pressed .rvr files out of git

Next:
  cd {dir}
  rvr samples resolve samples.toml       find every file; writes samples.lock
  rvr samples plan samples.lock          what a press would read
  rvr branches                           the branches the files hold, as a recipe names them
  rvr try \"count(good_muon)\"             what an expression gives on the first events
  rvr press --events 50000               a quick first press; rvr press for every event
  {:38} check the file it wrote",
        format!("rvr validate {study}.rvr")
    );
    Ok(())
}
