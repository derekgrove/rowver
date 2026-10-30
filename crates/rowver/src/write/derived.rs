//! The record of a derived column: a column a file keeps as the expression that computes it,
//! with the checksums and facts of its values, and no array.
//!
//! Documentation: `docs/FORMAT.md` ("derived").

use super::column;
use crate::rvr::Held;
use crate::Result;
use serde_json::{Map, Value};

/// The record of derived column `name`, `expression` giving `column` over samples of `rows` rows.
pub fn derived(name: &str, expression: &str, column: &Held, rows: &[usize]) -> Result<Value> {
    let mut edges = vec![0];
    for n in rows {
        edges.push(edges[edges.len() - 1] + n);
    }
    let measured = column::measured(name, column, &edges)?;
    let mut record = Map::new();
    record.insert("name".into(), name.into());
    record.insert("expression".into(), expression.into());
    record.extend(measured.into_iter().filter(|(key, _)| key != "name"));
    Ok(Value::Object(record))
}
