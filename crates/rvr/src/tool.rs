//! `rvr tool FILE`: list a file's tool keys, or set, write out or drop one, in the file itself
//! (docs/SPEC.md §8.11).

use crate::options::Options;
use rowver::change::Change;
use rowver::rvr::Reader;
use serde_json::Value;
use std::path::Path;

pub(crate) const USAGE: &str = "usage:
  rvr tool FILE                                 the file's tool keys, with their sizes
  rvr tool FILE KEY --set DATA.json [--force | -f]
      make the JSON object DATA.json holds the tool key KEY, in the file itself; one there is
      replaced once you say so, or with --force
  rvr tool FILE KEY --get [OUT.json] [--force | -f]
      KEY's object, to OUT.json (written over once you say so, or with --force) or standard output
  rvr tool FILE KEY... --drop [--force | -f]    drop KEY (or each KEY named), once you say so, or with --force";

/// `rvr tool` with its options: the exit status (2 for a wrong use of it).
pub fn main(options: &Options) -> u8 {
    let words = options.plain();
    let acts: Vec<&str> = ["--set", "--get", "--drop"].into_iter().filter(|flag| options.has(flag)).collect();
    // A word with a dash is a key's name right after FILE, and after it the name of another to drop: anywhere else, a flag mistyped.
    if let Some(word) = words.iter().enumerate().find(|(at, word)| (*at == 0 || (*at > 1 && acts != ["--drop"])) && crate::options::short(word)).map(|(_, word)| word) {
        return crate::wrong("tool", &format!("{word} is not one of its flags (a tool key whose name begins with - is named right after FILE)"));
    }
    let (file, force) = (Path::new(words[0]), options.has("--force"));
    // -f right after FILE is the key's name (SPEC §5.1.2): said where the force was meant.
    let meant = match words.get(1) {
        Some(&"-f") => " (-f right after FILE is taken as KEY: give it after KEY, or write --force)",
        _ => "",
    };
    let done = match (words.get(1), acts.as_slice(), words.len()) {
        (None, [], _) => list(file),
        (Some(key), ["--set"], 2) => set(file, key, Path::new(options.value("--set").unwrap_or_default()), force),
        (Some(key), ["--get"], 2 | 3) => get(file, key, words.get(2).map(Path::new), force),
        (Some(_), ["--drop"], _) => {
            // A key named twice is dropped once.
            let keys: Vec<&str> = words[1..].iter().enumerate().filter(|(at, key)| !words[1..*at + 1].contains(key)).map(|(_, key)| *key).collect();
            dropped(file, &keys, force)
        }
        (None, _, _) => return crate::wrong("tool", "give the tool key's name, KEY, before --set, --get or --drop"),
        (Some(_), [], _) => return crate::wrong("tool", &format!("give what to do with KEY: --set DATA.json, --get [OUT.json] or --drop{meant}")),
        (Some(_), [_, _, ..], _) => return crate::wrong("tool", "give one of --set, --get and --drop"),
        (Some(_), _, _) => return crate::wrong("tool", &format!("only --get takes a word after KEY, the file it writes, and --drop more keys{meant}")),
    };
    // Said when the file has no key -f: the force was meant.
    let keyless = || rowver::rvr::header_text(file).ok().and_then(|text| rowver::rvr::tool_texts(&text).ok()).is_some_and(|keys| keys.iter().all(|(key, _)| key != "-f"));
    crate::report("tool", done.map_err(|why| if !meant.is_empty() && keyless() { format!("{why}{meant}") } else { why }).map(|()| 0))
}

/// `text` in double quotes as Python reads a string: `"` and `\` escaped, each character outside printable ASCII by its code (`\u202e`, `\U0001f600`).
fn quoted(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' | '\\' => out.extend(['\\', c]),
            ' '..='~' => out.push(c),
            c if u32::from(c) <= 0xffff => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push_str(&format!("\\U{:08x}", u32::from(c))),
        }
    }
    out + "\""
}

/// Whether `key` holds a character outside printable ASCII, which a terminal may hide or turn about.
fn unplain(key: &str) -> bool {
    !key.chars().all(|c| matches!(c, ' '..='~'))
}

/// `key` as it is shown: as it is, or `quoted` when it holds a character outside printable ASCII.
fn seen(key: &str) -> String {
    match unplain(key) {
        true => quoted(key),
        false => key.to_owned(),
    }
}

/// The tool keys of the header `text` of `file`, by name, each with its object's bytes as they lie there.
fn keys(file: &Path, text: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let spans = rowver::rvr::tool_texts(text).map_err(|e| rowver::plain(file, &e))?;
    Ok(spans.into_iter().map(|(key, span)| (key, text[span].to_vec())).collect())
}

/// The bytes of the tool key `key`'s object, as the header `text` of `file` holds them: refused when it holds more than one.
fn held(file: &Path, text: &[u8], key: &str) -> Result<Vec<u8>, String> {
    let mut copies: Vec<Vec<u8>> = keys(file, text)?.into_iter().filter(|(name, _)| name == key).map(|(_, bytes)| bytes).collect();
    match copies.len() {
        0 => Err(absent(file, key)),
        1 => Ok(copies.remove(0)),
        n => Err(format!("{} holds the tool key {} {n} times, which FORMAT.md §1 refuses, so rvr tool cannot write out one: rowver.drop_tool({}, {}) in Python drops it and gives back every copy", file.display(), seen(key), quoted(&file.display().to_string()), quoted(key))),
    }
}

