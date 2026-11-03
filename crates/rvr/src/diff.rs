//! `rvr diff`: where two files differ, from their headers alone; no array is decompressed.
//!
//! Every per-row array carries one sha256 per sample (`by_sample`), so two headers are enough to
//! name the column and the sample that changed between one press and the next.

use serde_json::Value;

/// A column's kind as text: `f4`, or `jagged f4`.
fn kind(column: &Value) -> String {
    match column["kind"].as_str() {
        Some("jagged") => format!(
            "jagged {}",
            column["content"]["kind"].as_str().unwrap_or("?")
        ),
        other => other.unwrap_or("?").to_string(),
    }
}

/// A column's arrays that carry `by_sample`, by part: values, or offsets and content.
fn parts(column: &Value) -> Vec<(&'static str, &Value)> {
    if column["kind"] == "jagged" {
        vec![
            ("offsets", &column["offsets"]),
            ("content", &column["content"]),
        ]
    } else {
        vec![("values", column)]
    }
}

/// `{name: record}` of a header list, in order.
fn named(list: &Value) -> Vec<(&str, &Value)> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["name"].as_str()?, r)))
        .collect()
}

fn find<'a>(list: &[(&str, &'a Value)], name: &str) -> Option<&'a Value> {
    list.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

/// The records two lists both hold, by name, each named `what NAME`.
fn pairs<'a>(lists: &[Vec<(&str, &'a Value)>; 2], what: &str) -> Vec<(String, &'a Value, &'a Value)> {
    let first = lists[0].iter();
    first
        .filter_map(|(n, r)| Some((format!("{what} {n}"), *r, find(&lists[1], n)?)))
        .collect()
}

/// Every line saying how two headers differ; none when they show no difference.
fn compare(a: &str, b: &str, headers: [&Value; 2]) -> Vec<String> {
    let (first, second) = (&headers[0]["data"], &headers[1]["data"]);
    let mut said = Vec::new();
    if first["rows"] != second["rows"] {
        said.push(format!(
            "rows: {} in {a}, {} in {b}",
            first["rows"], second["rows"]
        ));
    }
    let samples = [named(&first["samples"]), named(&second["samples"])];
    let index = |i: usize, name: &str| samples[i].iter().position(|(n, _)| *n == name).unwrap();
    let shared: Vec<&str> = samples[0]
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| find(&samples[1], n).is_some())
        .collect();
    let columns = [named(&first["cols"]), named(&second["cols"])];
    let derived = [named(&headers[0]["derived"]["columns"]), named(&headers[1]["derived"]["columns"])];
    for (what, lists) in [("sample", &samples), ("column", &columns), ("derived column", &derived)] {
        for (i, file) in [(0, a), (1, b)] {
            for (name, _) in &lists[i] {
                if find(&lists[1 - i], name).is_none() {
                    said.push(format!("{what} {name}: only in {file}"));
                }
            }
        }
    }
    for name in &shared {
        let rows = [
            &find(&samples[0], name).unwrap()["rows"],
            &find(&samples[1], name).unwrap()["rows"],
        ];
        if rows[0] != rows[1] {
            said.push(format!(
                "sample {name}: {} rows in {a}, {} in {b}",
                rows[0], rows[1]
            ));
        }
    }
    let differs = |one: &Value, other: &Value, sample: &str| {
        one["by_sample"][index(0, sample)] != other["by_sample"][index(1, sample)]
    };
    let both: Vec<(String, &Value, &Value)> = pairs(&columns, "column")
        .into_iter()
        .chain(pairs(&derived, "derived column"))
        .collect();
    for (name, one, other) in &both {
        if kind(one) != kind(other) {
            said.push(format!("{name}: {} in {a}, {} in {b}", kind(one), kind(other)));
        }
        if one["expression"] != other["expression"] {
            said.push(format!("{name}: its expression is {} in {a}, {} in {b}", one["expression"], other["expression"]));
        }
    }
    for sample in &shared {
        if differs(&first["entry"], &second["entry"], sample) {
            said.push(format!("entry: differs in sample {sample}"));
        }
    }
    for (name, one, other) in both
        .iter()
        .filter(|(_, one, other)| kind(one) == kind(other))
    {
        for sample in &shared {
            let found: Vec<&str> = parts(one)
                .into_iter()
                .zip(parts(other))
                .filter(|((_, x), (_, y))| differs(x, y, sample))
                .map(|((part, _), _)| part)
                .collect();
            match found.as_slice() {
                [] => {}
                ["values"] => said.push(format!("{name}: differs in sample {sample}")),
                found => said.push(format!(
                    "{name}: differs in sample {sample} ({})",
                    found.join(", ")
                )),
            }
        }
    }
    let held = [named(&first["sample_cols"]), named(&second["sample_cols"])];
    for (i, file) in [(0, a), (1, b)] {
        for (name, _) in &held[i] {
            if find(&held[1 - i], name).is_none() {
                said.push(format!("sample column {name}: only in {file}"));
            }
        }
    }
    for (name, one) in &held[0] {
        let Some(other) = find(&held[1], name) else {
            continue;
        };
        if one["kind"] != other["kind"] {
            said.push(format!(
                "sample column {name}: {} in {a}, {} in {b}",
                one["kind"].as_str().unwrap_or("?"),
                other["kind"].as_str().unwrap_or("?")
            ));
            continue;
        }
        for sample in &shared {
            let values = [
                &one["values"][index(0, sample)],
                &other["values"][index(1, sample)],
            ];
            if values[0] != values[1] {
                said.push(format!(
                    "sample column {name}: differs in sample {sample}: {} in {a}, {} in {b}",
                    values[0], values[1]
                ));
            }
        }
    }
    said
}

/// Print one line per difference; the exit status is 0 when the headers show none, 1 when they
/// show some (a header that fails its checks, or one file empty and the other not, is one), and 2
/// when the files cannot be compared.
pub fn main(a: &str, b: &str) -> u8 {
    let read = |path: &str| rowver::rvr::Reader::inspect(path).map_err(|e| format!("{path}: {e}"));
    let (first, second) = match (read(a), read(b)) {
        (Ok(first), Ok(second)) => (first, second),
        (Err(e), _) | (_, Err(e)) => {
            println!("cannot compare: {e}");
            return 2;
        }
    };
    let mut said = Vec::new();
    for (path, (_, checked)) in [(a, &first), (b, &second)] {
        if let Err(why) = checked {
            said.push(format!("{path}: its header fails its checks, and is compared all the same: {why}"));
        }
    }
    for (empty, full) in [((a, &first.0), (b, &second.0)), ((b, &second.0), (a, &first.0))] {
        if empty.1.empty() && !full.1.empty() {
            said.push(format!("{} is empty: its arrays are gone (rvr refill presses them again), and {} holds them", empty.0, full.0));
        }
    }
    said.extend(compare(a, b, [first.0.header(), second.0.header()]));
    if said.is_empty() {
        println!("The headers show no difference in rows, samples, columns, derived columns, by_sample or sample columns.");
        return 0;
    }
    println!("{}", said.join("\n"));
    1
}
