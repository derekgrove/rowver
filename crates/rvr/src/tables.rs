//! A recipe's five tables: which ones a recipe.toml leaves out, and, asked at a terminal, writing
//! them in, empty and in their place.

use std::path::Path;

/// The tables every study's recipe.toml writes, in their order.
pub const TABLES: [&str; 5] = ["define", "keep", "columns", "totals", "derived"];

/// The tables of the five that `root` does not write.
pub fn lacking(root: &toml::Table) -> Vec<&'static str> {
    TABLES.into_iter().filter(|table| !root.contains_key(*table)).collect()
}

/// Why a recipe lacking `missing` is refused, and what to add.
pub fn refusal(missing: &[&str]) -> String {
    let add = match missing {
        [one] => format!("Add the line [{one}]."),
        _ => format!("Add {}, each on a line of its own.", listed(missing, "and")),
    };
    format!("{} {add}", lacks(missing))
}

/// At a terminal, offer to write in the tables `source`, recipe.toml in `dir`, lacks, before
/// `going` on (as "continue pressing") with the command `then` ("press"): whether it did.
pub fn offer(dir: &Path, source: &str, then: &str, going: &str) -> Result<bool, String> {
    let Ok(root) = source.parse() else { return Ok(false) };
    let missing = lacking(&root);
    let asking = crate::at_terminal();
    // A recipe whose [columns] stores nothing, which no press takes, is the user's to write.
    let stores = || rvr_recipe::Recipe::read(source).is_ok_and(|recipe| !recipe.columns.is_empty());
    if missing.is_empty() || !asking || !stores() {
        return Ok(false);
    }
    // Placed before asking, so that a yes is not refused after.
    let text = match written(source, &root, &missing) {
        Ok(text) => text,
        Err(why) => {
            eprintln!("{why}");
            return Ok(false);
        }
    };
    let named = listed(&missing, "and");
    let unchanged = |why: String| Err(format!("{why}, so recipe.toml is as it was: add {named} to it, then {then} again."));
    match crate::yes(&asked(&missing, &named, going)) {
        Err(why) => return unchanged(why),
        Ok(false) => return unchanged("not added".into()),
        Ok(true) => {}
    }
    // Written beside the file (the one a link names) and renamed over it, so that a failed write
    // leaves it as it was; and not over an edit saved while rvr asked.
    let path = dir.join("recipe.toml").canonicalize().map_err(|e| format!("recipe.toml: {e}"))?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(source) {
        return Err(format!("recipe.toml changed while rvr asked, so it is as you saved it: {then} again."));
    }
    let staged = crate::empty::beside(&path, ".rvr-new");
    let wrote = std::fs::write(&staged, text)
        .and_then(|()| std::fs::set_permissions(&staged, std::fs::metadata(&path)?.permissions()))
        .and_then(|()| std::fs::File::open(&staged)?.sync_all())
        .and_then(|()| std::fs::rename(&staged, &path));
    if let Err(e) = wrote {
        _ = std::fs::remove_file(&staged);
        return unchanged(format!("it could not be written ({e})"));
    }
    eprintln!("Added {named} to recipe.toml.");
    Ok(true)
}

/// `source` (read as `root`) with each of `missing` written in, empty, before the next of the five
/// it has, else at the end; refused unless it reads as `source` did with only those tables added.
fn written(source: &str, root: &toml::Table, missing: &[&str]) -> Result<String, String> {
    let lines: Vec<(usize, &str)> = source
        .split_inclusive('\n')
        .scan(0, |at, line| {
            *at += line.len();
            Some((*at - line.len(), line.trim()))
        })
        .collect();
    // Where each of the five begins: its header line, or the comment lines just above it when a
    // blank line (or the file's start) is above those and none reads as a key put out of use;
    // else they close the table before.
    let mut begins: Vec<(&str, usize)> = Vec::new();
    for (index, (at, line)) in lines.iter().enumerate() {
        let Some(table) = header(line).filter(|table| !begins.iter().any(|(name, _)| name == table)) else { continue };
        let first = (0..index).rev().take_while(|&above| lines[above].1.starts_with('#')).last().unwrap_or(index);
        let keyed = lines[first..index].iter().any(|(_, comment)| unkeyed(comment));
        let own = (first == 0 || lines[first - 1].1.is_empty()) && !keyed;
        begins.push((table, if own { lines[first].0 } else { *at }));
    }
    let mut places: Vec<(usize, &str)> = TABLES
        .iter()
        .enumerate()
        .filter(|(_, table)| missing.contains(table))
        .map(|(index, table)| {
            let next = TABLES[index + 1..].iter().find_map(|later| begins.iter().find(|(name, _)| name == later));
            (next.map_or(source.len(), |(_, at)| *at), *table)
        })
        .collect();
    // Stable: tables written at one place keep the five's order.
    places.sort_by_key(|(at, _)| *at);
    let crlf = source.contains("\r\n") && source.matches('\n').count() == source.matches("\r\n").count();
    let end = if crlf { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(source.len() + 16 * places.len());
    let mut from = 0;
    for (at, table) in places {
        out.push_str(&source[from..at]);
        from = at;
        if !out.is_empty() && !out.ends_with(end) {
            out.push_str(end);
        }
        if !out.is_empty() && !out.ends_with(&end.repeat(2)) {
            out.push_str(end);
        }
        out.push_str(&format!("[{table}]{end}"));
        if at < source.len() {
            out.push_str(end);
        }
    }
    out.push_str(&source[from..]);
    let mut expected = root.clone();
    for table in missing {
        expected.insert(table.to_string(), toml::Value::Table(toml::Table::new()));
    }
    match out.parse::<toml::Table>() {
        Ok(read) if read == expected => Ok(out),
        _ => Err(format!("{} could not be written into recipe.toml in {}, so it is as it was: add {} by hand.", listed(missing, "and"), place(missing), if missing.len() == 1 { "it" } else { "them" })),
    }
}

/// The one of the five a line (trimmed) heads, as `[keep]`.
pub fn header(line: &str) -> Option<&'static str> {
    let (name, rest) = line.strip_prefix('[')?.split_once(']')?;
    let rest = rest.trim_start();
    (rest.is_empty() || rest.starts_with('#')).then_some(())?;
    TABLES.into_iter().find(|table| *table == name.trim())
}