/// The header text of the file at `file`, and why the header keeps the file from opening, when it does.
fn opened(file: &Path) -> Result<(Vec<u8>, Option<rowver::Error>), String> {
    let refused = match Reader::open(file).and_then(|reader| reader.header_text()) {
        Ok(text) => return Ok((text, None)),
        Err(e) => e,
    };
    // A header a key breaks is read as it lies, so that its keys are listed, written out and dropped.
    let text = rowver::rvr::header_text(file).map_err(|_| rowver::plain(file, &refused))?;
    match rowver::rvr::checked_header(&text) {
        Err(why) => Ok((text, Some(why))),
        Ok(_) => Err(rowver::plain(file, &refused)),
    }
}

/// How many bytes a tool key's object will take in a header: its JSON, written compactly.
fn size(value: &Value) -> usize {
    serde_json::to_string(value).map_or(0, |text| text.len())
}

/// List the file's tool keys, each with its size.
fn list(file: &Path) -> Result<(), String> {
    let (text, broken) = opened(file)?;
    let keys = keys(file, &text)?;
    let shown = file.display();
    match keys.len() {
        0 => println!("{shown} has no tool keys."),
        n => {
            println!("{shown} has {}:", crate::samples::some(n, "tool key", "tool keys"));
            for (key, bytes) in &keys {
                println!("  {} ({} bytes)", seen(key), bytes.len());
            }
        }
    }
    // A header that does not open, said after its keys, with the key whose drop alone lets it open,
    // else whether dropping its keys would; a §1 break under the format's own keys is no key's.
    let Some(why) = broken else {
        return Ok(());
    };
    let (said, under) = match &why {
        rowver::Error::Breaks { fact, under, .. } => (fact.clone(), under.clone()),
        rowver::Error::Tool { fact, key } => (fact.clone(), Some(key.clone())),
        other => (other.to_string(), None),
    };
    // A key held more than once is one name.
    let mut names: Vec<&str> = Vec::new();
    for (name, _) in &keys {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    let elsewhere = under.is_some_and(|key| !names.contains(&key.as_str()));
    let opens = |without: &[&str]| !elsewhere && rowver::change::opens_without(&text, without);
    // The command that drops `keys`: rvr tool's, which names a key beginning with -- or named -f
    // only right after FILE, and one spelled as its flags in full not at all (SPEC §5.1.2); else Python's.
    let command = |mut keys: Vec<&str>| {
        let flag = keys.iter().find(|key| ["--set", "--get", "--drop", "--force"].contains(&key.split_once('=').map_or(**key, |(flag, _)| flag)));
        let first = |key: &&str| key.starts_with("--") || *key == "-f";
        let unseen = keys.iter().find(|key| unplain(key));
        if flag.is_some() || unseen.is_some() || keys.iter().filter(|key| first(key)).count() > 1 {
            let why = match (flag, unseen) {
                (Some(key), _) => format!("rvr tool reads {key} as its flag"),
                (_, Some(key)) => format!("{} holds a character outside printable ASCII", seen(key)),
                _ => "rvr tool names only one key beginning with -- or named -f, right after FILE".to_owned(),
            };
            let quoted: Vec<String> = [shown.to_string().as_str()].into_iter().chain(keys.iter().copied()).map(quoted).collect();
            return format!("rowver.drop_tool({}) in Python ({why})", quoted.join(", "));
        }
        keys.sort_by_key(|key| !first(key));
        let keys: Vec<String> = keys.iter().map(|key| rowver::shell(key)).collect();
        format!("rvr tool {} {} --drop", rowver::shell(&shown.to_string()), keys.join(" "))
    };
    match names.iter().find(|name| opens(&[name])) {
        Some(key) => println!("{shown} does not open ({said}): {} takes out the key that breaks it", command(vec![key])),
        None if names.len() > 1 && opens(&names) => {
            // Each key the header does not open without dropping; every key, when those alone would not do.
            let needed: Vec<&str> = names.iter().copied().filter(|key| !opens(&names.iter().copied().filter(|other| other != key).collect::<Vec<_>>())).collect();
            let drop = if opens(&needed) { needed } else { names.clone() };
            println!("{shown} does not open ({said}): more than one of its tool keys breaks it, and {} takes them out", command(drop))
        }
        None => println!("{shown} does not open ({said})"),
    }
    Ok(())
}

/// Ask `question` at a terminal, or, with nobody there, refuse unless `force`: whether to go on.
fn asked(force: bool, question: &str, nobody: &str) -> Result<bool, String> {
    match (force, crate::at_terminal()) {
        (true, _) => Ok(true),
        (false, false) => Err(nobody.into()),
        (false, true) => crate::yes(question),
    }
}

/// Make DATA's object the tool key `key` of the file, asking first when it replaces one.
fn set(file: &Path, key: &str, data: &Path, force: bool) -> Result<(), String> {
    let text = std::fs::read(data).map_err(|e| rowver::plain_io(data, &e))?;
    // Only data that would be taken is asked about, its size as the header would hold it: a name
    // refused is told against the file, data refused against DATA.json.
    let value = rowver::change::tool_data(key, &text).map_err(|e| rowver::plain(if rowver::rvr::tool_key(key).is_ok() { data } else { file }, &e))?;
    let new = size(&value);
    let locked = crate::locked(file)?;
    // Its header read once, under the lock, for the change and for the question; the new one checked before it.
    let (change, header) = Change::setting(&locked, key, value).map_err(|e| rowver::plain(file, &e))?;
    // A key held twice (written by another program) is replaced whole: every copy counted.
    let held: Vec<usize> = keys(file, change.text())?.into_iter().filter(|(name, _)| name == key).map(|(_, bytes)| bytes.len()).collect();
    let was = (!held.is_empty()).then(|| held.iter().sum::<usize>());
    let shown = file.display();
    if let Some(was) = was {
        let question = format!("Replace the tool key {key} of {shown} ({was} bytes) with {} ({new} bytes)? [y/N] ", data.display());
        let nobody = format!("nothing was set: {key} is a tool key of it already, and rvr tool asks before it replaces one, and no one is at a terminal to answer. Give --force (or -f) to replace it without asking.");
        if !asked(force, &question, &nobody)? {
            return Err(format!("{shown} is as it was: {key} not replaced."));
        }
    }
    change.finish(&header).map_err(|e| rowver::plain(file, &e))?;
    crate::anew(file, &locked);
    match was {
        None => println!("Set the tool key {key} of {shown} ({new} bytes)."),
        Some(was) => println!("Replaced the tool key {key} of {shown} ({new} bytes; it held {was})."),
    }
    Ok(())
}

/// Write the tool key `key`'s object as the header holds its bytes, and a newline, to `out`
/// (asking first when it is there), or to standard output.
fn get(file: &Path, key: &str, out: Option<&Path>, force: bool) -> Result<(), String> {
    let bytes = held(file, &opened(file)?.0, key)?;
    let n = bytes.len();
    let text = String::from_utf8_lossy(&bytes).into_owned() + "\n";
    let Some(out) = out else {
        return crate::out("tool", text.as_bytes());
    };
    if rowver::write::same_file(out, file) {
        return Err(format!("{} is {} itself: write the key's object to another file", out.display(), file.display()));
    }
    // A file there, perhaps edited by hand, is written over only once you say so.
    if let Ok(there) = std::fs::metadata(out) {
        let question = format!("Write the tool key {key} of {} ({n} bytes) over {} ({} bytes)? [y/N] ", file.display(), out.display(), there.len());
        let nobody = format!("nothing was written: {} is there already, and rvr tool asks before it writes over a file, and no one is at a terminal to answer. Give --force (or -f) to write over it without asking.", out.display());
        if !asked(force, &question, &nobody)? {
            return Err(format!("{} is as it was: the tool key {key} not written to it.", out.display()));
        }
    }
    crate::tables::put(out, &text).map_err(|e| rowver::plain_io(out, &e))?;
    println!("Wrote the tool key {key} of {} to {} ({n} bytes).", file.display(), out.display());
    Ok(())
}

/// What a file without the tool key `key` is told.
fn absent(file: &Path, key: &str) -> String {
    format!("{} has no tool key {key}: rvr tool {} lists its tool keys", file.display(), rowver::shell(&file.display().to_string()))
}

/// Drop the tool keys `keys` in one change, asking first: ones that break the header's rules too.
fn dropped(file: &Path, keys: &[&str], force: bool) -> Result<(), String> {
    let shown = file.display();
    let locked = crate::locked(file)?;
    // Their bytes found unparsed, so that keys the header's rules refuse are dropped, and the header left checked, before it asks.
    let change = Change::dropping(&locked, keys).map_err(|e| rowver::plain(file, &e))?;
    // A key held more than once is all its copies' bytes.
    let sizes: Vec<usize> = change.dropped().iter().map(|copies| copies.iter().map(|bytes| bytes.len()).sum()).collect();
    let held: Vec<String> = keys.iter().zip(&sizes).map(|(key, size)| format!("{} ({size} bytes)", seen(key))).collect();
    let named = match keys {
        [key] => format!("the tool key {} of {shown} ({} bytes)", seen(key), sizes[0]),
        _ => format!("the tool keys {} of {shown}", held.join(", ")),
    };
    let question = format!("Drop {named}? [y/N] ");
    let nobody = "nothing was dropped: rvr tool asks before it drops a tool key, and no one is at a terminal to answer. Give --force (or -f) to drop it without asking.";
    if !asked(force, &question, nobody)? {
        return Err(format!("{shown} is as it was: {} not dropped.", keys.join(", ")));
    }
    let header = change.header().clone();
    change.finish(&header).map_err(|e| rowver::plain(file, &e))?;
    crate::anew(file, &locked);
    println!("Dropped {named}.");
    Ok(())
}
