//! rowver's canonical JSON, which `made.sha256` and a signature are taken of:
//! keys sorted, no spaces, UTF-8, whole numbers as written, floats in their shortest digits.

use crate::{invalid, Result};
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

/// How deep a header's arrays and objects may nest, the header itself the first.
const DEEPEST: usize = 64;

/// Parse a header's JSON, refusing what a plain parse lets through (docs/FORMAT.md §1): an object
/// holding a key twice, or arrays and objects nested deeper than 64.
pub(crate) fn strict(text: &[u8]) -> Result<Value> {
    let mut parsed = serde_json::Deserializer::from_slice(text);
    let value = Strict { depth: 1, at: At::Top }.deserialize(&mut parsed)?;
    parsed.end()?;
    Ok(value)
}

/// Where a value is in the header, written out only when it is refused.
#[derive(Clone, Copy)]
enum At<'a> {
    Top,
    Key(&'a At<'a>, &'a str),
    Index(&'a At<'a>, usize),
}

impl std::fmt::Display for At<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            At::Top => f.write_str("the header"),
            At::Key(At::Top, key) => f.write_str(key),
            At::Key(parent, key) => write!(f, "{parent}.{key}"),
            At::Index(parent, index) => write!(f, "{parent}[{index}]"),
        }
    }
}

/// One value of the header, `depth` arrays and objects deep, at `at`.
#[derive(Clone, Copy)]
struct Strict<'a> {
    depth: usize,
    at: At<'a>,
}

impl Strict<'_> {
    /// The depth of what an array or object holds, refused when it is too deep itself.
    fn deeper<E: serde::de::Error>(self) -> std::result::Result<usize, E> {
        match self.depth > DEEPEST {
            true => Err(E::custom(format!("{} is nested {} arrays and objects deep, and a header nests at most {DEEPEST} (docs/FORMAT.md §1): hold the tool's data flatter", self.at, self.depth))),
            false => Ok(self.depth + 1),
        }
    }
}

impl<'de> DeserializeSeed<'de> for Strict<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, value: D) -> std::result::Result<Value, D::Error> {
        value.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E>(self, value: f64) -> std::result::Result<Value, E> {
        Ok(Number::from_f64(value).map_or(Value::Null, Value::Number))
    }
    fn visit_str<E>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }
    fn visit_string<E>(self, value: String) -> std::result::Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let depth = self.deeper()?;
        let mut out = Vec::new();
        while let Some(value) = seq.next_element_seed(Strict { depth, at: At::Index(&self.at, out.len()) })? {
            out.push(value);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            // serde_json hands a number over as a map of one member, its digits as written.
            if out.is_empty() && key == "$serde_json::private::Number" {
                let digits: String = map.next_value()?;
                return digits.parse().map(Value::Number).map_err(A::Error::custom);
            }
            let depth = self.deeper()?;
            if out.contains_key(&key) {
                return Err(A::Error::custom(format!("{} is written twice, and no object of a header holds a key twice (docs/FORMAT.md §1): write it once", At::Key(&self.at, &key))));
            }
            let value = map.next_value_seed(Strict { depth, at: At::Key(&self.at, &key) })?;
            out.insert(key, value);
        }
        if out.is_empty() {
            self.deeper()?;
        }
        Ok(Value::Object(out))
    }
}

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

/// Whether a number is written as a whole number, without a fraction or an exponent.
pub(crate) fn whole(number: &Number) -> bool {
    whole_text(&number.to_string())
}

/// Whether a number's text is a whole number, written without a fraction or an exponent.
fn whole_text(text: &str) -> bool {
    !text.contains(['.', 'e', 'E'])
}

/// A number written as a whole number from 0 to 2^64 − 1 (`-0` is 0).
pub(crate) fn natural(value: &Value) -> Option<u64> {
    let text = value.as_number()?.to_string();
    u64::try_from(text.parse::<i128>().ok().filter(|_| whole_text(&text))?).ok()
}

/// Spell a number: a whole number as written, a float in its shortest digits.
fn number(number: &serde_json::Number) -> Result<String> {
    let text = number.to_string();
    if !whole_text(&text) {
        let value = number.as_f64().filter(|value| value.is_finite()).ok_or_else(|| {
            invalid(format!("the number {text} is past the largest double, and a checksum is taken of JSON whose numbers with a fraction or an exponent are doubles (docs/FORMAT.md §8): write a number a double holds"))
        })?;
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
