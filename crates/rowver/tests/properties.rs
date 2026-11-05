//! Properties that hold for every input, over generated cases (SplitMix64 from each case's seed,
//! `ROWVER_CASES` of them, every failure naming its seed): what the writer writes reads back value
//! for value and validates, with its facts made by FORMAT.md §4's rule; a header changed in one place
//! is refused or means the same; canonical JSON is FORMAT.md §8's; and the random choice is §6.1's.

mod common;

use common::gen::{self, Rng};
use common::{to_bytes, written};
use rowver::rvr::{validate_file, Reader};
use rowver::write::Values;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The fewest significant digits that read back as `x` (nonzero, finite), as d.ddde±N: of k digits,
/// the k-digit decimal nearest `x`'s exact value (ties to even), for the least k that reads back.
fn shortest(x: f64) -> String {
    (1..=17).map(|k| format!("{:.*e}", k - 1, x)).find(|t| t.parse::<f64>() == Ok(x)).unwrap()
}

/// A shortest decimal of an end of `lim`, as (digits, exponent of the last digit): `value` read as
/// its kind's number (a whole kind's exactly).
fn decimal(text: &str) -> (i128, i32) {
    // Digits with a point, an exponent, or both (as `shortest` or JSON writes them).
    let (mantissa, exponent) = text.split_once('e').unwrap_or((text, "0"));
    let exponent: i32 = exponent.parse().unwrap();
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits: i128 = format!("{whole}{fraction}").parse().unwrap();
    (digits, exponent - fraction.len() as i32)
}

/// A decimal with its trailing zeros taken off, so that two spellings of one number are one pair.
fn plain((mut d, mut e): (i128, i32)) -> (i128, i32) {
    while d != 0 && d % 10 == 0 {
        (d, e) = (d / 10, e + 1);
    }
    if d == 0 { (0, 0) } else { (d, e) }
}

/// The power of ten of a nonzero decimal's first significant digit.
fn first_digit(digits: i128, exponent: i32) -> i32 {
    digits.unsigned_abs().to_string().len() as i32 - 1 + exponent
}

/// `digits × 10^exponent` rounded to a multiple of `10^step`, down or up, as digits of that power.
fn rounded(digits: i128, exponent: i32, step: i32, up: bool) -> i128 {
    if exponent >= step {
        return digits * 10_i128.pow((exponent - step) as u32);
    }
    let scale = 10_i128.pow((step - exponent) as u32);
    let (q, r) = (digits.div_euclid(scale), digits.rem_euclid(scale));
    if up && r != 0 { q + 1 } else { q }
}

/// FORMAT.md §4's `nice` of `lim`, each end as (digits, power of ten), written independently of the writer.
fn nice(lim: [&str; 2], whole: bool) -> [(i128, i32); 2] {
    let mut out = [(0, 0); 2];
    let mut steps = [None; 2];
    for (i, end) in lim.into_iter().enumerate() {
        let (digits, exponent) = decimal(end);
        if digits != 0 {
            let step = first_digit(digits, exponent) - 1;
            out[i] = (rounded(digits, exponent, step, i == 1), step);
            steps[i] = Some(step);
        }
    }
    if plain(out[0]) == plain(out[1]) {
        out[1] = match steps[1] {
            None => (1, 0),
            // Raised by 1 where its step is below 1 on a whole-number column.
            Some(step) if whole && step < 0 => (out[1].0 + 10_i128.pow((-step) as u32), step),
            Some(step) => (out[1].0 + 1, step),
        };
    }
    out
}

/// A decimal (digits, power) as the number a reader takes: the nearest double, the largest past it.
fn double((digits, exponent): (i128, i32)) -> f64 {
    let x: f64 = format!("{digits}e{exponent}").parse().unwrap();
    if x.is_infinite() { f64::MAX.copysign(x) } else { x }
}

/// A flat column's values as text a decimal reads: a whole kind's digits, a float's shortest double.
fn texts(kind: &str, bytes: &[u8]) -> Vec<(String, bool)> {
    let w = rowver::rvr::width(kind).unwrap();
    bytes
        .chunks(w)
        .map(|b| {
            let mut wide = [0u8; 8];
            wide[..w].copy_from_slice(b);
            let raw = u64::from_le_bytes(wide);
            let signed = |bits: u32| ((raw << (64 - bits)) as i64) >> (64 - bits);
            match kind {
                "f8" | "f4" => {
                    let x = if kind == "f8" { f64::from_bits(raw) } else { f32::from_bits(raw as u32) as f64 };
                    (if x == 0.0 || !x.is_finite() { "0".into() } else { shortest(x) }, x.is_finite())
                }
                "i8" | "i4" | "i2" | "i1" => (signed(w as u32 * 8).to_string(), true),
                _ => (raw.to_string(), true),
            }
        })
        .collect()
}

