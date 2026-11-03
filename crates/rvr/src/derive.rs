//! `rvr derive FILE`: add, drop and rename the derived columns of a pressed file, keeping the
//! recipe it carries in step; and the records of a press's derived columns. rowver works them out.

use crate::options::Options;
use rowver::rvr::derived::records;
use rowver::rvr::Reader;
use rowver::write::Laid;
use rvr_recipe::{parse, plain, reads, Recipe, RESERVED};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) const USAGE: &str = "usage:
  rvr derive FILE 'NAME = EXPRESSION'...
      add derived columns to a pressed file: each kept as its expression over the file's columns
  rvr derive FILE --drop NAME...
      remove derived columns
  rvr derive FILE --rename OLD NEW
      give a derived column another name; its expression stays as it is";

/// Put the records of `derived`, worked out from the arrays `laid` for a press, in its header,
/// refused unless every reader would open it.
pub(crate) fn attach(laid: &Laid, derived: &[(String, String)], header: &mut Value) -> Result<(), String> {
    let records = laid.derived(header, derived).map_err(|e| e.to_string())?;
    header["derived"] = json!({ "columns": records });
    sealed(header, None)
}

/// Take the derived columns `names` out of a press's header, and empty the `[derived]` table of
/// the recipe it carries: what a press read, kept when its derived columns failed.
pub(crate) fn without(header: &mut Value, names: Vec<String>) -> Result<(), String> {
    let text = header["made"]["recipe"]["text"].as_str().unwrap_or_default().to_owned();
    // A [derived] table not written a line per column is emptied in the recipe's TOML instead,
    // written again whole: the recipe loses its comments and layout.
    let text = edited(&text, &dropping(names), &[]).or_else(|_| {
        let mut table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        table.insert("derived".into(), toml::Table::new().into());
        toml::to_string(&table).map_err(|e| e.to_string())
    })?;
    let made = &mut header["made"];
    made["recipe"]["text"] = text.as_str().into();
    made["recipe"]["sha256"] = rowver::write::text_checksum(&text).into();
    made["sha256"] = rowver::write::made_checksum(made).map_err(|e| e.to_string())?.into();
    if let Some(root) = header.as_object_mut() {
        root.shift_remove("derived");
    }
    Ok(())
}

/// Sign `header` when there is a key, and refuse it unless every reader would open it.
pub(crate) fn sealed(header: &mut Value, signer: Option<&[u8; 32]>) -> Result<(), String> {
    if let Some(secret) = signer {
        rowver::sign::sign(header, secret).map_err(|e| format!("the file could not be signed with your key: {e}"))?;
    }
    let text = serde_json::to_vec(header).map_err(|e| e.to_string())?;
    rowver::rvr::checked_header(&text).map(drop).map_err(|e| format!("it would not be valid: {e}"))
}

/// Write `from` again at `to` with `header`, sealed.
pub(crate) fn rewritten(from: &Path, to: &Path, header: &mut Value, signer: Option<&[u8; 32]>) -> Result<(), String> {
    sealed(header, signer).map_err(|why| format!("{} could not be written: {why}", to.display()))?;
    rowver::write::rewrite(from, to, header).map_err(|e| e.to_string())
}

/// A change that removes the `[derived]` lines of these names.
fn dropping(names: Vec<String>) -> impl Fn(&mut Vec<String>, (usize, usize)) -> Result<(), String> {
    move |lines, table| {
        let mut gone = Vec::new();
        for name in &names {
            let view: Vec<&str> = lines.iter().map(String::as_str).collect();
            gone.push(line_of(&view, table, name).ok_or_else(|| format!("rvr could not find {name} in the [derived] table of the recipe the file carries"))?);
        }
        gone.sort_unstable();
        for at in gone.into_iter().rev() {
            lines.remove(at);
        }
        Ok(())
    }
}

