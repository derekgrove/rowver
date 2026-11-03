//! The tables a study's TOML files write even when empty (a recipe's five, samples.toml's three):
//! which ones a file leaves out, and, asked at a terminal, writing them in, empty and in their
//! place.

use std::path::Path;

pub use rowver::rvr::RECIPE_TABLES as TABLES;

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
    let text = match written(source, &root, &missing, &TABLES, "recipe.toml") {
        Ok(text) => text,
        Err(why) => {
            eprintln!("{why}");
            return Ok(false);
        }
    };
    let named = listed(&missing, "and");
    let by_hand = format!("add {named} to it");
    ask_and_write(&dir.join("recipe.toml"), source, &text, &asked(&missing, &named, going), &by_hand, then)?;
    eprintln!("Added {named} to recipe.toml.");
    Ok(true)
}

/// Ask `question`, and on a yes write `text` over the file at `path`, which read `source`; refused
/// on a no, or when rvr cannot ask, the file as it was and `by_hand`, then `then`, the way on.
pub fn ask_and_write(path: &Path, source: &str, text: &str, question: &str, by_hand: &str, then: &str) -> Result<(), String> {
    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let unchanged = |why: String| Err(format!("{why}, so {name} is as it was: {by_hand}, then {then} again."));
    match crate::yes(question) {
        Err(why) => unchanged(why),
        Ok(false) => unchanged("not written in".into()),
        Ok(true) => rewrite(path, source, text, then, by_hand),
    }
}

/// Write `text` over the file at `path`, which read `source` when rvr asked: beside the file (the
/// one a link names) and renamed over it, so that a failed write leaves it as it was, with its
/// permissions; refused, in a sentence of its own, when it was edited while rvr asked, saying to
/// `then` again, or when it cannot be written, saying to do `by_hand` first.
pub fn rewrite(path: &Path, source: &str, text: &str, then: &str, by_hand: &str) -> Result<(), String> {
    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let path = path.canonicalize().map_err(|e| format!("{name}: {e}"))?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(source) {
        return Err(format!("{name} changed while rvr asked, so it is as you saved it: {then} again."));
    }
    put(&path, text).map_err(|e| format!("{name} could not be written ({e}), so it is as it was: {by_hand}, then {then} again."))
}

/// Write `text` as the file at `path` (the one a link names), whole or not at all: beside it, with
/// its permissions when it is there, and renamed over it.
pub fn put(path: &Path, text: &str) -> std::io::Result<()> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let staged = crate::empty::beside(&path, ".rvr-new");
    let wrote = std::fs::write(&staged, text)
        .and_then(|()| match std::fs::metadata(&path) {
            Ok(was) => std::fs::set_permissions(&staged, was.permissions()),
            Err(_) => Ok(()),
        })
        .and_then(|()| std::fs::File::open(&staged)?.sync_all())
        .and_then(|()| std::fs::rename(&staged, &path));
    if wrote.is_err() {
        _ = std::fs::remove_file(&staged);
    }
    wrote
}

/// `source` (read as `root`) with each of `missing` written in, empty, before the next of `order`
/// (the tables in their order, `[[name]]` ones too) it has, else at the end; refused unless it reads
/// as `source` did with only those tables added. `file` names it in the refusal.
pub fn written(source: &str, root: &toml::Table, missing: &[&str], order: &[&str], file: &str) -> Result<String, String> {
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
        let Some(table) = opens(line, order).filter(|table| !begins.iter().any(|(name, _)| name == table)) else { continue };
        let first = (0..index).rev().take_while(|&above| lines[above].1.starts_with('#')).last().unwrap_or(index);
        let keyed = lines[first..index].iter().any(|(_, comment)| unkeyed(comment));
        let own = (first == 0 || lines[first - 1].1.is_empty()) && !keyed;
        begins.push((table, if own { lines[first].0 } else { *at }));
    }
    let mut places: Vec<(usize, &str)> = order
        .iter()
        .enumerate()
        .filter(|(_, table)| missing.contains(table))
        .map(|(index, table)| {
            let next = order[index + 1..].iter().find_map(|later| begins.iter().find(|(name, _)| name == later));
            (next.map_or(source.len(), |(_, at)| *at), *table)
        })
        .collect();
    // Stable: tables written at one place keep their order.
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
        _ => Err(format!("{} could not be written into {file} in {}, so it is as it was: add {} by hand.", listed(missing, "and"), place(missing), if missing.len() == 1 { "it" } else { "them" })),
    }
}

/// The tables written `[[name]]`, one per entry: samples.toml's samples.
const ARRAYS: [&str; 1] = ["sample"];

