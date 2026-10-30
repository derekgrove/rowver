//! The native part of the Python package `rowver`: an open `.rvr` file's header and columns,
//! handed to NumPy without a copy, and the `rvr` command itself. `python/rowver` builds the
//! Python API on these.

use numpy::{IntoPyArray, PyArray1};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use rowver::rvr::{Held, Reader};
use std::path::PathBuf;

pyo3::create_exception!(
    rowver,
    Error,
    PyException,
    "A .rvr file that cannot be read, or a problem found in one."
);

fn failed(error: impl std::fmt::Display) -> PyErr {
    Error::new_err(error.to_string())
}

/// One column as the file holds it: its kind, its values as little-endian bytes, and for a list
/// column each row's start in the values, then the end of the last.
type Column<'py> = (
    String,
    Bound<'py, PyArray1<u8>>,
    Option<Bound<'py, PyArray1<i64>>>,
);

fn handed(py: Python<'_>, held: Held) -> Column<'_> {
    let offsets = held.offsets.map(|offsets| {
        offsets
            .into_iter()
            .map(|o| o as i64)
            .collect::<Vec<_>>()
            .into_pyarray(py)
    });
    (held.kind, held.bytes.into_pyarray(py), offsets)
}

/// An open `.rvr` file: its header read and checked, its arrays read when asked for.
#[pyclass(frozen, module = "rowver._rowver")]
struct File {
    reader: Reader,
}

impl File {
    /// Say which file a problem is in.
    fn located(&self, error: impl std::fmt::Display) -> PyErr {
        failed(format!("{}: {error}", self.reader.path().display()))
    }
}

#[pymethods]
impl File {
    #[new]
    fn new(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        if !path.exists() {
            let missing = format!("{}: no such file", path.display());
            return Err(pyo3::exceptions::PyFileNotFoundError::new_err(missing));
        }
        let reader = py
            .detach(|| Reader::open(&path))
            .map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(File { reader })
    }

    /// The JSON header, as the file holds it.
    #[getter]
    fn header_text(&self) -> String {
        serde_json::to_string(self.reader.header()).expect("a header read from JSON writes as JSON")
    }

    #[getter]
    fn rows(&self) -> PyResult<usize> {
        self.reader.rows().map_err(|e| self.located(e))
    }

    /// Each row's index into the file's samples.
    fn sample<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray1<i32>>> {
        let values = py
            .detach(|| self.reader.sample())
            .map_err(|e| self.located(e))?;
        Ok(values.into_pyarray(py))
    }

    /// Each row's entry number in its file's tree.
    fn entry<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray1<i64>>> {
        let values = py
            .detach(|| self.reader.entry())
            .map_err(|e| self.located(e))?;
        Ok(values.into_pyarray(py))
    }

    /// The columns of these names: the stored ones read at once on every core, then the derived
    /// ones worked out together; `None` for a name the file holds no column of.
    fn columns<'py>(
        &self,
        py: Python<'py>,
        names: Vec<String>,
    ) -> PyResult<Vec<Option<Column<'py>>>> {
        let listed = rowver::rvr::derived::listed(self.reader.header());
        let derived: Vec<String> = listed.into_iter().map(|(name, _)| name).filter(|name| names.contains(name)).collect();
        let read = py.detach(|| {
            let stored = rowver::parallel::map(&names, rowver::parallel::cores(), |name| match derived.contains(name) {
                true => Ok(None),
                false => self.reader.column(name).map_err(|e| e.to_string()),
            })?;
            let worked = rowver::rvr::derived::columns(&self.reader, &derived).map_err(|e| e.to_string())?;
            let mut worked: std::collections::HashMap<&String, _> = derived.iter().zip(worked).collect();
            Ok::<_, String>(names.iter().zip(stored).map(|(name, held)| worked.remove(name).or(held)).collect::<Vec<_>>())
        });
        Ok(read
            .map_err(|e| self.located(e))?
            .into_iter()
            .map(|held| held.map(|h| handed(py, h)))
            .collect())
    }
}

/// Check a file completely, as `rvr validate` does: its rows and columns, or `Error` saying what
/// is wrong.
#[pyfunction]
fn validate(py: Python<'_>, path: PathBuf) -> PyResult<(usize, usize)> {
    let checked = py.detach(|| rowver::rvr::validate_file(&path));
    let summary = checked.map_err(|e| failed(format!("{}: {e}", path.display())))?;
    Ok((summary.rows, summary.columns))
}

/// Run `rvr` with these arguments, as the command does; its exit status.
#[pyfunction(name = "rvr")]
fn run_rvr(py: Python<'_>, args: Vec<String>) -> u8 {
    py.detach(|| {
        let code = ::rvr::run(&args);
        // A Rust binary flushes its standard output on exit; inside Python nothing would.
        let _ = std::io::Write::flush(&mut std::io::stdout());
        code
    })
}

/// Make a closed output pipe end the process quietly, as the `rvr` command does.
#[pyfunction]
fn quiet_on_closed_pipe() {
    ::rvr::quiet_on_closed_pipe();
}

#[pymodule]
fn _rowver(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<File>()?;
    module.add("Error", module.py().get_type::<Error>())?;
    module.add_function(wrap_pyfunction!(validate, module)?)?;
    module.add_function(wrap_pyfunction!(run_rvr, module)?)?;
    module.add_function(wrap_pyfunction!(quiet_on_closed_pipe, module)?)?;
    Ok(())
}