/// Refuse a name a derived column of this file cannot have, saying why.
fn free(header: &Value, recipe: &Recipe, name: &str, path: &Path) -> Result<(), String> {
    if !plain(name) || RESERVED.contains(&name) {
        return Err(format!("{name:?} is not a name a derived column can have: one plain name, such as ht_over_met"));
    }
    let has = |key: &str| header["data"][key].as_array().into_iter().flatten().any(|c| c["name"] == name);
    let added = crate::press::ADDED;
    let mut samples = header["made"]["files"].as_object().into_iter().flat_map(|files| files.values());
    let keyed = samples.any(|sample| sample["metadata"].get(name).is_some());
    let whose = if has("cols") {
        format!("a stored column of {}", path.display())
    } else if recipe.derived.iter().any(|(n, _)| n == name) {
        format!("a derived column of {}", path.display())
    } else if recipe.defines.contains_key(name) {
        format!("a line of [define] in the recipe {} carries", path.display())
    } else if has("sample_cols") || added.contains(&name) || keyed {
        format!("a sample column or metadata key of {}, or of one rvr press gives every sample ({})", path.display(), added.join(", "))
    } else {
        return Ok(());
    };
    Err(format!("{name} is already the name of {whose}: give the derived column another name"))
}

/// `rvr derive` with its options: the exit status (2 for a wrong use of it).
pub fn main(options: &Options) -> u8 {
    let (path, change) = match asked(options) {
        Ok(asked) => asked,
        Err(why) => return crate::wrong("derive", &why),
    };
    match run(path, change) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rvr derive: {error}");
            1
        }
    }
}

/// What `rvr derive` is asked to change, its words read.
enum Asked<'a> {
    /// `--drop NAME…`, each name once.
    Drop(Vec<&'a str>),
    /// `--rename OLD NEW`.
    Rename(&'a str, &'a str),
    /// `'NAME = EXPRESSION'…`, each split at its first `=`.
    Add(Vec<(&'a str, &'a str)>),
}

/// The file derive changes and what it changes, or how its words do not fit its flags (wrong use).
fn asked(options: &Options) -> Result<(&Path, Asked<'_>), String> {
    let (path, rest) = options.words.split_first().ok_or("derive takes a file and what to change of it")?;
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let change = match (options.has("--drop"), options.has("--rename"), rest.as_slice()) {
        (true, true, _) => return Err("derive takes --drop or --rename, not both".into()),
        (false, true, [old, new]) => Asked::Rename(old, new),
        (false, true, _) => return Err("--rename takes the derived column's name and its new one".into()),
        (true, false, _) => {
            let mut once = Vec::new();
            for name in rest {
                if !once.contains(&name) {
                    once.push(name);
                }
            }
            Asked::Drop(once)
        }
        (false, false, lines) => {
            let mut added = Vec::new();
            for line in lines {
                let (name, expression) = line.split_once('=').ok_or_else(|| format!("{line:?} is not NAME = EXPRESSION"))?;
                added.push((name.trim(), expression.trim()));
            }
            Asked::Add(added)
        }
    };
    Ok((Path::new(path), change))
}

/// The line of `[derived]` that holds `name`, by its index among `lines`, if there is one.
fn line_of(lines: &[&str], table: (usize, usize), name: &str) -> Option<usize> {
    (table.0 + 1..table.1).find(|&at| {
        let key = lines[at].split_once('=').map(|(key, _)| key.trim());
        key == Some(name) || key == Some(&format!("\"{name}\"")) || key == Some(&format!("'{name}'"))
    })
}

/// Where the `[derived]` table of a recipe's lines is: its heading's index, and the index of the
/// line after it ends.
fn table(lines: &[&str]) -> Option<(usize, usize)> {
    let heading = lines.iter().position(|line| crate::tables::header(line.trim(), &crate::tables::TABLES) == Some("derived"))?;
    let end = (heading + 1..lines.len())
        .find(|&at| lines[at].trim_start().starts_with('['))
        .unwrap_or(lines.len());
    Some((heading, end))
}

