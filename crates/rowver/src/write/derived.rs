//! The record of a derived column: a column a file keeps as the expression that computes it,
//! with the checksums and facts of its values, and no array.
//!
//! Documentation: `docs/FORMAT.md` ("derived").

use super::column;
use crate::rvr::Held;
use crate::Result;
use serde_json::{Map, Value};

/// The record of derived column `name`, `expression` giving `column` over samples of `rows` rows,
/// its checksums cut at chunks of `chunk_rows` rows.
pub fn derived(name: &str, expression: &str, column: &Held, rows: &[usize], chunk_rows: usize) -> Result<Value> {
    super::bounded(chunk_rows)?;
    let mut edges = vec![0];
    for n in rows {
        edges.push(edges[edges.len() - 1] + n);
    }
    let measured = column::measured(name, column, &crate::chunks::rows(&edges, chunk_rows), rows.len())?;
    let mut record = Map::new();
    record.insert("name".into(), name.into());
    record.insert("expression".into(), expression.into());
    record.extend(measured.into_iter().filter(|(key, _)| key != "name"));
    Ok(Value::Object(record))
}