/// Check one flat column's facts against FORMAT.md §4, from its values: what is wrong, if anything.
fn facts(record: &Value, kind: &str, bytes: &[u8]) -> Option<String> {
    let whole = kind != "f8" && kind != "f4";
    let mut finite: Vec<(String, bool)> = texts(kind, bytes).into_iter().filter(|(_, f)| *f).collect();
    let number = |t: &str| t.parse::<f64>().unwrap();
    let key = |t: &String| if whole { (t.parse::<i128>().unwrap() as f64, t.parse::<i128>().unwrap()) } else { (number(t), 0) };
    finite.sort_by(|a, b| key(&a.0).partial_cmp(&key(&b.0)).unwrap().then(key(&a.0).1.cmp(&key(&b.0).1)));
    let as_number = |v: &Value| v.to_string();
    let (lo, hi) = match (finite.first(), finite.last()) {
        (Some(lo), Some(hi)) => (lo.0.clone(), hi.0.clone()),
        _ => ("0".into(), "0".into()),
    };
    // -0.0 is 0.0, and an end is compared as a number.
    let same = |held: &Value, want: &str| match whole {
        true => held.as_i64().map(i128::from).or(held.as_u64().map(i128::from)) == Some(want.parse().unwrap()) && !as_number(held).contains(['.', 'e']),
        false => held.as_f64() == Some(number(want)),
    };
    let lim = &record["lim"];
    if !same(&lim[0], &lo) || !same(&lim[1], &hi) {
        return Some(format!("lim {lim} for [{lo}, {hi}]"));
    }
    let want = nice([&lo, &hi], whole);
    let held = &record["nice"];
    for (end, (digits, exponent)) in want.iter().enumerate() {
        let ok = match whole {
            true => !as_number(&held[end]).contains(['.', 'e']) && plain(decimal(&as_number(&held[end]))) == plain((*digits, *exponent)),
            false => held[end].as_f64() == Some(double((*digits, *exponent))),
        };
        if !ok {
            return Some(format!("nice {held} for lim [{lo}, {hi}], not ends {want:?}"));
        }
    }
    let mut distinct: Vec<String> = finite.iter().map(|(t, _)| t.clone()).collect();
    distinct.dedup();
    let vals = (whole && distinct.len() <= 64).then(|| distinct.iter().map(|t| t.parse::<i128>().unwrap().to_string()).collect::<Vec<_>>());
    let recorded = record.get("vals").map(|v| v.as_array().unwrap().iter().map(as_number).collect::<Vec<_>>());
    (vals != recorded).then(|| format!("vals {recorded:?}, not {vals:?}"))
}

// FORMAT 1, 3, 4, 7, 8, 12; SPEC 9.1
#[test]
fn every_table_written_reads_back_value_for_value_and_validates() {
    let mut wrong = Vec::new();
    for seed in 0..gen::cases(200) {
        let mut rng = Rng::new(seed);
        let table = gen::table(&mut rng, 12);
        let values = gen::filled(&mut rng, &table);
        // What each column holds over every row, in row order, as bytes, and each row's count.
        let mut expected: Vec<(Vec<u8>, Option<Vec<usize>>)> = vec![(Vec::new(), None); table.columns.len()];
        for sample in &values {
            for (column, v) in sample.iter().enumerate() {
                match v {
                    Values::Flat(f) => expected[column].0.extend(&f.bytes),
                    Values::Jagged { counts, content } => {
                        expected[column].0.extend(&content.bytes);
                        expected[column].1.get_or_insert_with(Vec::new).extend(counts);
                    }
                }
            }
        }
        let (header, arrays) = gen::file(&table, values);
        let path = written(&to_bytes(&header, &arrays), "property");
        let reader = match Reader::open(&path) {
            Ok(reader) => reader,
            Err(e) => {
                wrong.push(format!("seed {seed}: refused on opening: {e}"));
                continue;
            }
        };
        if let Err(e) = validate_file(&path) {
            wrong.push(format!("seed {seed}: refused in full: {e}"));
        }
        for (column, (bytes, counts)) in table.columns.iter().zip(&expected) {
            let held = reader.column(&column.name).unwrap().unwrap();
            let offsets = counts.as_ref().map(|c| std::iter::once(0).chain(c.iter().scan(0, |at, n| { *at += n; Some(*at) })).collect::<Vec<usize>>());
            if held.bytes != *bytes || held.offsets != offsets || held.kind != column.kind {
                wrong.push(format!("seed {seed}: {} reads back otherwise", column.name));
            }
            let record = header["data"]["cols"].as_array().unwrap().iter().find(|c| c["name"] == column.name.as_str()).unwrap();
            let facts_record = if column.jagged { None } else { Some(record) };
            if let Some(problem) = facts_record.and_then(|r| facts(r, &column.kind, bytes)) {
                wrong.push(format!("seed {seed}: {} ({}): {problem}", column.name, column.kind));
            }
        }
    }
    assert!(wrong.is_empty(), "{} failed:\n{}", wrong.len(), wrong.join("\n"));
}

