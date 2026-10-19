use super::{checksum, digest, list, object, string};
pub(super) use crate::json::canonical;
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::HashSet;

/// The texts a file carries of the study that made it, each with the SHA-256 of its text.
const TEXTS: [&str; 3] = ["recipe", "samples", "lock"];

/// What `made.toolchain` names.
const TOOLCHAIN: [&str; 5] = ["rvr", "rowver", "commit", "rustc", "target"];

/// Validate `made`: its fields, the study's texts against their checksums, and `made.sha256`.
pub(super) fn validate(made: &Value) -> Result<()> {
    for field in ["study", "built", "by"] {
        string(made, field)?;
    }
    if made.get("title").is_some() {
        string(made, "title")?;
    }
    let toolchain = object(made, "toolchain")?;
    for field in TOOLCHAIN {
        string(toolchain, field)
            .map_err(|_| invalid(format!("made.toolchain.{field} must be text")))?;
    }
    object(made, "files")?;
    for name in TEXTS {
        let record = object(made, name)?;
        if digest(string(record, "text")?.as_bytes()) != checksum(record)? {
            return Err(invalid(format!(
                "made.{name}.sha256 does not match the {name} text the file holds"
            )));
        }
    }
    let recipe = object(made, "recipe")?;
    string(recipe, "format")?;
    let mut seen = HashSet::new();
    for column in list(recipe, "columns")? {
        let name = column.as_str().filter(|name| !name.is_empty());
        if !name.is_some_and(|name| seen.insert(name)) {
            return Err(invalid(
                "made.recipe.columns must name each column once, as text",
            ));
        }
    }
    let mut body = made.clone();
    body.as_object_mut().unwrap().shift_remove("sha256");
    if digest(canonical(&body)?.as_bytes()) != checksum(made)? {
        return Err(invalid(
            "made.sha256 does not match the complete recipe snapshot",
        ));
    }
    Ok(())
}

/// The columns the recipe pressed, in order.
pub(super) fn pressed(made: &Value) -> Result<Vec<&str>> {
    list(object(made, "recipe")?, "columns")?
        .iter()
        .map(|name| {
            name.as_str()
                .ok_or_else(|| invalid("made.recipe.columns must be text"))
        })
        .collect()
}

/// Ensure a pressed file holds exactly the recipe's columns, in its order.
pub(super) fn columns(made: &Value, columns: &[Value]) -> Result<()> {
    let names: Vec<&str> = columns
        .iter()
        .map(|column| string(column, "name"))
        .collect::<Result<_>>()?;
    if names != pressed(made)? {
        return Err(invalid(
            "data.cols does not hold the columns made.recipe.columns names, in that order",
        ));
    }
    Ok(())
}
