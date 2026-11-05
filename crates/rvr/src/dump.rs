//! `rvr dump`: the inside of a file, the header in full, the study's texts, then every column as
//! text: its first values, or with `--all` every value, one row a line.

use rowver::rvr::{push_shown, width, Held, Reader};
use serde_json::Value;

/// What a column record of `data.cols` the file does not hold (a damaged header, as one with no name) is said as.
fn unheld(reader: &rowver::rvr::Reader, name: &str) -> String {
    format!("{}: data.cols records a column {name:?}, which the file does not hold: its header is damaged; copy it again, or press its study again", reader.path().display())
}

/// The first `n` values of little-endian bytes of a kind, as text, ending in `...` when there are more.
fn first(kind: &str, bytes: &[u8], n: usize) -> String {
    let w = width(kind).unwrap_or(1);
    let mut out = String::from("[");
    let _ = push_shown(kind, &bytes[..bytes.len().min(n * w)], &mut out);
    out.push_str(if bytes.len() / w > n { ", ...]" } else { "]" });
    out
}

/// One line for a column of `rows` rows, `held` its first ones: name, `wide` characters at least,
/// kind, length, first values; for a derived column, `derived` and its expression, too.
fn show(name: &str, wide: usize, held: &Held, rows: usize, expression: Option<&str>) {
    let w = width(&held.kind).unwrap_or(1);
    let derived = match expression {
        Some(expression) => format!("derived = {expression:?}  "),
        None => String::new(),
    };
    match &held.offsets {
        None => {
            let shown = first(&held.kind, &held.bytes, 5);
            let shown = if rows > held.bytes.len() / w { shown.replacen(']', ", ...]", 1) } else { shown };
            println!("{name:wide$} {}[{rows}]  {derived}{shown}", held.kind);
        }
        Some(offsets) => {
            let lists: Vec<String> = offsets.windows(2).take(3).map(|r| first(&held.kind, &held.bytes[r[0] * w..r[1] * w], 4)).collect();
            let more = if rows > 3 { ", ..." } else { "" };
            println!("{name:wide$} jagged {}[{rows}]  {derived}[{}{more}]", held.kind, lists.join(", "));
        }
    }
}

/// A column's value in one row written to `out`: a value, or a list `[a, b]`.
pub(crate) fn push_row(held: &Held, row: usize, out: &mut String) {
    let w = width(&held.kind).unwrap_or(1);
    let (from, to, listed) = match &held.offsets {
        None => (row, row + 1, false),
        Some(offsets) => (offsets[row], offsets[row + 1], true),
    };
    if listed {
        out.push('[');
    }
    let _ = push_shown(&held.kind, held.bytes.get(from * w..to * w).unwrap_or_default(), out);
    if listed {
        out.push(']');
    }
}

/// One of the study's texts between two marker lines.
fn print_source(source: &str, name: &str) {
    println!("##### begin {name} #####");
    print!("{source}");
    if !source.ends_with('\n') {
        println!();
    }
    println!("##### end {name} #####");
}

/// Names line up in a column as wide as the longest of a file's columns, and never narrower than 14.
fn wide(reader: &Reader) -> usize {
    let data = &reader.header()["data"];
    let named = |key: &str| data[key].as_array().into_iter().flatten().filter_map(|r| r["name"].as_str()).map(|n| n.chars().count()).collect::<Vec<_>>();
    let derived = rowver::rvr::derived::listed(reader.header());
    named("cols").into_iter().chain(named("sample_cols")).chain(derived.iter().map(|(n, _)| n.chars().count())).fold(14, usize::max)
}

/// The samples' names, in row order.
fn sample_names(data: &Value) -> Vec<&str> {
    data["samples"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).collect()
}

/// Each sample column: its name, kind and every sample's value.
fn print_sample_columns(data: &Value, wide: usize) {
    let names = sample_names(data);
    for record in data["sample_cols"].as_array().into_iter().flatten() {
        let values: Vec<String> = names.iter().zip(record["values"].as_array().into_iter().flatten()).map(|(n, v)| format!("{n}: {v}")).collect();
        let (name, kind) = (record["name"].as_str().unwrap_or_default(), record["kind"].as_str().unwrap_or_default());
        println!("{name:wide$} {kind} by sample[{}]  {}", names.len(), values.join(", "));
    }
}

/// What an empty file's columns say.
const NO_COLUMNS: &str = "No columns: the file is empty (rvr empty let them go); rvr refill presses them again from the study it carries.";