/// A change to a recipe's lines, given where its `[derived]` table lies.
type Change<'a> = dyn Fn(&mut Vec<String>, (usize, usize)) -> Result<(), String> + 'a;

/// The recipe's text with its `[derived]` table changed as `change` says, refused when the
/// table then does not read as `expected`.
fn edited(text: &str, change: &Change, expected: &[(String, String)]) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut owned: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    let table = match table(&lines) {
        Some(table) => table,
        None if text.parse::<toml::Table>().is_ok_and(|root| root.contains_key("derived")) => {
            return Err("rvr derive changes the [derived] table of the recipe a file carries a line at a time, under a heading written [derived], and this one is written otherwise (inline, derived = { … }, or as another heading): write it so in recipe.toml, a line per column, and press again".into());
        }
        None => {
            if owned.last().is_some_and(|line| !line.trim().is_empty()) {
                owned.push(String::new());
            }
            owned.push("[derived]".into());
            (owned.len() - 1, owned.len())
        }
    };
    change(&mut owned, table)?;
    let mut text = owned.join("\n");
    text.push('\n');
    let recipe = Recipe::read(&text).map_err(|e| e.line())?;
    if recipe.derived != expected {
        return Err("rvr could not change the [derived] table of the recipe the file carries: it writes each derived column on a line of its own, NAME = \"EXPRESSION\"".into());
    }
    Ok(text)
}

