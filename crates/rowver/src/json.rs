//! Python's `json.dumps` text, reproduced so headers and checksums match the Python RowVer.
//!
//! Ported from: CPython 3.13 `json.dumps` (`json.encoder`) and `float.__repr__`.
//! Behaviours: the default style (`", "` and `": "`, insertion order, non-ASCII escaped) and the
//! checksum style (`sort_keys=True, separators=(",", ":"), ensure_ascii=False`).
//! Documentation: <https://docs.python.org/3/library/json.html#json.dumps>

use crate::{invalid, Result};
use serde_json::Value;

/// Reproduce `json.dumps(value)` with Python's defaults.
pub(crate) fn dumps(value: &Value) -> Result<String> {
    let mut out = String::new();
    write(value, false, &mut out)?;
    Ok(out)
}

/// Reproduce Python's sorted compact JSON, including its float exponent spelling.
pub(crate) fn canonical(value: &Value) -> Result<String> {
    let mut out = String::new();
    write(value, true, &mut out)?;
    Ok(out)
}

/// Append one value in the checksum style (sorted, compact, UTF-8) or Python's default style.
fn write(value: &Value, checksum: bool, out: &mut String) -> Result<()> {
    let (comma, colon) = if checksum { (",", ":") } else { (", ", ": ") };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::String(value) => text(value, checksum, out)?,
        Value::Number(number) => out.push_str(&self::number(number)?),
        Value::Array(values) => {
            out.push('[');
            for (index, member) in values.iter().enumerate() {
                if index > 0 {
                    out.push_str(comma);
                }
                write(member, checksum, out)?;
            }
            out.push(']');
        }
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            if checksum {
                keys.sort();
            }
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(comma);
                }
                text(key, checksum, out)?;
                out.push_str(colon);
                write(&values[key], checksum, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Spell a number as Python does: whole numbers as written, floats as `repr` gives them.
fn number(number: &serde_json::Number) -> Result<String> {
    let text = number.to_string();
    if text.contains(['.', 'e', 'E']) {
        let value = number
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| invalid("Nonfinite recipe JSON number"))?;
        return Ok(python_float(value));
    }
    Ok(if text == "-0" { "0".into() } else { text })
}

/// Quote a string, escaping every non-ASCII character unless the checksum style keeps UTF-8.
fn text(value: &str, utf8: bool, out: &mut String) -> Result<()> {
    let quoted = serde_json::to_string(value)?;
    if utf8 {
        out.push_str(&quoted);
        return Ok(());
    }
    let mut units = [0; 2];
    for character in quoted.chars() {
        if (' '..='~').contains(&character) {
            out.push(character);
        } else {
            for unit in character.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    Ok(())
}

/// Apply Python's decimal/scientific cutoffs to shortest round-to-even digits.
pub(crate) fn python_float(value: f64) -> String {
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    let mut formatter = zmij::Buffer::new();
    let text = formatter.format_finite(value.abs());
    let (mantissa, exponent) = text
        .split_once('e')
        .map_or((text, 0_i32), |(mantissa, exponent)| {
            (mantissa, exponent.parse().unwrap())
        });
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let first = digits.find(|character| character != '0').unwrap();
    let scientific = point - first as i32 - 1;
    let digits = digits[first..].trim_end_matches('0');
    if !(-4..16).contains(&scientific) {
        let tail = if digits.len() == 1 {
            String::new()
        } else {
            format!(".{}", &digits[1..])
        };
        return format!("{sign}{}{tail}e{scientific:+03}", &digits[..1]);
    }
    let position = scientific + 1;
    if position <= 0 {
        return format!("{sign}0.{}{}", "0".repeat((-position) as usize), digits);
    }
    let position = position as usize;
    if position >= digits.len() {
        return format!("{sign}{digits}{}.0", "0".repeat(position - digits.len()));
    }
    format!("{sign}{}.{}", &digits[..position], &digits[position..])
}
