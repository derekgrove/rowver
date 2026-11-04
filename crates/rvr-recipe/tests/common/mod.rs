//! What the tests share: columns held by name.

use rvr_recipe::{Array, Columns};
use std::collections::BTreeMap;

/// Columns held by name.
pub struct Held(pub BTreeMap<String, Array>);

impl Columns for Held {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        Ok(self.0.get(name).cloned())
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

/// A recipe of `body` after its format and version, each of the five tables it leaves out written in empty, and a column stored when it has no `[columns]`.
pub fn complete(body: &str) -> String {
    let mut text = format!("format = \"rvr-recipe\"\nversion = 1\n{body}");
    for table in rvr_recipe::TABLES {
        if !body.lines().any(|line| line.trim() == format!("[{table}]")) {
            text.push_str(&format!("\n[{table}]\n"));
            if table == "columns" {
                text.push_str("branches = [\"stored\"]\n");
            }
        }
    }
    text
}
