//! rowver's canonical JSON, which `made.sha256` and a signature are taken of:
//! keys sorted, no spaces, UTF-8, whole numbers as written, floats in their shortest digits.

use crate::{invalid, Error, Result};
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::cell::Cell;

/// How deep a header's arrays and objects may nest, the header itself the first.
const DEEPEST: usize = 64;

/// A rule of docs/FORMAT.md §1 a header breaks: the fact, and the top-level key it lies under.
type Broken = Option<(String, Option<String>)>;

/// Parse a header's JSON, refusing what a plain parse lets through (docs/FORMAT.md §1): an object
/// holding a key twice, arrays and objects nested deeper than 64, or a whole number written -0.
pub(crate) fn strict(text: &[u8]) -> Result<Value> {
    let broken = Cell::new(None);
    let mut parsed = serde_json::Deserializer::from_slice(text);
    let value = Strict { depth: 1, at: At::Top, broken: &broken }.deserialize(&mut parsed).map_err(|e| match broken.take() {
        Some((fact, under)) => Error::Breaks { fact, under, line: e.line(), column: e.column() },
        None => e.into(),
    })?;
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

impl<'a> At<'a> {
    /// The top-level key this value lies under.
    fn top(&self) -> Option<&'a str> {
        match *self {
            At::Top => None,
            At::Key(At::Top, key) => Some(key),
            At::Key(parent, _) | At::Index(parent, _) => parent.top(),
        }
    }

    /// Whether this object's keys are names, as `made.files` and a sample's `files` hold.
    fn names(&self) -> bool {
        matches!(self, At::Key(At::Key(At::Top, "made"), "files") | At::Key(At::Key(At::Key(At::Key(At::Top, "made"), "files"), _), "files"))
    }
}

impl std::fmt::Display for At<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // A name, or a key not of plain letters, digits and `_`, is quoted; a field is not.
        let field = |key: &str| !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        match self {
            At::Top => f.write_str("the header"),
            At::Key(At::Top, key) if field(key) => f.write_str(key),
            At::Key(At::Top, key) => write!(f, "[{key:?}]"),
            At::Key(parent, key) if field(key) && !parent.names() => write!(f, "{parent}.{key}"),
            At::Key(parent, key) => write!(f, "{parent}[{key:?}]"),
            At::Index(parent, index) => write!(f, "{parent}[{index}]"),
        }
    }
}

/// One value of the header, `depth` arrays and objects deep, at `at`; `broken` holds the rule it
/// breaks, when it breaks one.
#[derive(Clone, Copy)]
struct Strict<'a> {
    depth: usize,
    at: At<'a>,
    broken: &'a Cell<Broken>,
}

