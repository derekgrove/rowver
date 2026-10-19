//! rowver's canonical JSON, which `made.sha256`, `derived.sha256` and a signature are taken of:
//! keys sorted, no spaces, UTF-8, whole numbers as written, floats in their shortest digits.
//! (The rule is the bytes Python's `json.dumps(sort_keys=True, separators=(",", ":"),
//! ensure_ascii=False)` gives, which is where it came from; FORMAT.md states it on its own.)

use crate::{invalid, Result};
use serde_json::Value;

/// The canonical text of a value.
pub(crate) fn canonical(value: &Value) -> Result<String> {
    let mut out = String::new();
    write(value, &mut out)?;
    Ok(out)
}

/// Append one value: objects with their keys sorted, nothing between the parts.
fn write(value: &Value, out: &mut String) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::String(value) => out.push_str(&serde_json::to_string(value)?),
        Value::Number(number) => out.push_str(&self::number(number)?),
        Value::Array(values) => {
            out.push('[');
            for (index, member) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(member, out)?;
            }
            out.push(']');
        }
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                write(&values[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Spell a number: a whole number as written, a float in its shortest digits.
fn number(number: &serde_json::Number) -> Result<String> {
    let text = number.to_string();
    if text.contains(['.', 'e', 'E']) {
        let value = number
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| invalid("Nonfinite recipe JSON number"))?;
        return Ok(float(value));
    }
    Ok(if text == "-0" { "0".into() } else { text })
}

/// A float's shortest round-trip digits: in decimal from 1e-4 up to 1e16, and otherwise as
/// `d.ddde±XX`, with `.0` on a whole number and `0.0` for zero.
pub(crate) fn float(value: f64) -> String {
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