/// A header's meaning: objects by key, the numbers of facts (`lim`, `nice`, `vals`) and of sample
/// columns' `values` by value, every other number as canonical JSON takes it (a whole number by
/// its digits, any other by its double), so that the spellings FORMAT.md allows are the same and
/// nothing else is.
fn meaning(value: &Value, facts: bool) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: Vec<(&String, &Value)> = map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), meaning(v, ["lim", "nice", "vals", "values"].contains(&k.as_str())))).collect::<Map<_, _>>())
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| meaning(v, facts)).collect()),
        Value::Number(n) if facts => json!(format!("{:?}", plain(decimal(&n.to_string().replace('E', "e").replace("e+", "e"))))),
        Value::Number(n) if n.to_string().contains(['.', 'e', 'E']) => json!(format!("double {:?}", n.as_f64())),
        Value::Number(n) => json!(format!("whole {n}")),
        other => other.clone(),
    }
}

// FORMAT 1, 2, 11, 12, 14
#[test]
fn a_header_changed_in_one_place_is_refused_or_means_the_same() {
    let mut wrong = Vec::new();
    let mut refused = 0;
    let cases = gen::cases(200);
    for seed in 0..cases {
        let mut rng = Rng::new(seed);
        let table = gen::table(&mut rng, 6);
        let values = gen::filled(&mut rng, &table);
        let (header, arrays) = gen::file(&table, values);
        let text = serde_json::to_string(&header).unwrap();
        let changed = mutated(&mut rng, &text);
        let mut bytes = b"\x89RVR\r\n\x1a\n".to_vec();
        bytes.extend((changed.len() as u32).to_le_bytes());
        bytes.extend(changed.as_bytes());
        bytes.extend(arrays.concat());
        let path = written(&bytes, "mutated");
        // Refused on opening, or by full validation (by_sample, say, is checked only there).
        match Reader::open(&path).and_then(|reader| validate_file(&path).map(|_| reader)) {
            Err(_) => refused += 1,
            Ok(reader) => {
                if meaning(reader.header(), false) != meaning(&header, false) {
                    wrong.push(format!("seed {seed}: accepted with another meaning:\n  {}", difference(&text, &changed)));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {cases}:\n{}", wrong.len(), wrong.join("\n"));
    assert!(refused * 2 > cases, "most changes are refused: {refused} of {cases}");
}

/// Where two texts first differ, a little of each.
fn difference(a: &str, b: &str) -> String {
    let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(30);
    format!("{} | {}", &a[from..(at + 30).min(a.len())], &b[from..(at + 30).min(b.len())])
}

/// One change to a header's JSON text: a byte, a key renamed, a number spelled otherwise, a key
/// written twice, or a value nested one deeper.
fn mutated(rng: &mut Rng, text: &str) -> String {
    let bytes = text.as_bytes();
    let pick = |rng: &mut Rng, matches: Vec<usize>| matches[rng.below(matches.len() as u64) as usize];
    let keys: Vec<usize> = text.match_indices("\":").map(|(i, _)| i).collect();
    let numbers: Vec<usize> = (1..bytes.len()).filter(|&i| bytes[i].is_ascii_digit() && matches!(bytes[i - 1], b':' | b'[' | b',')).collect();
    match rng.below(5) {
        0 => {
            let at = rng.below(bytes.len() as u64) as usize;
            let mut out = bytes.to_vec();
            out[at] = out[at].wrapping_add(1 + rng.below(254) as u8);
            String::from_utf8_lossy(&out).into_owned()
        }
        1 => {
            let at = pick(rng, keys);
            format!("{}x{}", &text[..at], &text[at..])
        }
        2 if !numbers.is_empty() => {
            let at = pick(rng, numbers);
            let end = at + text[at..].find(|c: char| !(c.is_ascii_digit() || ".eE+-".contains(c))).unwrap();
            let number = &text[at..end];
            let spelled = match rng.below(3) {
                0 if !number.contains(['.', 'e', 'E']) => format!("{number}.0"),
                1 => format!("{number}e0"),
                _ => format!("{number}0e-1"),
            };
            format!("{}{spelled}{}", &text[..at], &text[end..])
        }
        3 => {
            // A key written twice: `"k":v` copied in front of itself, for a key whose value is a plain token.
            let at = pick(rng, keys);
            let start = text[..at].rfind('"').unwrap();
            let end = at + 2 + text[at + 2..].find([',', '}']).unwrap();
            format!("{}{},{}", &text[..start], &text[start..end], &text[start..])
        }
        _ => {
            let at = pick(rng, keys) + 2;
            let end = at + text[at..].find([',', '}']).unwrap();
            format!("{}[{}]{}", &text[..at], &text[at..end], &text[end..])
        }
    }
}

/// FORMAT.md §8's canonical JSON of a value, written from that section alone.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut members: Vec<(&String, &Value)> = map.iter().collect();
            members.sort_by(|a, b| a.0.chars().cmp(b.0.chars()));
            let inner: Vec<String> = members.into_iter().map(|(k, v)| format!("{}:{}", string(k), canonical(v))).collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::String(text) => string(text),
        Value::Number(n) => {
            let written = n.to_string();
            match written.contains(['.', 'e', 'E']) {
                false => written,
                true => float(n.as_f64().unwrap()),
            }
        }
        other => other.to_string(),
    }
}

/// A string as canonical JSON writes it.
fn string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\u{8}' => out += "\\b",
            '\t' => out += "\\t",
            '\n' => out += "\\n",
            '\u{c}' => out += "\\f",
            '\r' => out += "\\r",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out + "\""
}

/// A double as canonical JSON writes it: the fewest significant digits that read back, in decimal
/// when its first digit's power is from -4 to 15, else as d.ddde±XX.
fn float(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let digits = shortest(x.abs());
    let (mantissa, exponent) = digits.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let digits: String = mantissa.replace('.', "");
    let sign = if x < 0.0 { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        let text = match exponent >= 0 {
            true => {
                let point = exponent as usize + 1;
                let padded = format!("{digits:0<point$}");
                let (whole, fraction) = padded.split_at(point);
                format!("{whole}.{}", if fraction.is_empty() { "0" } else { fraction })
            }
            false => format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize)),
        };
        return format!("{sign}{text}");
    }
    let rest = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
    format!("{sign}{}{rest}e{}{:02}", &digits[..1], if exponent < 0 { "-" } else { "+" }, exponent.abs())
}