fn run(path: &Path, change: Asked) -> Result<(), String> {
    let file = Reader::open(path).map_err(|e| rowver::plain(path, &e))?;
    // A file is changed, and signed again, only when its derived columns are what it records.
    let secret = crate::key::secret()?;
    rowver::rvr::derived::checked(&file).map_err(|e| rowver::plain(path, &e))?;
    let mut header = file.header().clone();
    let text = header["made"]["recipe"]["text"].as_str().unwrap_or_default().to_owned();
    let recipe = Recipe::read(&text).map_err(|e| format!("{}: the recipe it carries cannot be read by this rvr ({}). {}", path.display(), e.line(), rowver::rvr::derived::remedy(&header)))?;
    let before: Vec<Value> = header["derived"]["columns"].as_array().cloned().unwrap_or_default();
    let mut derived = recipe.derived.clone();
    let users = |name: &str, derived: &[(String, String)]| -> Option<String> {
        derived.iter().find_map(|(other, expression)| {
            let mut used = BTreeSet::new();
            reads(&parse(expression).ok()?, &mut used);
            used.contains(name).then(|| other.clone())
        })
    };
    let (text, said, kept) = match change {
        Asked::Drop(once) => {
            let rest = &once;
            for name in rest {
                if !derived.iter().any(|(n, _)| n == name) {
                    return Err(format!("{} has no derived column named {name}", path.display()));
                }
            }
            derived.retain(|(n, _)| !rest.contains(&n.as_str()));
            for name in rest {
                if let Some(user) = users(name, &derived) {
                    return Err(format!("{user} uses {name}: drop {user} first, or with it"));
                }
            }
            let names: Vec<&str> = rest.to_vec();
            let change = dropping(names.iter().map(|n| (*n).to_owned()).collect());
            (edited(&text, &change, &derived)?, format!("Dropped {} from", names.join(", ")), before.clone())
        }
        Asked::Rename(old, new) => {
            let at = derived.iter().position(|(n, _)| n == old).ok_or_else(|| format!("{} has no derived column named {old}", path.display()))?;
            if let Some(user) = users(old, &derived) {
                return Err(format!("{user} uses {old}: drop {user} first, or keep the name"));
            }
            free(&header, &recipe, new, path)?;
            derived[at].0 = (*new).to_owned();
            let change = |lines: &mut Vec<String>, table: (usize, usize)| {
                let view: Vec<&str> = lines.iter().map(String::as_str).collect();
                let line = line_of(&view, table, old).ok_or_else(|| format!("rvr could not find {old} in the [derived] table of the recipe the file carries"))?;
                let (_, rest) = lines[line].split_once('=').ok_or_else(|| format!("rvr could not find {old} in the [derived] table of the recipe the file carries"))?;
                lines[line] = format!("{new} ={rest}");
                Ok(())
            };
            // The record is kept as it is, under its new name: its values have not changed.
            let kept: Vec<Value> = before
                .iter()
                .cloned()
                .map(|mut r| {
                    if r["name"] == *old {
                        r["name"] = (*new).into();
                    }
                    r
                })
                .collect();
            (edited(&text, &change, &derived)?, format!("Renamed {old} to {new} in"), kept)
        }
        Asked::Add(lines) => {
            let mut added = Vec::new();
            for (name, expression) in lines {
                // Written as in recipe.toml, NAME = "EXPRESSION", it is that string.
                let quoted = expression.starts_with('"').then(|| format!("v = {expression}").parse::<toml::Table>().ok()).flatten();
                let quoted = quoted.and_then(|t| t.get("v")?.as_str().map(str::to_owned));
                let unquoted = quoted.as_deref().unwrap_or(expression);
                if derived.iter().any(|(n, _)| n == name) {
                    return Err(format!("{} already has a derived column named {name}: an expression is not changed in place; drop it, then add it again", path.display()));
                }
                free(&header, &recipe, name, path)?;
                derived.push((name.to_owned(), unquoted.to_owned()));
                added.push((name.to_owned(), unquoted.to_owned()));
            }
            let change = |lines: &mut Vec<String>, table: (usize, usize)| {
                // After the table's last line of its own: not a comment heading the next table.
                let own = |at: &usize| !lines[*at].trim().is_empty() && !lines[*at].trim_start().starts_with('#');
                let last = (table.0..table.1).rev().find(own).unwrap_or(table.0);
                for (offset, (name, expression)) in added.iter().enumerate() {
                    let quoted = toml::Value::String(expression.clone()).to_string();
                    lines.insert(last + 1 + offset, format!("{name} = {quoted}"));
                }
                Ok(())
            };
            let names: Vec<&str> = added.iter().map(|(n, _)| n.as_str()).collect();
            (edited(&text, &change, &derived)?, format!("Added {} to", names.join(", ")), before.clone())
        }
    };
    let records = records(&file, &derived, &kept).map_err(|e| e.to_string())?;
    let made = &mut header["made"];
    made["recipe"]["text"] = text.as_str().into();
    made["recipe"]["sha256"] = rowver::write::text_checksum(&text).into();
    made["sha256"] = rowver::write::made_checksum(made).map_err(|e| e.to_string())?.into();
    let root = header.as_object_mut().ok_or("the header is not an object")?;
    match records.is_empty() {
        true => root.shift_remove("derived"),
        false => root.insert("derived".into(), json!({ "columns": records })),
    };
    let was = root.shift_remove("signature");
    let signed = crate::key::resigned(secret.as_ref(), was.as_ref().and_then(|s| s["key"].as_str()));
    // The file is replaced only by one every reader opens, and only once it is on disk.
    let staged = crate::empty::beside(path, ".derive");
    let written = rewritten(path, &staged, &mut header, secret.as_ref())
        .and_then(|()| std::fs::File::open(&staged).and_then(|f| f.sync_all()).map_err(|e| rowver::plain_io(&staged, &e)))
        .and_then(|()| std::fs::rename(&staged, path).map_err(|e| rowver::plain_io(path, &e)));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    println!(
        "{said} {}: it has {}; {signed}",
        path.display(),
        crate::samples::some(records.len(), "derived column", "derived columns")
    );
    println!("Its study's recipe.toml is as it was: make the same change under [derived] there to keep it when the study is pressed again.");
    Ok(())
}
