//! `rvr empty` and `rvr refill`: let a pressed file's columns go, keeping the file as the shell of
//! its study (its header, byte for byte), and press them again from that shell.

use crate::options::Options;
use crate::plan::{Asked, At, Choose, On, Place};
use crate::reproduce::{base, study_texts};
use crate::samples::some;
use rowver::study::lock::Lock;
use rowver::rvr::Reader;
use serde_json::Value;
use std::path::Path;

pub(crate) const USAGE: &str = "usage:
  rvr empty FILE... [--study FOLDER] [--anyway] [--force | -f]
      let each file's columns go, keeping its header: its study, every checksum, its signature;
      refused unless every input its lock names is as the lock recorded it, or with --anyway;
      asks first at a terminal, and needs --force (-f) elsewhere, or to empty without asking
  rvr refill FILE [--study FOLDER] [--workers N]
      press an empty file's columns again from the study it carries, checked against its
      checksums; a relative path in the lock is read from FOLDER (FILE's folder when not given)";

/// `rvr empty` with its options: the exit status, the worst of its files'.
pub fn empty_main(options: &Options) -> u8 {
    let force = options.has("--force");
    if !force && !crate::at_terminal() {
        return crate::report("empty", Err("nothing was emptied: rvr empty asks before it lets a file's columns go, and no one is at a terminal to answer. Give --force (or -f) to empty without asking.".into()));
    }
    let mut code = 0;
    for file in &options.words {
        code = code.max(crate::report("empty", empty(Path::new(file), options.value("--study").map(Path::new), options.has("--anyway"), force).map(|()| 0)));
    }
    code
}

/// `rvr refill` with its options: the exit status.
pub fn refill_main(options: &Options) -> u8 {
    let workers = match crate::options::workers(options) {
        Ok(workers) => workers,
        Err(why) => return crate::wrong("refill", &why),
    };
    crate::report("refill", refill(Path::new(&options.words[0]), options.value("--study").map(Path::new), workers).map(|()| 0))
}

/// Empty `file`, unless it has holes or, without `anyway`, an input differs from its lock; asked
/// first unless `force`.
fn empty(file: &Path, study: Option<&Path>, anyway: bool, force: bool) -> Result<(), String> {
    let shown = file.display();
    // No other writer changes it from before it is read until it is emptied, the question too
    // (SPEC §8.9, §8.12): the file it names, through a link. Its lock first, as every writer.
    let locked = crate::locked(file)?;
    crate::hold();
    let reader = locked.reader().map_err(|e| rowver::plain(file, &e))?;
    if reader.empty() {
        println!("{shown} is empty already.");
        return Ok(());
    }
    if reader.rows().map_err(|e| rowver::plain(file, &e))? == 0 {
        println!("{shown} has no rows, so its arrays hold no bytes: there is nothing to empty.");
        return Ok(());
    }
    // Emptying lets data go: only of a file you may write (a change that loses nothing is written anew).
    if let Some(kind) = locked.unwritable() {
        return Err(unwritable(file, kind));
    }
    // Staged only once it is to be emptied: a file that is not says so, wherever it lies.
    let held = crate::journal::Staged::lock(locked.path())?;
    let by = reader.header()["made"]["by"].as_str().unwrap_or("an unknown rvr").to_owned();
    let this = concat!("rvr ", env!("CARGO_PKG_VERSION"));
    if by != this && !anyway {
        return Err(format!("{shown} was not emptied: {by} pressed it, and a refill must press the same values again, which this {this} may not. Empty it with {by}, or with --anyway."));
    }
    let holes = rowver::rvr::holes(reader.header()).len();
    if holes > 0 {
        return Err(format!("{shown} has {}: a refill reads what its press read, and a hole may read otherwise later. Read them with rvr press --fill, or press its study whole, then empty it.", some(holes, "hole", "holes")));
    }
    if !anyway {
        let text = reader.header()["made"]["lock"]["text"].as_str().unwrap_or_default();
        let lock = Lock::read(text).map_err(|p| format!("{shown} was not emptied: the samples.lock it carries does not check out ({p}). Press its study again with rvr press --full, then empty that file."))?;
        let (changed, unasked) = crate::samples::differing(&lock, &base(file, study)?);
        if let Some(why) = unasked.first() {
            return Err(format!("{shown} was not emptied: its inputs could not all be checked ({why}). With --anyway it is emptied all the same."));
        }
        if !changed.is_empty() {
            let lines: String = changed.iter().map(|line| format!("\n  {line}")).collect();
            return Err(format!("{shown} was not emptied: rvr refill reads its inputs again, and {} as its lock recorded:{lines}\nWith --anyway it is emptied all the same, and cannot be refilled until they are back.", some(changed.len(), "is not", "are not")));
        }
    }
    // Written anew and moved over it, the file gives its bytes back only once no other link holds them.
    let others = links(locked.path()).saturating_sub(1);
    let (linked, until) = match others {
        0 => (String::new(), String::new()),
        1 => (" Its other hard link keeps its columns until it is removed.".to_owned(), " once its other hard link is removed".to_owned()),
        n => (format!(" Its {n} other hard links keep its columns until they are removed."), format!(" once its {n} other hard links are removed")),
    };
    if !force {
        let rows = reader.rows().map_err(|e| rowver::plain(file, &e))?;
        let columns = reader.header()["data"]["cols"].as_array().map_or(0, Vec::len);
        let size = std::fs::metadata(file).map_or(0, |m| m.len());
        let question = format!("Empty {shown} ({}, {}, {})? Its columns go and its header stays; rvr refill presses them again from the study it carries, reading its inputs again.{linked} [y/N] ", some(rows, "row", "rows"), some(columns, "column", "columns"), amount(size));
        let yes = crate::yes(&question).map_err(|why| format!("{shown} is as it was: {why}. Give --force (or -f) to empty it without asking."))?;
        if !yes {
            return Err(format!("{shown} is as it was: not emptied."));
        }
    }
    let signed = reader.header().get("signature").is_some();
    drop(reader);
    let freed = rowver::write::empty(locked).map_err(|e| rowver::plain(file, &e))?;
    drop(held);
    let kept = std::fs::metadata(file).map_or(0, |m| m.len());
    let signature = match signed {
        true => ", every checksum and its signature",
        false => " and every checksum (it is not signed)",
    };
    println!("Emptied {shown}: {} given back{until}; its header ({}) keeps its study{signature}. rvr refill {shown} presses its columns again.", amount(freed), amount(kept));
    Ok(())
}

/// How many hard links the file at `path` has (1 where that cannot be told).
fn links(path: &Path) -> u64 {
    #[cfg(unix)]
    return std::fs::metadata(path).map_or(1, |m| std::os::unix::fs::MetadataExt::nlink(&m));
    #[cfg(not(unix))]
    1
}

/// Why `rvr empty` refuses the file at `file`, which opening to be written met `kind`.
fn unwritable(file: &Path, kind: std::io::ErrorKind) -> String {
    let (is, fix) = rowver::change::unwritable_said(kind, file);
    format!("{} {is}: rvr empty gives back the space only of a file you may write ({fix} first)", file.display())
}

/// A number of bytes as a person reads it.
fn amount(bytes: u64) -> String {
    match bytes as f64 {
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b if b >= 1e6 => format!("{:.1} MB", b / 1e6),
        b if b >= 1e3 => format!("{:.1} kB", b / 1e3),
        _ => format!("{bytes} bytes"),
    }
}

/// Press the empty `file`'s columns again from its study, and put them in it when they are what
/// it records.
fn refill(file: &Path, study: Option<&Path>, workers: usize) -> Result<(), String> {
    let shown = file.display();
    // No other writer writes it while it is refilled (SPEC §7.5.2, §8.12): its lock first.
    let locked = crate::locked(file)?;
    crate::hold();
    let _held = crate::journal::Staged::lock(locked.path())?;
    // Beside the file it names, through a link, so that it is moved over it.
    let real = locked.path().to_path_buf();
    let shell = locked.reader().map_err(|e| rowver::plain(file, &e))?;
    if !shell.empty() {
        return Err(format!("{shown} holds its columns: there is nothing to refill."));
    }
    let (texts, name) = study_texts(shell.header());
    let base = base(file, study)?;
    let (staged, out) = (rowver::write::beside(&real, ".refill"), rowver::write::beside(&real, ".refilled"));
    if let Some(there) = [&staged, &out].into_iter().find(|p| p.exists()) {
        return Err(format!("{} is in the way: remove it, and refill again.", there.display()));
    }
    println!("Pressing the study {shown} carries again, into {}:", staged.display());
    let place = Place { base, study: name, target: staged.clone(), at: At::Missing, locked: None };
    // The copy pressed beside it is not signed: only its arrays are kept.
    let asked = Asked { choose: Choose::Recorded(shell.header()["made"]["files"].clone()), on: On::Once, workers, sign: false };
    let pressed = crate::plan::build(place, texts, asked).and_then(crate::core::press);
    let checked = pressed.and_then(|holes| settle(file, (&locked, &shell), &staged, &out, holes));
    let _ = std::fs::remove_file(&staged);
    let said = checked.map_err(|why| {
        let _ = std::fs::remove_file(&out);
        format!("{why}\n{shown} is still empty.")
    })?;
    // The file refilled is checked: when it cannot be put in place it stays, to be moved by hand;
    // but not over a file another program moved there, which it would replace.
    let synced = std::fs::File::open(&out).and_then(|f| f.sync_all()).map_err(rowver::Error::from);
    match synced.and_then(|()| locked.replace(&out)) {
        Ok(()) => {}
        Err(e @ rowver::Error::Invalid(_)) => {
            let _ = std::fs::remove_file(&out);
            return Err(e.to_string());
        }
        Err(rowver::Error::Io(e) | rowver::Error::File(_, e)) => {
            return Err(format!("{shown} could not be put in place ({}): the refilled file, checked, is at {}; mv '{}' '{shown}' puts it in place", rowver::described(&e), out.display(), out.display()));
        }
        Err(e) => return Err(e.to_string()),
    }
    println!("Refilled {shown}: {said}.");
    Ok(())
}

/// Write at `out` the file refilled with the arrays pressed into `staged`, when their values are what it records, checked; what was done, in words.
fn settle(file: &Path, (locked, shell): (&rowver::change::Locked, &Reader), staged: &Path, out: &Path, holes: usize) -> Result<String, String> {
    let shown = file.display();
    if holes > 0 {
        return Err(format!("pressed again, it has {} its press did not have: an input could not be read, as the holes listed above say. Their fixes are a press's: for a refill, put each input back as its lock recorded it, then run rvr refill {shown} again.", some(holes, "hole", "holes")));
    }
    let pressed = Reader::open(staged).map_err(|e| rowver::plain(staged, &e))?;
    let (was, now) = (&shell.header()["data"], &pressed.header()["data"]);
    // The same bytes, wherever this press laid them: each is put back where the file records it.
    let said = if without(was, &["at"]) == without(now, &["at"]) {
        rowver::write::refilled(locked, staged, out).map_err(|e| e.to_string())?;
        let signer = shell.header().get("signature").map(|s| crate::key::whose(s["key"].as_str().unwrap_or_default()));
        match signer {
            Some(whose) => format!("every column is back, byte for byte as it was, and its signature, by {whose}, is as it was"),
            None => "every column is back, byte for byte as it was; it is not signed".into(),
        }
    } else if placeless(was) == placeless(now) {
        // The same values compressed otherwise, as by another rvr's zstd: placed anew, signed again.
        let mut header = shell.header().clone();
        header["data"] = now.clone();
        if let Some(root) = header.as_object_mut() {
            root.shift_remove("signature");
        }
        let signer = crate::key::secret()?;
        crate::derive::rewritten(staged, out, &mut header, signer.as_ref())?;
        let was = shell.header().get("signature").map(|s| s["key"].as_str().unwrap_or_default().to_owned());
        let signed = crate::key::resigned(signer.as_ref(), was.as_deref());
        format!("every column's values are as its checksums say; this rvr compresses them otherwise, so its header is written again with their new places, {signed}")
    } else {
        return Err(format!("pressed again, {} not what it records: its inputs, or the rvr that reads them, have changed since it was pressed. rvr reproduce {shown} presses its study into a file of its own and compares the two.", differing(was, now).join(", ")));
    };
    match rowver::rvr::validate_file(out) {
        Ok(summary) if !summary.empty => {}
        Ok(_) => return Err("the refilled file holds no arrays".into()),
        Err(e @ (rowver::Error::Io(_) | rowver::Error::File(..))) => return Err(rowver::plain(out, &e)),
        Err(e) => return Err(format!("{}, the file refilled, does not check out: {e}", out.display())),
    }
    Ok(said)
}

/// A data section without where its arrays lie: what two presses of the same values share.
fn placeless(value: &Value) -> Value {
    without(value, &["at", "size"])
}

/// `value` without the fields named `keys`, at any depth.
fn without(value: &Value, keys: &[&str]) -> Value {
    match value {
        Value::Object(fields) => fields.iter().filter(|(key, _)| !keys.contains(&key.as_str())).map(|(key, value)| (key.clone(), without(value, keys))).collect(),
        Value::Array(items) => items.iter().map(|item| without(item, keys)).collect(),
        other => other.clone(),
    }
}

/// What differs between two data sections, by name: their rows, samples, files, arrays or columns.
fn differing(was: &Value, now: &Value) -> Vec<String> {
    let mut names: Vec<String> = ["rows", "samples", "files", "sample", "entry", "sample_cols"]
        .into_iter()
        .filter(|key| placeless(&was[*key]) != placeless(&now[*key]))
        .map(|key| format!("its {key}"))
        .collect();
    let columns = |data: &Value| data["cols"].as_array().cloned().unwrap_or_default();
    let now_columns = columns(now);
    for column in columns(was) {
        let again = now_columns.iter().find(|c| c["name"] == column["name"]);
        if again.is_none_or(|again| placeless(again) != placeless(&column)) {
            names.push(column["name"].as_str().unwrap_or_default().to_owned());
        }
    }
    match names.as_slice() {
        [] => vec!["its data section".into()],
        [one] => vec![format!("{one} is")],
        _ => vec![format!("{} are", names.join(", "))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // SPEC 8.9
    #[test]
    fn a_file_on_a_read_only_filesystem_is_told_how_it_may_be_emptied_and_not_to_chmod_it() {
        let said = unwritable(Path::new("f.rvr"), std::io::ErrorKind::ReadOnlyFilesystem);
        assert_eq!(said, "f.rvr lies on a read-only filesystem: rvr empty gives back the space only of a file you may write (mount it to be written first)");
        let said = unwritable(Path::new("f.rvr"), std::io::ErrorKind::PermissionDenied);
        assert_eq!(said, "f.rvr is read-only: rvr empty gives back the space only of a file you may write (chmod u+w f.rvr first)");
    }

    // SPEC 8.10
    #[test]
    fn a_data_section_pressed_again_is_the_same_placed_otherwise_or_named_where_it_differs() {
        let was = json!({"rows": 2, "sample": {"at": 0, "size": 9, "sha256": "a"}, "cols": [{"name": "MET", "at": 9, "size": 5, "sha256": "b"}]});
        let moved = json!({"rows": 2, "sample": {"at": 0, "size": 8, "sha256": "a"}, "cols": [{"name": "MET", "at": 8, "size": 6, "sha256": "b"}]});
        assert_ne!(was, moved);
        assert_eq!(placeless(&was), placeless(&moved));
        let changed = json!({"rows": 2, "sample": {"at": 0, "size": 9, "sha256": "a"}, "cols": [{"name": "MET", "at": 9, "size": 5, "sha256": "c"}]});
        assert_eq!(differing(&was, &changed), ["MET is"]);
        assert_eq!(differing(&was, &json!({"rows": 3, "cols": []})), ["its rows, its sample, MET are"]);
    }
}