impl Strict<'_> {
    /// Refuse the value at `at`, which breaks a rule of §1 as `fact` says.
    fn breaks<E: serde::de::Error>(self, at: At, fact: &str) -> E {
        let fact = format!("{at} {fact}");
        let error = E::custom(&fact);
        self.broken.set(Some((fact, at.top().map(str::to_owned))));
        error
    }

    /// The depth of what an array or object holds, refused when it is too deep itself.
    fn deeper<E: serde::de::Error>(self) -> std::result::Result<usize, E> {
        match self.depth > DEEPEST {
            true => Err(self.breaks(self.at, &format!("is nested {} arrays and objects deep, more than {DEEPEST}", self.depth))),
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
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> std::result::Result<Value, E> {
        // Only a number written with a `-` comes here; serde_json before 1.0.154 hands -0 over as 0.
        match value {
            0 => Err(self.breaks(self.at, "is written -0")),
            _ => Ok(value.into()),
        }
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
        while let Some(value) = seq.next_element_seed(Strict { depth, at: At::Index(&self.at, out.len()), ..self })? {
            out.push(value);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            // With arbitrary_precision (rowver's Cargo.toml turns it on), serde_json hands a number
            // no i64 or u64 holds over as a map of one member, its digits as written; from 1.0.154
            // on, -0 too.
            if out.is_empty() && key == "$serde_json::private::Number" {
                let digits: String = map.next_value()?;
                if digits == "-0" {
                    return Err(self.breaks(self.at, "is written -0"));
                }
                return digits.parse().map(Value::Number).map_err(A::Error::custom);
            }
            let depth = self.deeper()?;
            if out.contains_key(&key) {
                return Err(self.breaks(At::Key(&self.at, &key), "is written twice"));
            }
            let value = map.next_value_seed(Strict { depth, at: At::Key(&self.at, &key), ..self })?;
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

/// A number written as a whole number from 0 to 2^64 − 1.
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
    Ok(text)
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

#[cfg(test)]
mod tests {
    use super::*;

    // FORMAT 1
    #[test]
    fn a_whole_number_written_minus_zero_is_refused_wherever_it_is() {
        let refused = |text: &str| strict(text.as_bytes()).unwrap_err().to_string();
        assert_eq!(refused("-0"), "the header breaks docs/FORMAT.md §1: the header is written -0 (line 1, column 2): the program that wrote it does not follow the format; press its study again with rvr");
        assert!(refused(r#"{"data": {"files": [{"rows": 1}, {"rows": -0}]}}"#).contains(": data.files[1].rows is written -0 ("));
        assert!(refused(r#"{"tool": {"x": [1, -0]}}"#).contains(": tool.x[1] is written -0 ("));
        // -0.0 and -0e0 are floats, and allowed; so is a negative whole number.
        for text in ["-0.0", "-0e0", "-1", "0", r#"{"a": [-0.0, -0e0]}"#] {
            assert!(strict(text.as_bytes()).is_ok(), "{text}");
        }
    }

    // FORMAT 1
    #[test]
    fn a_refusal_names_a_sample_a_file_and_a_key_not_of_plain_letters_quoted() {
        let refused = |text: &str| strict(text.as_bytes()).unwrap_err().to_string();
        let at = |text: &str| refused(text).split_once(": ").unwrap().1.split_once(" (line").unwrap().0.to_owned();
        assert_eq!(at(r#"{"made": {"files": {"A": {"files": {"./f.root": {"stop": -0}}}}}}"#), r#"made.files["A"].files["./f.root"].stop is written -0"#);
        assert_eq!(at(r#"{"made": {"files": {"A": {"metadata": {"big": -0}}}}}"#), r#"made.files["A"].metadata.big is written -0"#);
        assert_eq!(at(r#"{"tool.x": {"a.b": [1, -0]}}"#), r#"["tool.x"]["a.b"][1] is written -0"#);
        assert_eq!(at(r#"{"t": {"a\"b": 1, "a\"b": 2}}"#), r#"t["a\"b"] is written twice"#);
    }

    // FORMAT 1
    #[test]
    fn a_header_breaking_a_rule_of_section_1_is_a_writer_s_mistake_not_damage() {
        let said = |text: &str| strict(text.as_bytes()).unwrap_err().to_string();
        let deep = format!("{}{}", "[".repeat(DEEPEST + 1), "]".repeat(DEEPEST + 1));
        for text in ["-0", r#"{"a": 1, "a": 2}"#, &deep] {
            let said = said(text);
            assert!(said.starts_with("the header breaks docs/FORMAT.md §1: "), "{said}");
            assert!(said.ends_with("): the program that wrote it does not follow the format; press its study again with rvr"), "{said}");
        }
        assert!(said(&deep).contains(" is nested 65 arrays and objects deep, more than 64 (line 1, column 66)"), "{}", said(&deep));
        // Only Strict's own refusals break §1; text that does not parse is not JSON.
        for text in [r#"{"a": "#, "{\"a\": 1} x", "[1,]", "\u{feff}{}", r#"{"a": "\ud800"}"#] {
            assert!(said(text).starts_with("the header is not JSON as docs/FORMAT.md §1 reads it ("), "{}", said(text));
        }
    }

    // FORMAT 1
    #[test]
    fn each_refusal_of_a_header_names_its_file() {
        let about = |text: &str| crate::about(std::path::Path::new("f.rvr"), strict(text.as_bytes()).unwrap_err()).to_string();
        assert!(about("-0").starts_with("f.rvr: the header breaks docs/FORMAT.md §1: "), "{}", about("-0"));
        assert!(about("[1,]").starts_with("f.rvr: the header is not JSON as docs/FORMAT.md §1 reads it ("), "{}", about("[1,]"));
    }

    // FORMAT 1
    #[test]
    fn a_header_rowver_was_about_to_write_that_breaks_section_1_says_what_put_it_there() {
        let deep = (0..DEEPEST).fold(serde_json::json!(1), |inner, _| serde_json::json!([inner]));
        let said = |header: Value| crate::rvr::Reader::laid(std::path::Path::new("."), &[], header).err().unwrap().to_string();
        let tool = said(serde_json::json!({"rowver": 15, "deep-tool": {"x": deep}}));
        assert!(tool.starts_with("the header rowver was about to write breaks docs/FORMAT.md §1: [\"deep-tool\"].x"), "{tool}");
        assert!(tool.ends_with(" is nested 65 arrays and objects deep, more than 64, in the tool key deep-tool it was given: change that tool key's data to follow §1"), "{tool}");
        let made = said(serde_json::json!({"rowver": 15, "made": {"x": deep}}));
        assert!(made.ends_with(", in what it was given to write: change that to follow §1"), "{made}");
    }
}
