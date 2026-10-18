use super::{confirm_parent, decode, read_header};
use crate::{invalid, Column, ColumnInfo, Error, EventIdentity, Result, Table};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One set of rows whose columns come from a file and, when it is derived, the ancestors it names.
#[derive(Debug)]
pub struct Stack {
    /// The opened file first, then its parent, and so on up to the pressed file.
    files: Vec<Arc<Table>>,
}

impl Stack {
    /// Open a file and every ancestor it names, each confirmed by checksum before it is used.
    ///
    /// A parent is looked for at its recorded path (a relative one counted from its child's
    /// folder), beside its child under the recorded file name, and then in `elsewhere`, where a
    /// folder stands for that file name inside it. A candidate that is damaged is passed over.
    pub fn open(path: impl AsRef<Path>, elsewhere: &[PathBuf]) -> Result<Self> {
        let mut at = path.as_ref().to_path_buf();
        let mut files = vec![decode(&std::fs::read(&at)?)?];
        while let Some(record) = files.last().unwrap().header().get("derived") {
            let hint = record["parents"][0]["path"].as_str().unwrap_or_default();
            let child = files.last().unwrap().clone();
            let found = candidates(&at, hint, elsewhere)
                .into_iter()
                .find_map(|candidate| {
                    let above = read_header(&candidate).ok()?;
                    confirm_parent(child.header(), &above).ok()?;
                    let above = decode(&std::fs::read(&candidate).ok()?).ok()?;
                    confirm_parent(child.header(), above.header()).ok()?;
                    Some((candidate, above))
                });
            let (place, above) = found.ok_or_else(|| Error::Parent {
                child: at.display().to_string(),
                hint: hint.to_owned(),
            })?;
            at = place;
            files.push(above);
        }
        Ok(Self { files })
    }

    /// Return the number of rows every file of the stack shares.
    pub fn len(&self) -> usize {
        self.files[0].len()
    }

    /// Report whether the stack has no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// List the files of the stack, the opened one first.
    pub fn files(&self) -> &[Arc<Table>] {
        &self.files
    }

    /// Find a column in the nearest file that holds it.
    pub fn column(&self, name: &str) -> Result<&Column> {
        let held = self.files.iter().find_map(|file| file.column(name).ok());
        held.ok_or_else(|| {
            invalid(format!(
                "Column {name} is absent from this file and its parents"
            ))
        })
    }

    /// Describe every reachable column once, nearest file first.
    pub fn columns(&self) -> Vec<ColumnInfo> {
        let mut columns: Vec<ColumnInfo> = Vec::new();
        for column in self.files.iter().flat_map(|file| file.columns()) {
            if columns.iter().all(|known| known.name != column.name) {
                columns.push(column);
            }
        }
        columns
    }

    /// Name the columns a nearer file holds under a name an ancestor also uses, hiding the ancestor's.
    pub fn shadowed(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut shadowed = Vec::new();
        for column in self.files.iter().flat_map(|file| file.columns()) {
            if !seen.contains(&column.name) {
                seen.push(column.name);
            } else if !shadowed.contains(&column.name) {
                shadowed.push(column.name);
            }
        }
        shadowed
    }

    /// Preserve the sample, file, ROOT identity, and original entry of a row.
    pub fn identity(&self, row: usize) -> Result<EventIdentity> {
        self.files[0].identity(row)
    }
}

/// List where a parent may be, in the order the places are tried.
fn candidates(child: &Path, hint: &str, elsewhere: &[PathBuf]) -> Vec<PathBuf> {
    let name = Path::new(hint).file_name();
    let folder = child.parent().unwrap_or(Path::new(""));
    let recorded = (!hint.is_empty()).then(|| folder.join(hint));
    let beside = name.map(|name| folder.join(name));
    let chosen = elsewhere.iter().filter_map(|place| {
        if place.is_dir() {
            name.map(|name| place.join(name))
        } else {
            Some(place.clone())
        }
    });
    recorded.into_iter().chain(beside).chain(chosen).collect()
}

/// List a file and every ancestor it names, each with its header, confirmed from headers alone.
///
/// No array is read: use this to plan a read, and let the reader check the arrays it takes.
pub fn ancestry(
    path: impl AsRef<Path>,
    elsewhere: &[PathBuf],
) -> Result<Vec<(PathBuf, serde_json::Value)>> {
    let mut at = path.as_ref().to_path_buf();
    let mut files = vec![(at.clone(), read_header(&at)?)];
    while let Some(record) = files.last().unwrap().1.get("derived") {
        let hint = record["parents"][0]["path"].as_str().unwrap_or_default();
        let child = &files.last().unwrap().1;
        let found = candidates(&at, hint, elsewhere)
            .into_iter()
            .find_map(|candidate| {
                let above = read_header(&candidate).ok()?;
                confirm_parent(child, &above).ok()?;
                Some((candidate, above))
            });
        let found = found.ok_or_else(|| Error::Parent {
            child: at.display().to_string(),
            hint: hint.to_owned(),
        })?;
        at = found.0.clone();
        files.push(found);
    }
    Ok(files)
}