/// A random JSON value: objects of keys with any character, strings with control characters and
/// every plane, whole numbers to 20 digits, and doubles spelled several ways.
fn json_value(rng: &mut Rng, depth: u32) -> Value {
    let text = |rng: &mut Rng| -> String {
        (0..rng.below(6)).map(|_| *rng.one(&['a', 'Z', '"', '\\', '\n', '\u{1}', '\u{1f}', 'é', '€', '😀', '\u{7f}', ' '])).collect()
    };
    match rng.below(if depth == 0 { 4 } else { 6 }) {
        0 => Value::String(text(rng)),
        1 => {
            let n = match rng.below(3) {
                0 => (rng.next() >> rng.below(64)).to_string(),
                // A header never writes -0 (§1).
                1 => format!("-{}", (rng.next() >> rng.below(64)).max(1)),
                _ => "18446744073709551615".into(),
            };
            serde_json::from_str(&n).unwrap()
        }
        2 => {
            let x = match rng.below(3) {
                0 => f64::from_bits(rng.next()),
                1 => (rng.next() >> 11) as f64 / 1e9,
                _ => *rng.one(&[0.0, -0.0, 1e-5, 1e-4, 1.5e2, 1234567890123456.0, 1.5e16, 1e300, 5e-324]),
            };
            let x = if x.is_finite() { x } else { 0.5 };
            let spelled = match rng.below(3) {
                0 => format!("{x:e}"),
                1 => format!("{x:?}"),
                _ => format!("{x:E}"),
            };
            serde_json::from_str(&spelled).unwrap()
        }
        3 => json!(rng.below(3) == 0),
        4 => Value::Array((0..rng.below(4)).map(|_| json_value(rng, depth - 1)).collect()),
        _ => Value::Object((0..rng.below(4)).map(|_| (text(rng), json_value(rng, depth - 1))).collect()),
    }
}