/// Whether a comment line reads as a key put out of use, as `# ht = "sum(Jet_pt)"`.
fn unkeyed(comment: &str) -> bool {
    let words = comment.trim_start_matches('#').trim();
    words.split_once('=').is_some_and(|(key, _)| {
        let key = key.trim();
        !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || "_-.\"'".contains(c))
    })
}

/// What a recipe lacking `missing` (`named`) is asked before `going` on: the same for any.
fn asked(missing: &[&str], named: &str, going: &str) -> String {
    format!("{}\nAdd {named} to recipe.toml, empty and in {}, then {going}? [y/N] ", lacks(missing), place(missing))
}

/// Where `missing` go: "its place", or "their places".
fn place(missing: &[&str]) -> &'static str {
    if missing.len() == 1 { "its place" } else { "their places" }
}

/// What a recipe lacking `missing` is told, before how to fix it.
fn lacks(missing: &[&str]) -> String {
    format!(
        "recipe.toml has no {} table: every recipe has its five tables, {}, each written even when it is empty.",
        listed(missing, "or"),
        listed(&TABLES, "and")
    )
}

/// `tables` named, the last joined by `word`: "[define], [keep] and [totals]".
fn listed(tables: &[&str], word: &str) -> String {
    let named: Vec<String> = tables.iter().map(|table| format!("[{table}]")).collect();
    match named.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} {word} {last}", rest.join(", ")),
        _ => named.concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tables_are_written_where_they_belong() {
        // Before the next table and the comment just above it.
        let source = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"x\"]\n\n# Worked out when read.\n[derived]\ny = \"x * 2\"\n";
        let whole = source.replace("\n# Worked", "\n[totals]\n\n# Worked");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"]).unwrap(), whole);
        // At the start, two at one place in the five's order, and at the end of a last line
        // without its newline.
        let source = "version = 1\n\n# Cuts.\n[keep]\nk = \"x > 1\"\n\n[totals]\nsumw = \"w\"";
        let whole = "version = 1\n\n[define]\n\n# Cuts.\n[keep]\nk = \"x > 1\"\n\n[columns]\n\n[totals]\nsumw = \"w\"\n\n[derived]\n";
        assert_eq!(written(source, &source.parse().unwrap(), &["define", "columns", "derived"]).unwrap(), whole);
        let source = "format = \"rvr-recipe\"\nversion = 1\n";
        let whole = format!("{source}\n[define]\n\n[keep]\n\n[columns]\n\n[totals]\n\n[derived]\n");
        assert_eq!(written(source, &source.parse().unwrap(), &TABLES).unwrap(), whole);
        // A comment just under a key stays with its table; line endings stay the file's.
        let source = "[define]\n\n[keep]\n\n[columns]\nbranches = [\"x\"]\n# ht = \"sum(x)\"\n[derived]\n";
        let whole = source.replace("[derived]", "\n[totals]\n\n[derived]");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"]).unwrap(), whole);
        let source = &source.replace("\n# ht", "\n\n# ht");
        let whole = source.replace("[derived]", "\n[totals]\n\n[derived]");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"]).unwrap(), whole);
        assert_eq!((header("[ derived ]"), header("[derived] # read"), header("[derived.x]")), (Some("derived"), Some("derived"), None));
        let source = "[define]\r\n\r\n[keep]\r\n\r\n[columns]\r\nb = 1\r\n\r\n[derived]\r\n";
        let whole = source.replace("[derived]", "[totals]\r\n\r\n[derived]");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"]).unwrap(), whole);
    }

    #[test]
    fn a_place_that_would_change_the_recipe_is_refused() {
        // A line inside a string that reads as the next table's header.
        let source = "[define]\n\n[keep]\n\n[columns]\nnote = \"\"\"\n[derived]\n\"\"\"\n\n[derived]\n";
        let refused = written(source, &source.parse().unwrap(), &["totals"]).unwrap_err();
        assert_eq!(refused, "[totals] could not be written into recipe.toml in its place, so it is as it was: add it by hand.");
        assert_eq!(refusal(&["keep"]), "recipe.toml has no [keep] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty. Add the line [keep].");
    }

    #[test]
    fn the_offer_asks_alike_whatever_is_missing() {
        assert_eq!(asked(&["keep"], "[keep]", "continue pressing"), "recipe.toml has no [keep] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty.\nAdd [keep] to recipe.toml, empty and in its place, then continue pressing? [y/N] ");
        assert_eq!(asked(&["define", "keep", "derived"], "[define], [keep] and [derived]", "go on with the try"), "recipe.toml has no [define], [keep] or [derived] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty.\nAdd [define], [keep] and [derived] to recipe.toml, empty and in their places, then go on with the try? [y/N] ");
    }
}
