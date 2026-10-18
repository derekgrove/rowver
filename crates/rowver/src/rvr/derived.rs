use super::snapshot::canonical;
use super::{checksum, count, digest, list, object, required, string};
use crate::{invalid, Result};
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

/// The names of a JSON object in the order its text gives them.
struct Keys(Vec<String>);

impl<'de> Deserialize<'de> for Keys {
    /// Collect only the keys, ignoring every value.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Keys;
            /// Describe the object this visitor reads.
            fn expecting(&self, text: &mut std::fmt::Formatter) -> std::fmt::Result {
                text.write_str("an object of column marks")
            }
            /// Read the keys as they come.
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some((key, _)) = map.next_entry::<String, IgnoredAny>()? {
                    keys.push(key);
                }
                Ok(Keys(keys))
            }
        }
        deserializer.deserialize_map(Ordered)
    }
}

#[derive(Deserialize)]
struct Lineage {
    columns: Keys,
}

#[derive(Deserialize)]
struct Ordering {
    derived: Option<Lineage>,
}

/// Read the order in which a header's text names its derived columns, if it is a derived file.
pub(super) fn column_order(text: &[u8]) -> Result<Option<Vec<String>>> {
    let header: Ordering = serde_json::from_slice(text)?;
    Ok(header.derived.map(|lineage| lineage.columns.0))
}

/// Check a derived file's lineage record against its own header.
pub(super) fn validate(
    derived: &Value,
    data: &Value,
    made: &Value,
    order: Option<&[String]>,
) -> Result<()> {
    for field in ["built", "by", "source", "runtime"] {
        string(derived, field)?;
    }
    object(derived, "parameters")?;
    let parent = parent(derived)?;
    for field in ["data_sha256", "made_sha256"] {
        named_checksum(parent, field)?;
    }
    if !required(parent, "derived_sha256")?.is_null() {
        named_checksum(parent, "derived_sha256")?;
    }
    string(parent, "path")?;
    for mark in object(parent, "read")?.as_object().unwrap().values() {
        checksum(&serde_json::json!({ "sha256": mark }))?;
    }
    if string(parent, "made_sha256")? != checksum(made)?
        || count(parent, "rows")? != count(data, "rows")?
    {
        return Err(invalid(
            "The derived file's rows or recipe are not the ones recorded for its parent",
        ));
    }
    let marks = object(derived, "columns")?.as_object().unwrap();
    let names = list(data, "cols")?
        .iter()
        .map(|column| string(column, "name"))
        .collect::<Result<Vec<_>>>()?;
    if order.is_none_or(|order| order != names) {
        return Err(invalid(
            "derived.columns does not name the file's columns in order",
        ));
    }
    for mark in marks.values() {
        checksum(&serde_json::json!({ "sha256": mark }))?;
    }
    let mut body = derived.clone();
    body.as_object_mut().unwrap().remove("sha256");
    if digest(canonical(&body)?.as_bytes()) != checksum(derived)? {
        return Err(invalid(
            "derived.sha256 does not match the complete lineage record",
        ));
    }
    Ok(())
}

/// Confirm from two headers that `parent` is the file `derived` was computed from.
pub fn confirm_parent(derived: &Value, parent: &Value) -> Result<()> {
    let record = self::parent(object(derived, "derived")?)?;
    let (own, above) = (object(derived, "data")?, object(parent, "data")?);
    let lineage = parent.get("derived").map(checksum).transpose()?;
    if string(record, "data_sha256")? != checksum(above)?
        || string(record, "made_sha256")? != checksum(object(parent, "made")?)?
        || required(record, "derived_sha256")?.as_str() != lineage
        || derived.get("made") != parent.get("made")
    {
        return Err(invalid(
            "This is not the parent the derived file was computed from",
        ));
    }
    for field in ["rows", "samples", "files", "sample_cols"] {
        if required(own, field)? != required(above, field)? {
            return Err(invalid(format!(
                "The parent's {field} differ from the derived file's"
            )));
        }
    }
    for field in ["sample", "entry"] {
        if checksum(object(own, field)?)? != checksum(object(above, field)?)? {
            return Err(invalid(format!(
                "The parent's {field} array differs from the derived file's"
            )));
        }
    }
    let held = column_checksums(above)?;
    for (name, mark) in object(record, "read")?.as_object().unwrap() {
        if held
            .iter()
            .all(|(other, sum)| other != name || Some(sum.as_str()) != mark.as_str())
        {
            return Err(invalid(format!(
                "The parent's column {name} is not the one that was read"
            )));
        }
    }
    for column in list(own, "cols")? {
        let name = string(column, "name")?;
        if held.iter().any(|(other, _)| other == name) {
            return Err(invalid(format!("The parent already holds {name}")));
        }
    }
    Ok(())
}

/// Name every column and sample column of a file with the one checksum lineage records for it.
fn column_checksums(data: &Value) -> Result<Vec<(String, String)>> {
    let mut held = Vec::new();
    for column in list(data, "cols")? {
        let sum = if string(column, "kind")? == "jagged" {
            let offsets = checksum(object(column, "offsets")?)?;
            digest(format!("{offsets}{}", checksum(object(column, "content")?)?).as_bytes())
        } else {
            checksum(column)?.to_owned()
        };
        held.push((string(column, "name")?.to_owned(), sum));
    }
    for column in list(data, "sample_cols")? {
        held.push((
            string(column, "name")?.to_owned(),
            checksum(column)?.to_owned(),
        ));
    }
    Ok(held)
}

/// Obtain the single parent a derived file names.
fn parent(derived: &Value) -> Result<&Value> {
    match list(derived, "parents")?.as_slice() {
        [parent] if parent.is_object() => Ok(parent),
        _ => Err(invalid("A derived file names exactly one parent")),
    }
}

/// Require a named lowercase SHA-256 field.
fn named_checksum(value: &Value, key: &str) -> Result<()> {
    checksum(&serde_json::json!({ "sha256": required(value, key)? })).map(drop)
}