// FORMAT 8
#[test]
fn canonical_json_is_format_md_s_for_every_value() {
    // FORMAT.md's own table.
    for (written, want) in [
        (r#"{ "b": 1, "a": [true, null] }"#, r#"{"a":[true,null],"b":1}"#),
        ("18446744073709551615", "18446744073709551615"),
        ("1.5e2", "150.0"),
        ("0.00010", "0.0001"),
        ("0.00001", "1e-05"),
        ("1234567890123456.0", "1234567890123456.0"),
        ("15000000000000000.0", "1.5e+16"),
    ] {
        let value: Value = serde_json::from_str(written).unwrap();
        assert_eq!(canonical(&value), want, "{written}");
        assert_eq!(rowver_canonical(&value), want, "{written}");
    }
    let mut wrong = Vec::new();
    for seed in 0..gen::cases(2000) {
        let mut rng = Rng::new(seed);
        let value = json_value(&mut rng, 3);
        let (ours, theirs) = (canonical(&value), rowver_canonical(&value));
        if ours != theirs {
            wrong.push(format!("seed {seed}: {value}\n  FORMAT {ours}\n  rowver {theirs}"));
        }
    }
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

/// rowver's canonical JSON of a value, seen through the checksum of `made` (its canonical JSON
/// without `sha256`): the text whose SHA-256 that is.
fn rowver_canonical(value: &Value) -> String {
    let ours = canonical(value);
    let made = json!({"x": value});
    let sum = rowver::write::made_checksum(&made).unwrap();
    let mine = format!("{{\"x\":{ours}}}");
    let hex: String = Sha256::digest(mine.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    if hex == sum { ours } else { format!("(another text, checksum {sum})") }
}

/// FORMAT.md §6.1's choice, written from its pseudocode: the entries chosen of `total`.
fn chosen(total: u64, n: u64, seed: u64) -> Vec<u64> {
    let mut state = seed;
    let mut draw = || {
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        (z >> 11) as f64 / 9007199254740992.0
    };
    let (mut needed, mut out) = (n.min(total), Vec::new());
    let mut i = 0;
    while needed > 0 {
        if ((total - i) as f64) * draw() < needed as f64 {
            out.push(i);
            needed -= 1;
        }
        i += 1;
    }
    out
}

// FORMAT 6.1; SPEC 7.1.1
#[test]
fn the_random_choice_is_format_md_s_and_files_do_not_change_it() {
    let mut wrong = Vec::new();
    for seed in 0..gen::cases(2000) {
        let mut rng = Rng::new(seed);
        let files: Vec<u64> = (0..1 + rng.below(6)).map(|_| {
            let most = if rng.below(4) == 0 { 0 } else { 400 };
            rng.below(most)
        }).collect();
        let total: u64 = files.iter().sum();
        let n = 1 + rng.below(total + 10);
        let choice_seed = rng.next();
        let want = chosen(total, n, choice_seed);
        let per = rowver::choice::per_file(&files, n, choice_seed);
        // Each file's share, counted from its own first entry, is the run's choice cut at the files.
        let mut start = 0;
        let mut joined = Vec::new();
        for (entries, taken) in files.iter().zip(&per) {
            joined.extend(taken.iter().map(|e| e + start));
            if taken.iter().any(|e| e >= entries) {
                wrong.push(format!("seed {seed}: an entry past its file"));
            }
            start += entries;
        }
        if joined != want || want.len() as u64 != n.min(total) {
            wrong.push(format!("seed {seed}: files {files:?}, n {n}: {joined:?}, not {want:?}"));
        }
        // The same entries cut into other files are chosen alike.
        let cut = rng.below(total + 1);
        let other = rowver::choice::per_file(&[cut, total - cut], n, choice_seed);
        let again: Vec<u64> = other[0].iter().copied().chain(other[1].iter().map(|e| e + cut)).collect();
        if again != want {
            wrong.push(format!("seed {seed}: cut at {cut}, chosen otherwise"));
        }
    }
    assert!(wrong.is_empty(), "{} failed:\n{}", wrong.len(), wrong.join("\n"));
}

// FORMAT 6.1, 6.3; SPEC 7.1.1
#[test]
fn the_first_n_read_each_file_in_turn_and_stop_where_n_runs_out() {
    use rowver::choice::Events;
    use rowver::study::lock::{File, Sample};
    let mut wrong = Vec::new();
    for seed in 0..gen::cases(2000) {
        let mut rng = Rng::new(seed);
        let entries: Vec<u64> = (0..1 + rng.below(6)).map(|_| rng.below(50)).collect();
        let n = 1 + rng.below(entries.iter().sum::<u64>() + 20);
        let files = entries.iter().enumerate().map(|(i, &entries)| File { at: format!("./f{i}.root"), bytes: 1, adler32: None, uuid: String::new(), entries, stop: None, take: None }).collect();
        let sample = Sample { name: "s".into(), label: "s".into(), asked: vec![], tree: "T".into(), branches: String::new(), events: Events::First(n), events_total: 0, events_read: 0, metadata: Default::default(), files };
        let planned = sample.planned(Events::First(n));
        let mut left = n;
        for (file, &held) in planned.files.iter().zip(&entries) {
            let read = held.min(left);
            left -= read;
            // A file records stop exactly when it reads fewer than it has (0 for one never reached).
            if file.stop != (read < held).then_some(read) || file.take.is_some() {
                wrong.push(format!("seed {seed}: entries {entries:?}, n {n}: {} has stop {:?}", file.at, file.stop));
            }
        }
        if planned.events_read != n.min(entries.iter().sum()) || planned.events_total != entries.iter().sum::<u64>() {
            wrong.push(format!("seed {seed}: reads {} of {}", planned.events_read, planned.events_total));
        }
    }
    assert!(wrong.is_empty(), "{} failed:\n{}", wrong.len(), wrong.join("\n"));
}

// FORMAT 4, example
#[test]
fn nice_is_format_md_s_on_its_own_examples_and_the_edges_of_every_kind() {
    use rowver::write::array;
    let cases: Vec<(rowver::write::NumericBuffer, &str, &str)> = vec![
        (array(&[5_i32, 5]), "[5,5]", "[5,6]"),
        (array(&[5.0_f64, 5.0]), "[5.0,5.0]", "[5.0,5.1]"),
        (array(&[-1.2_f64]), "[-1.2,-1.2]", "[-1.2,-1.1]"),
        (array(&[0_u8, 0]), "[0,0]", "[0,1]"),
        (array(&[f64::NAN, f64::INFINITY]), "[0.0,0.0]", "[0.0,1.0]"),
        (array::<f32>(&[]), "[0.0,0.0]", "[0.0,1.0]"),
        (array(&[true, true]), "[1,1]", "[1,2]"),
        (array(&[0_u64, u64::MAX]), "[0,18446744073709551615]", "[0,19000000000000000000]"),
        (array(&[0.338_f64, 373.67]), "[0.338,373.67]", "[0.33,380.0]"),
        (array(&[-0.037_f64, 0.0]), "[-0.037,0.0]", "[-0.037,0.0]"),
        (array(&[1.1_f32]), "[1.100000023841858,1.100000023841858]", "[1.1,1.2]"),
        (array(&[i64::MIN, i64::MAX]), "[-9223372036854775808,9223372036854775807]", "[-9300000000000000000,9300000000000000000]"),
        (array(&[f64::MAX, -f64::MAX]), "[-1.7976931348623157e+308,1.7976931348623157e+308]", "[-1.7976931348623157e+308,1.7976931348623157e+308]"),
        (array(&[5e-324_f64, 1.0]), "[5e-324,1.0]", "[5e-324,1.0]"),
    ];
    let mut wrong = Vec::new();
    for (values, lim, nice) in cases {
        let kind = values.kind.clone();
        let raw = values.bytes.clone();
        let n = values.count;
        let table = gen::Table { samples: vec![("s".into(), (0..n as i64).collect(), n as u64, Map::new())], columns: vec![gen::Column { name: "x".into(), kind: kind.clone(), jagged: false }] };
        let (header, _) = gen::file(&table, vec![vec![Values::Flat(values)]]);
        let record = &header["data"]["cols"][0];
        let (got_lim, got_nice) = (record["lim"].to_string(), record["nice"].to_string());
        if got_lim != lim || got_nice != nice {
            wrong.push(format!("{kind} {raw:?}: lim {got_lim} nice {got_nice}, not {lim} {nice}"));
        }
        if let Some(problem) = facts(record, &kind, &raw) {
            wrong.push(format!("{kind}: against this test's own rule: {problem}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
