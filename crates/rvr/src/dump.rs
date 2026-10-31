//! `rvr dump`: the inside of a file, the header in full, the study's texts, then every array as text.

use rowver::rvr::{Held, Reader};
use serde_json::Value;

/// The first `n` values of little-endian bytes of a kind, as text, ending in `...` when there are more.
fn first(kind: &str, bytes: &[u8], n: usize) -> String {
    let w = width(kind);
    let values: Vec<String> = bytes
        .chunks_exact(w)
        .take(n)
        .map(|b| match kind {
            "f8" => f64::from_le_bytes(b.try_into().unwrap()).to_string(),
            "f4" => f32::from_le_bytes(b.try_into().unwrap()).to_string(),
            "i8" => i64::from_le_bytes(b.try_into().unwrap()).to_string(),
            "i4" => i32::from_le_bytes(b.try_into().unwrap()).to_string(),
            "i2" => i16::from_le_bytes(b.try_into().unwrap()).to_string(),
            "i1" => (b[0] as i8).to_string(),
            "u8" => u64::from_le_bytes(b.try_into().unwrap()).to_string(),
            "u4" => u32::from_le_bytes(b.try_into().unwrap()).to_string(),
            "u2" => u16::from_le_bytes(b.try_into().unwrap()).to_string(),
            "b1" => (b[0] == 1).to_string(),
            _ => b[0].to_string(),
        })
        .collect();
    let more = if bytes.len() / w > n { ", ..." } else { "" };
    format!("[{}{more}]", values.join(", "))
}

/// One line for a column: name, `wide` characters at least, kind, length, first values; for a
/// derived column, `derived` and the expression it is worked out by, too.
fn show(name: &str, wide: usize, held: &Held, expression: Option<&str>) {
    let w = width(&held.kind);
    let derived = match expression {
        Some(expression) => format!("derived = {expression:?}  "),
        None => String::new(),
    };
    match &held.offsets {
        None => {
            let count = held.bytes.len() / w;
            println!(
                "{name:wide$} {}[{count}]  {derived}{}",
                held.kind,
                first(&held.kind, &held.bytes, 5)
            );
        }
        Some(offsets) => {
            let lists: Vec<String> = offsets
                .windows(2)
                .take(3)
                .map(|r| first(&held.kind, &held.bytes[r[0] * w..r[1] * w], 4))
                .collect();
            let more = if offsets.len() > 4 { ", ..." } else { "" };
            println!(
                "{name:wide$} jagged {}[{}]  {derived}[{}{more}]",
                held.kind,
                offsets.len() - 1,
                lists.join(", ")
            );
        }
    }
}

/// Bytes per value of a kind.
fn width(kind: &str) -> usize {
    match kind {
        "f8" | "i8" | "u8" => 8,
        "f4" | "i4" | "u4" => 4,
        "i2" | "u2" => 2,
        _ => 1,
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

fn print_columns(reader: &Reader) -> Result<(), String> {
    if reader.empty() {
        println!("No columns: the file is empty (rvr empty let them go); rvr refill presses them again from the study it carries.");
        return Ok(());
    }
    let data = &reader.header()["data"];
    let rows = reader.rows().map_err(|e| e.to_string())?;
    let sample = reader.sample().map_err(|e| e.to_string())?;
    let entry = reader.entry().map_err(|e| e.to_string())?;
    let sample: Vec<u8> = sample.iter().flat_map(|x| x.to_le_bytes()).collect();
    let entry: Vec<u8> = entry.iter().flat_map(|x| x.to_le_bytes()).collect();
    let derived = rowver::rvr::derived::listed(reader.header());
    // Names line up in a column as wide as the longest, and never narrower than 14.
    let named = |key: &str| data[key].as_array().into_iter().flatten().filter_map(|r| r["name"].as_str());
    let all = named("cols").chain(named("sample_cols")).chain(derived.iter().map(|(n, _)| n.as_str()));
    let wide = all.map(|name| name.chars().count()).fold(14, usize::max);
    println!("{:wide$} i4[{rows}]  {}", "sample", first("i4", &sample, 5));
    println!("{:wide$} i8[{rows}]  {}", "entry", first("i8", &entry, 5));
    for record in data["cols"].as_array().into_iter().flatten() {
        let name = record["name"].as_str().unwrap_or_default();
        let held = reader
            .column(name)
            .map_err(|e| e.to_string())?
            .ok_or(format!("no column {name}"))?;
        show(name, wide, &held, None);
    }
    // The derived columns are worked out together, each column they use read once; when that
    // fails, one at a time, to say which cannot be.
    let names: Vec<String> = derived.iter().map(|(name, _)| name.clone()).collect();
    let together = rowver::rvr::derived::columns(reader, &names);
    for (at, (name, expression)) in derived.iter().enumerate() {
        let alone;
        let held = match &together {
            Ok(held) => Ok(&held[at]),
            Err(_) => {
                alone = rowver::rvr::derived::columns(reader, std::slice::from_ref(name));
                alone.as_ref().map(|held| &held[0])
            }
        };
        match held {
            Ok(held) => show(name, wide, held, Some(expression)),
            Err(why) => println!("{name:wide$} derived = {expression:?}  (not worked out: {why})"),
        }
    }
    let names: Vec<&str> = data["samples"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["name"].as_str())
        .collect();
    for record in data["sample_cols"].as_array().into_iter().flatten() {
        let values: Vec<String> = names
            .iter()
            .zip(record["values"].as_array().into_iter().flatten())
            .map(|(n, v)| format!("{n}: {v}"))
            .collect();
        println!(
            "{:wide$} {} by sample[{}]  {}",
            record["name"].as_str().unwrap_or_default(),
            record["kind"].as_str().unwrap_or_default(),
            names.len(),
            values.join(", ")
        );
    }
    Ok(())
}

/// The study's texts a file holds, by the flag that prints one alone: (flag, `made` key, title).
const TEXTS: [(&str, &str, &str); 3] = [
    ("--recipe", "recipe", "recipe.toml"),
    ("--samples", "samples", "samples.toml"),
    ("--lock", "lock", "samples.lock"),
];

/// Print the parts the flags ask for (`--files`, `--recipe`, `--samples`, `--lock`,
/// `--columns`), or everything: the header with the texts printed after it, then every column.
/// A file whose header fails its checks is still shown, after one line saying what is wrong.
pub fn main(file: &str, flags: &[&str]) -> Result<(), String> {
    let (reader, checked) = Reader::inspect(file).map_err(|e| format!("{file}: {e}"))?;
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
    let mut parts = vec!["--files", "--columns"];
    parts.extend(TEXTS.iter().map(|(flag, _, _)| *flag));
    if let Some(bad) = flags.iter().find(|f| !parts.contains(f)) {
        return Err(format!("dump does not know {bad}"));
    }
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
        println!("{}\n", serde_json::to_string_pretty(&header).unwrap());
        for ((_, _, title), text) in TEXTS.iter().zip(&texts) {
            print_source(text, title);
            println!();
        }
        return print_columns(&reader);
    }
    for (i, flag) in flags.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match *flag {
            "--files" => println!(
                "{}",
                serde_json::to_string_pretty(&header["made"]["files"]).unwrap()
            ),
            "--columns" => print_columns(&reader)?,
            flag => {
                let at = TEXTS.iter().position(|(f, _, _)| *f == flag).unwrap();
                print_source(&texts[at], TEXTS[at].2);
            }
        }
    }
    Ok(())
}
