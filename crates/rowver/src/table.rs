use crate::numeric::{width, NumericArray};
use crate::{invalid, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub kind: String,
    pub jagged: bool,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventIdentity {
    pub sample: String,
    pub file: String,
    pub entry: i64,
    pub uuid: String,
    pub tree: String,
}

#[derive(Debug)]
pub struct NumericBuffer {
    pub kind: String,
    pub count: usize,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Column {
    pub(crate) name: String,
    pub(crate) data: NumericArray,
    pub(crate) offsets: Option<Vec<usize>>,
}

impl Column {
    /// Read a flat event column; object collections require explicit flattening.
    pub fn value(&self, row: usize) -> Result<f64> {
        if self.offsets.is_some() {
            return Err(invalid(format!("{} is an object collection", self.name)));
        }
        self.data.value(row)
    }

    /// Preview flat scalars without losing integer precision in JavaScript.
    pub fn preview_value(&self, row: usize) -> Result<Value> {
        if self.offsets.is_some() {
            return Err(invalid(format!("{} is an object collection", self.name)));
        }
        if self.data.kind.starts_with('f') {
            let number = serde_json::Number::from_f64(self.data.value(row)?)
                .ok_or_else(|| invalid("A scalar preview requires a finite float"))?;
            return Ok(Value::Number(number));
        }
        let value = self.data.integer(row)?;
        if self.data.kind == "b1" {
            return Ok(Value::Bool(value != 0));
        }
        const SAFE_INTEGER: i128 = 9_007_199_254_740_991;
        if (-SAFE_INTEGER..=SAFE_INTEGER).contains(&value) {
            Ok(Value::Number((value as i64).into()))
        } else {
            Ok(Value::String(value.to_string()))
        }
    }

    /// Count objects belonging to an original event row.
    pub fn object_count(&self, row: usize) -> Result<usize> {
        let range = self.range(row)?;
        Ok(range.end - range.start)
    }

    /// Say whether each row holds a list of values rather than one.
    pub fn is_jagged(&self) -> bool {
        self.offsets.is_some()
    }

    /// Read one stored value by its position in the content, as `range` numbers them.
    pub fn content_value(&self, index: usize) -> Result<f64> {
        self.data.value(index)
    }

    /// Find an event's flat value or jagged content interval.
    pub fn range(&self, row: usize) -> Result<std::ops::Range<usize>> {
        if row >= self.len() {
            return Err(invalid(format!(
                "Row {row} is outside column {}",
                self.name
            )));
        }
        Ok(match &self.offsets {
            Some(offsets) => offsets[row]..offsets[row + 1],
            None => row..row + 1,
        })
    }

    /// Return the number of event rows rather than flattened objects.
    pub(crate) fn len(&self) -> usize {
        self.offsets
            .as_ref()
            .map_or(self.data.count, |offsets| offsets.len() - 1)
    }

    /// Materialize selected native buffers with the original dtype and order.
    pub fn buffers_for(&self, rows: &[usize]) -> Result<Vec<NumericBuffer>> {
        let w = width(&self.data.kind)?;
        let selected_count = rows.iter().try_fold(0_usize, |count, row| {
            count
                .checked_add(self.range(*row)?.len())
                .ok_or_else(|| invalid("Selected buffer is too large"))
        })?;
        let size = selected_count
            .checked_mul(w)
            .ok_or_else(|| invalid("Selected buffer byte length overflows"))?;
        let mut bytes = Vec::with_capacity(size);
        let mut offsets = self.offsets.as_ref().map(|_| {
            let mut offsets = Vec::with_capacity(rows.len() + 1);
            offsets.push(0_i64);
            offsets
        });
        let mut count = 0_usize;
        for &row in rows {
            let range = self.range(row)?;
            bytes.extend_from_slice(&self.data.bytes[range.start * w..range.end * w]);
            count = count
                .checked_add(range.len())
                .ok_or_else(|| invalid("Selected buffer is too large"))?;
            if let Some(offsets) = &mut offsets {
                offsets.push(
                    i64::try_from(count)
                        .map_err(|_| invalid("Selected collection exceeds i8 offsets"))?,
                );
            }
        }
        let content = NumericBuffer {
            kind: self.data.kind.clone(),
            count,
            bytes,
        };
        let Some(offsets) = offsets else {
            return Ok(vec![content]);
        };
        let bytes = offsets
            .iter()
            .flat_map(|offset| offset.to_le_bytes())
            .collect();
        Ok(vec![
            NumericBuffer {
                kind: "i8".into(),
                count: offsets.len(),
                bytes,
            },
            content,
        ])
    }
}

#[derive(Debug)]
pub struct Table {
    pub(crate) header: Value,
    pub(crate) columns: Vec<Column>,
    pub(crate) sources: Vec<Source>,
    pub(crate) entries: NumericArray,
}

#[derive(Debug)]
pub(crate) struct Source {
    pub identity: EventIdentity,
    pub start: usize,
    pub end: usize,
}

impl Table {
    /// Build deterministic synthetic events for the first interactive workflow.
    pub fn demo() -> Arc<Self> {
        let rows = 240;
        let nlep: Vec<i32> = (0..rows).map(|row| (row % 5) as i32).collect();
        let met: Vec<f64> = (0..rows)
            .map(|row| 15.0 + (row * 37 % 180) as f64 + (row % 7) as f64 * 0.1)
            .collect();
        let mut offsets = vec![0];
        let mut muons = Vec::<f64>::new();
        for (row, &count) in nlep.iter().enumerate() {
            muons
                .extend((0..count).map(|index| 12.0 + (row * 13 % 90) as f64 + index as f64 * 7.0));
            offsets.push(muons.len());
        }
        Arc::new(Self {
            header: json!({"demo":true,"title":"Synthetic local events"}),
            columns: vec![
                Column {
                    name: "nlep".into(),
                    offsets: None,
                    data: NumericArray {
                        kind: "i4".into(),
                        count: rows,
                        bytes: nlep.iter().flat_map(|value| value.to_le_bytes()).collect(),
                    },
                },
                Column {
                    name: "MET".into(),
                    offsets: None,
                    data: NumericArray {
                        kind: "f8".into(),
                        count: rows,
                        bytes: met.iter().flat_map(|value| value.to_le_bytes()).collect(),
                    },
                },
                Column {
                    name: "Muon_pt".into(),
                    offsets: Some(offsets),
                    data: NumericArray {
                        kind: "f8".into(),
                        count: muons.len(),
                        bytes: muons.iter().flat_map(|value| value.to_le_bytes()).collect(),
                    },
                },
            ],
            sources: vec![Source {
                identity: EventIdentity {
                    sample: "Synthetic".into(),
                    file: "synthetic".into(),
                    entry: 0,
                    uuid: "synthetic".into(),
                    tree: "Events".into(),
                },
                start: 0,
                end: rows,
            }],
            entries: NumericArray {
                kind: "i8".into(),
                count: rows,
                bytes: (0..rows)
                    .flat_map(|row| (row as i64).to_le_bytes())
                    .collect(),
            },
        })
    }

    /// Return the complete input header, including every tool metadata key.
    pub fn header(&self) -> &Value {
        &self.header
    }

    /// Return the number of source events.
    pub fn len(&self) -> usize {
        self.entries.count
    }

    /// Report whether the table has no event rows.
    pub fn is_empty(&self) -> bool {
        self.entries.count == 0
    }

    /// Describe columns in the unchanged checkpoint order.
    pub fn columns(&self) -> Vec<ColumnInfo> {
        self.columns
            .iter()
            .map(|column| ColumnInfo {
                name: column.name.clone(),
                kind: column.data.kind.clone(),
                jagged: column.offsets.is_some(),
                length: column.len(),
            })
            .collect()
    }

    /// Find a column without silently retrieving upstream source branches.
    pub fn column(&self, name: &str) -> Result<&Column> {
        self.columns
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| invalid(format!("Column {name} is absent from this input")))
    }

    /// Preserve the sample, file, ROOT identity, and original entry of a row.
    pub fn identity(&self, row: usize) -> Result<EventIdentity> {
        let source = self
            .sources
            .get(self.sources.partition_point(|source| source.end <= row))
            .filter(|source| source.start <= row && row < source.end)
            .ok_or_else(|| invalid(format!("Source row {row} is out of bounds")))?;
        let mut identity = source.identity.clone();
        identity.entry = self.entries.signed(row)?;
        Ok(identity)
    }
}

#[derive(Clone, Debug)]
pub struct EventView {
    table: Arc<Table>,
    rows: Arc<Vec<usize>>,
}

impl EventView {
    /// Reference every original row without copying column payloads.
    pub fn all(table: Arc<Table>) -> Self {
        let rows = Arc::new((0..table.len()).collect());
        Self { table, rows }
    }

    /// Return original source-row IDs in current view order.
    pub fn rows(&self) -> &[usize] {
        &self.rows
    }

    /// Access the shared immutable source event table.
    pub fn table(&self) -> &Arc<Table> {
        &self.table
    }

    /// Select or reorder unique source rows already present in this view.
    pub fn select_rows(&self, rows: Vec<usize>) -> Result<Self> {
        let parent: HashSet<usize> = self.rows.iter().copied().collect();
        let mut seen = HashSet::new();
        for &row in &rows {
            if !parent.contains(&row) {
                return Err(invalid(format!(
                    "Source row {row} is outside the parent population"
                )));
            }
            if !seen.insert(row) {
                return Err(invalid(format!(
                    "Source row {row} is selected more than once"
                )));
            }
        }
        Ok(Self {
            table: self.table.clone(),
            rows: Arc::new(rows),
        })
    }
}