/// The columns, each with its kind, length and first values; then the sample columns.
fn print_columns(reader: &Reader) -> Result<(), String> {
    if reader.empty() {
        println!("{NO_COLUMNS}");
        return Ok(());
    }
    let data = &reader.header()["data"];
    let failed = |e: rowver::Error| rowver::plain(reader.path(), &e);
    let rows = reader.rows().map_err(failed)?;
    // Each column's first rows alone are read: as many as its first values shown.
    let few = rows.min(5);
    let sample: Vec<u8> = reader.sample_slice(0, few).map_err(failed)?.iter().flat_map(|x| x.to_le_bytes()).collect();
    let entry: Vec<u8> = reader.entry_slice(0, few).map_err(failed)?.iter().flat_map(|x| x.to_le_bytes()).collect();
    let wide = wide(reader);
    let more = |shown: &str| if rows > few { shown.replacen(']', ", ...]", 1) } else { shown.to_owned() };
    println!("{:wide$} i4[{rows}]  {}", "sample", more(&first("i4", &sample, 5)));
    println!("{:wide$} i8[{rows}]  {}", "entry", more(&first("i8", &entry, 5)));
    for record in data["cols"].as_array().into_iter().flatten() {
        let name = record["name"].as_str().unwrap_or_default();
        let held = reader.slice(name, 0, few).map_err(failed)?.ok_or_else(|| unheld(reader, name))?;
        show(name, wide, &held, rows, None);
    }
    // The derived columns are checked a run at a time, then their first rows worked out; when the
    // check fails, each one alone, to say which cannot be.
    let derived = rowver::rvr::derived::listed(reader.header());
    let names: Vec<String> = derived.iter().map(|(name, _)| name.clone()).collect();
    let checked = rowver::rvr::derived::checked(reader).and_then(|_| rowver::rvr::derived::slice(reader, &names, 0, few));
    for (at, (name, expression)) in derived.iter().enumerate() {
        let alone;
        let held = match &checked {
            Ok(held) => Ok(&held[at]),
            Err(_) => {
                alone = rowver::rvr::derived::columns(reader, std::slice::from_ref(name)).and_then(|whole| whole[0].rows(0, few));
                alone.as_ref()
            }
        };
        match held {
            Ok(held) => show(name, wide, held, rows, Some(expression)),
            Err(why) => println!("{name:wide$} derived = {expression:?}  (not worked out: {why})"),
        }
    }
    print_sample_columns(data, wide);
    Ok(())
}

/// Every value of every column: a line naming them, then one line a row, the values apart by
/// tabs (its sample, entry, then each column, stored then derived), each written as it is made
/// from the columns as read; then the sample columns.
fn print_all(reader: &Reader) -> Result<(), String> {
    if reader.empty() {
        println!("{NO_COLUMNS}");
        return Ok(());
    }
    let data = &reader.header()["data"];
    let stored: Vec<String> = data["cols"].as_array().into_iter().flatten().filter_map(|r| r["name"].as_str().map(str::to_owned)).collect();
    let derived = rowver::rvr::derived::listed(reader.header());
    let heads: Vec<&str> = ["sample", "entry"].into_iter().chain(stored.iter().map(String::as_str)).chain(derived.iter().map(|(name, _)| name.as_str())).collect();
    let samples_named = sample_names(data);
    let mut out = heads.join("\t") + "\n";
    // The derived columns checked against their records first; then a run of rows at a time, every
    // column of it read or worked out once, written, and let go.
    let mut each = |first: usize, last: usize, columns: Vec<Held>| {
        use std::fmt::Write;
        let (samples, entries) = (&columns[0].bytes, &columns[1].bytes);
        for row in 0..last - first {
            let at = i32::from_le_bytes(samples[row * 4..row * 4 + 4].try_into().unwrap_or_default());
            let entry = i64::from_le_bytes(entries[row * 8..row * 8 + 8].try_into().unwrap_or_default());
            let _ = write!(out, "{}\t{entry}", samples_named.get(at as usize).copied().unwrap_or("?"));
            for held in &columns[2..] {
                out.push('\t');
                push_row(held, row, &mut out);
            }
            out.push('\n');
            if out.len() > 1 << 16 {
                crate::out("dump", out.as_bytes()).map_err(rowver::Error::Invalid)?;
                out.clear();
            }
        }
        Ok(())
    };
    rowver::rvr::derived::walk(reader, &stored, &mut each).map_err(|why| rowver::plain(reader.path(), &why))?;
    crate::out("dump", out.as_bytes())?;
    print_sample_columns(data, wide(reader));
    Ok(())
}

/// The study's texts a file holds, by the flag that prints one alone: (flag, `made` key, title).
const TEXTS: [(&str, &str, &str); 3] = [
    ("--recipe", "recipe", "recipe.toml"),
    ("--samples", "samples", "samples.toml"),
    ("--lock", "lock", "samples.lock"),
];

/// Print the parts the flags ask for, or everything (with `all`, every value); a header that fails its checks too.
pub fn main(file: &str, flags: &[&str], all: bool) -> Result<(), String> {
    let (reader, checked) = Reader::inspect(file).map_err(|e| rowver::plain(file, &e))?;
    if let Err(why) = &checked {
        println!("{file} is not valid: {why}\n");
    }
    let mut header = reader.header().clone();
    let text = |key: &str| {
        header["made"][key]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let texts: Vec<String> = TEXTS.iter().map(|(_, key, _)| text(key)).collect();
    if flags.is_empty() {
        // A damaged header may hold anything where an object belongs: change only real texts.
        let placed = |at: Option<&mut Value>, said: String| {
            if let Some(text @ Value::String(_)) = at {
                *text = Value::from(said);
            }
        };
        for (_, key, title) in TEXTS {
            let at = header
                .get_mut("made")
                .and_then(|m| m.get_mut(key))
                .and_then(|r| r.get_mut("text"));
            placed(at, format!("[printed below as {title}]"));
        }
        println!("{}\n", serde_json::to_string_pretty(&header).unwrap_or_default());
        for ((_, _, title), text) in TEXTS.iter().zip(&texts) {
            print_source(text, title);
            println!();
        }
        return match all {
            true => print_all(&reader),
            false => print_columns(&reader),
        };
    }
    for (i, flag) in flags.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match *flag {
            "--files" => println!("{}", serde_json::to_string_pretty(&header["made"]["files"]).unwrap_or_default()),
            "--columns" if all => print_all(&reader)?,
            "--columns" => print_columns(&reader)?,
            flag => {
                if let Some(at) = TEXTS.iter().position(|(f, _, _)| *f == flag) {
                    print_source(&texts[at], TEXTS[at].2);
                }
            }
        }
    }
    Ok(())
}