/// The one of `order` a line (trimmed) heads, as `[keep]` or `[[sample]]` (each with its own brackets).
pub fn header<'a>(line: &str, order: &[&'a str]) -> Option<&'a str> {
    let (name, rest, doubled) = match line.strip_prefix("[[").and_then(|rest| rest.split_once("]]")) {
        Some((name, rest)) => (name, rest, true),
        None => line.strip_prefix('[')?.split_once(']').map(|(name, rest)| (name, rest, false))?,
    };
    let (name, rest) = (name.trim(), rest.trim_start());
    (rest.is_empty() || rest.starts_with('#')).then_some(())?;
    (ARRAYS.contains(&name) == doubled).then_some(())?;
    order.iter().copied().find(|table| *table == name)
}

/// The one of `order` a line begins: its header, or one of its own tables (`[eras.UL18]` begins `[eras]`).
fn opens<'a>(line: &str, order: &[&'a str]) -> Option<&'a str> {
    header(line, order).or_else(|| {
        let name = line.strip_prefix('[').filter(|rest| !rest.starts_with('['))?.split_once('.')?.0.trim();
        order.iter().copied().find(|table| *table == name && !ARRAYS.contains(table))
    })
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
pub fn listed(tables: &[&str], word: &str) -> String {
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
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"], &TABLES, "recipe.toml").unwrap(), whole);
        // At the start, two at one place in the five's order, and at the end of a last line
        // without its newline.
        let source = "version = 1\n\n# Cuts.\n[keep]\nk = \"x > 1\"\n\n[totals]\nsumw = \"w\"";
        let whole = "version = 1\n\n[define]\n\n# Cuts.\n[keep]\nk = \"x > 1\"\n\n[columns]\n\n[totals]\nsumw = \"w\"\n\n[derived]\n";
        assert_eq!(written(source, &source.parse().unwrap(), &["define", "columns", "derived"], &TABLES, "recipe.toml").unwrap(), whole);
        let source = "format = \"rvr-recipe\"\nversion = 1\n";
        let whole = format!("{source}\n[define]\n\n[keep]\n\n[columns]\n\n[totals]\n\n[derived]\n");
        assert_eq!(written(source, &source.parse().unwrap(), &TABLES, &TABLES, "recipe.toml").unwrap(), whole);
        // A comment just under a key stays with its table; line endings stay the file's.
        let source = "[define]\n\n[keep]\n\n[columns]\nbranches = [\"x\"]\n# ht = \"sum(x)\"\n[derived]\n";
        let whole = source.replace("[derived]", "\n[totals]\n\n[derived]");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"], &TABLES, "recipe.toml").unwrap(), whole);
        let source = &source.replace("\n# ht", "\n\n# ht");
        let whole = source.replace("[derived]", "\n[totals]\n\n[derived]");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"], &TABLES, "recipe.toml").unwrap(), whole);
        assert_eq!((header("[ derived ]", &TABLES), header("[derived] # read", &TABLES), header("[derived.x]", &TABLES)), (Some("derived"), Some("derived"), None));
        assert_eq!(header("[[sample]]", &["places", "sample"]), Some("sample"));
        assert_eq!((header("[[derived]]", &TABLES), header("[sample]", &["sample"])), (None, None));
        let order = ["places", "defaults", "eras", "sample"];
        let source = "[eras.UL18]\nera = 3\n\n[[sample]]\nname = \"a\"\n";
        let text = written(source, &source.parse().unwrap(), &["places", "defaults"], &order, "samples.toml").unwrap();
        assert!(text.starts_with("[places]\n\n[defaults]\n\n[eras.UL18]"), "{text}");
        let source = "[define]\r\n\r\n[keep]\r\n\r\n[columns]\r\nb = 1\r\n\r\n[derived]\r\n";
        let whole = source.replace("[derived]", "[totals]\r\n\r\n[derived]");
        assert_eq!(written(source, &source.parse().unwrap(), &["totals"], &TABLES, "recipe.toml").unwrap(), whole);
    }

    #[test]
    fn a_place_that_would_change_the_recipe_is_refused() {
        // A line inside a string that reads as the next table's header.
        let source = "[define]\n\n[keep]\n\n[columns]\nnote = \"\"\"\n[derived]\n\"\"\"\n\n[derived]\n";
        let refused = written(source, &source.parse().unwrap(), &["totals"], &TABLES, "recipe.toml").unwrap_err();
        assert_eq!(refused, "[totals] could not be written into recipe.toml in its place, so it is as it was: add it by hand.");
        assert_eq!(refusal(&["keep"]), "recipe.toml has no [keep] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty. Add the line [keep].");
    }

    #[test]
    fn the_offer_asks_alike_whatever_is_missing() {
        assert_eq!(asked(&["keep"], "[keep]", "continue pressing"), "recipe.toml has no [keep] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty.\nAdd [keep] to recipe.toml, empty and in its place, then continue pressing? [y/N] ");
        assert_eq!(asked(&["define", "keep", "derived"], "[define], [keep] and [derived]", "go on with the try"), "recipe.toml has no [define], [keep] or [derived] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty.\nAdd [define], [keep] and [derived] to recipe.toml, empty and in their places, then go on with the try? [y/N] ");
    }
}
