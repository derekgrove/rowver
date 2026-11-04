//! `made` (docs/FORMAT.md §6): its fields and their values, the study's three texts against their
//! checksums, and `made.sha256`.

use super::{checksum, digest, exactly, list, nonempty, object, text, MEND};
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::HashSet;

/// The texts a file carries of the study that made it, each with the SHA-256 of its text.
const TEXTS: [&str; 3] = ["recipe", "samples", "lock"];

/// Whether `text` is a version `15.MINOR.PATCH`, each part digits with no leading zero.
fn version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    let number = |part: &&str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) && (part.len() == 1 || !part.starts_with('0'));
    parts.len() == 3 && parts[0] == crate::VERSION.to_string() && parts.iter().all(number)
}

/// Whether `text` is a commit as `made.toolchain.commit` names it: 40 lowercase hexadecimal
/// digits, then `-dirty-` and 12 more or `-dirty` alone when the code had changes; or `unknown`.
fn commit(text: &str) -> bool {
    let hex = |text: &str, n| crate::study::hex(text, n);
    if text == "unknown" {
        return true;
    }
    let (Some(sha), Some(dirty)) = (text.get(..40), text.get(40..)) else {
        return false;
    };
    hex(sha, 40) && (dirty.is_empty() || dirty == "-dirty" || dirty.strip_prefix("-dirty-").is_some_and(|h| hex(h, 12)))
}

/// Whether `name` can be a study folder's name: nonempty, not `.` or `..`, with no `/`, `\` or control character.
pub fn study_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

/// Check what built a program, as `made.toolchain` records it (docs/FORMAT.md §6): what is wrong, if anything.
pub fn toolchain(toolchain: &Value) -> std::result::Result<(), String> {
    let place = "made.toolchain";
    let fields = ["program", "version", "rowver", "commit", "compiler", "target"];
    let held = toolchain.as_object().ok_or_else(|| format!("{place} must be an object"))?;
    if let Some(key) = held.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(format!("{place} has a field {key:?}, which the format does not give it"));
    }
    let text = |key: &str| held.get(key).and_then(Value::as_str).ok_or_else(|| format!("{place}.{key} must be text"));
    if text("program")?.is_empty() {
        return Err(format!("{place}.program must be nonempty text: the name of the program that wrote the file, rvr for RowVer's own"));
    }
    let made_with = text("version")?;
    if !version(made_with) {
        return Err(format!("{place}.version is {made_with:?}, and it is the program's version, {}.MINOR.PATCH (docs/FORMAT.md §6)", crate::VERSION));
    }
    if text("rowver")? != crate::VERSION.to_string() {
        return Err(format!("{place}.rowver is {}, and it is \"{}\", the format version (docs/FORMAT.md §6)", toolchain["rowver"], crate::VERSION));
    }
    let made_by = text("commit")?;
    if !commit(made_by) {
        return Err(format!("{place}.commit is {made_by:?}, and it is 40 lowercase hexadecimal digits (with -dirty-HHHHHHHHHHHH or -dirty after them), or unknown (docs/FORMAT.md §6)"));
    }
    for key in ["compiler", "target"] {
        if text(key)?.is_empty() {
            return Err(format!("{place}.{key} must be nonempty text"));
        }
    }
    Ok(())
}

/// Validate `made`: its fields, each value's form, the study's texts against their checksums, and
/// `made.sha256`.
pub(super) fn validate(made: &Value) -> Result<()> {
    let fields = ["study", "title", "built", "by", "toolchain", "recipe", "samples", "lock", "files", "sha256"];
    exactly(made, "made", &fields, &["title"])?;
    let name = text(made, "made", "study")?;
    if !study_name(name) {
        return Err(invalid(format!("made.study is {name:?}, and a study folder's name is nonempty, not . or .., with no /, \\ or control character (docs/FORMAT.md §6): {MEND}")));
    }
    if made.get("title").is_some() {
        text(made, "made", "title")?;
    }
    let built = text(made, "made", "built")?;
    if !crate::study::utc(built) {
        return Err(invalid(format!("made.built is {built:?}, and it is a UTC time to the second, YYYY-MM-DDTHH:MM:SSZ (docs/FORMAT.md §6): {MEND}")));
    }
    nonempty(made, "made", "by")?;
    toolchain(object(made, "made", "toolchain")?).map_err(|e| invalid(format!("{e}: {MEND}")))?;
    object(made, "made", "files")?;
    for name in TEXTS {
        let place = format!("made.{name}");
        let record = object(made, "made", name)?;
        match name {
            "recipe" => exactly(record, &place, &["sha256", "text", "format", "columns"], &[])?,
            _ => exactly(record, &place, &["sha256", "text"], &[])?,
        }
        if digest(text(record, &place, "text")?.as_bytes()) != checksum(record, &place)? {
            return Err(invalid(format!("{place}.sha256 is not the SHA-256 of {place}.text: the text was changed after the file was written; press its study again")));
        }
    }
    let recipe = &made["recipe"];
    if text(recipe, "made.recipe", "format")? != "rvr-recipe" {
        return Err(invalid(format!("made.recipe.format is {}, and it is \"rvr-recipe\" (docs/FORMAT.md §6): {MEND}", recipe["format"])));
    }
    let mut seen = HashSet::new();
    for column in list(recipe, "made.recipe", "columns")? {
        let name = column.as_str().filter(|name| !name.is_empty());
        if !name.is_some_and(|name| seen.insert(name)) {
            return Err(invalid(format!("made.recipe.columns must name each column the recipe stores once, as nonempty text, and it holds {column} (docs/FORMAT.md §6): {MEND}")));
        }
    }
    if crate::write::made_checksum(made)? != checksum(made, "made")? {
        return Err(invalid("made.sha256 is not the SHA-256 of made's canonical JSON without it (docs/FORMAT.md §8): made was changed after the file was written; press its study again"));
    }
    Ok(())
}

/// The columns the recipe pressed, in order.
pub(super) fn pressed(made: &Value) -> Vec<&str> {
    made["recipe"]["columns"].as_array().into_iter().flatten().filter_map(Value::as_str).collect()
}

/// Ensure a pressed file holds exactly the recipe's columns, in its order.
pub(super) fn columns(made: &Value, columns: &[Value]) -> Result<()> {
    let names: Vec<&str> = columns.iter().filter_map(|column| column["name"].as_str()).collect();
    if names != pressed(made) {
        return Err(invalid(format!("data.cols holds the columns {names:?}, and they are those made.recipe.columns names, in its order ({:?}) (docs/FORMAT.md §3): {MEND}", pressed(made))));
    }
    Ok(())
}
