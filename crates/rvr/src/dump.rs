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

/// One line for a column: name, kind, length, first values.
fn show(name: &str, held: &Held) {
    let w = width(&held.kind);
    match &held.offsets {
        None => {
            let count = held.bytes.len() / w;
            println!(
                "{name:14} {}[{count}]  {}",
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
                "{name:14} jagged {}[{}]  [{}{more}]",
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

/// One of the study's texts, or a derived file's source, between two marker lines.
fn print_source(source: &str, name: &str) {
    println!("##### begin {name} #####");
    print!("{source}");
    if !source.ends_with('\n') {
        println!();
    }
    println!("##### end {name} #####");
}

fn print_columns(reader: &Reader) -> Result<(), String> {
    let data = &reader.header()["data"];
    let rows = reader.rows().map_err(|e| e.to_string())?;
    let sample = reader.sample().map_err(|e| e.to_string())?;
    let entry = reader.entry().map_err(|e| e.to_string())?;
    let sample: Vec<u8> = sample.iter().flat_map(|x| x.to_le_bytes()).collect();
    let entry: Vec<u8> = entry.iter().flat_map(|x| x.to_le_bytes()).collect();
    println!("{:14} i4[{rows}]  {}", "sample", first("i4", &sample, 5));
    println!("{:14} i8[{rows}]  {}", "entry", first("i8", &entry, 5));
    for record in data["cols"].as_array().into_iter().flatten() {
        let name = record["name"].as_str().unwrap_or_default();
        let held = reader
            .column(name)
            .map_err(|e| e.to_string())?
            .ok_or(format!("no column {name}"))?;
        show(name, &held);
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
            "{:14} {} by sample[{}]  {}",
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
        let derived = header
            .get("derived")
            .and_then(|d| d.get("source"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let at = header.get_mut("derived").and_then(|d| d.get_mut("source"));
        placed(at, "[printed below as derived source]".into());
        println!("{}\n", serde_json::to_string_pretty(&header).unwrap());
        for ((_, _, title), text) in TEXTS.iter().zip(&texts) {
            print_source(text, title);
            println!();
        }
        if let Some(derived) = derived {
            print_source(&derived, "derived source");
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
