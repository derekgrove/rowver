//! The native part of the Python package `rowver`: an open `.rvr` file's header and columns, each
//! column's bytes handed to NumPy as read (a list column's offsets copied into `i64`), and the `rvr`
//! command itself. `python/rowver` builds the Python API on these.

use numpy::{IntoPyArray, PyArray1};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
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

/// Columns read together, handed to NumPy: list columns that share their offsets
/// (docs/FORMAT.md §7) are handed one array of them.
fn handed(py: Python<'_>, read: Vec<Option<Held>>) -> Vec<Option<Column<'_>>> {
    let mut shared: Vec<(std::sync::Arc<Vec<usize>>, Bound<'_, PyArray1<i64>>)> = Vec::new();
    let mut one = |mut held: Held| {
        let offsets = held.offsets.take().map(|offsets| {
            match shared
                .iter()
                .find(|(known, _)| std::sync::Arc::ptr_eq(known, &offsets))
            {
                Some((_, array)) => array.clone(),
                None => {
                    let array = counted(py, &offsets);
                    shared.push((offsets, array.clone()));
                    array
                }
            }
        });
        (held.kind, held.bytes.into_pyarray(py), offsets)
    };
    read.into_iter().map(|held| held.map(&mut one)).collect()
}

/// A list column's offsets as NumPy holds them.
fn counted<'py>(py: Python<'py>, offsets: &[usize]) -> Bound<'py, PyArray1<i64>> {
    offsets
        .iter()
        .map(|o| *o as i64)
        .collect::<Vec<_>>()
        .into_pyarray(py)
}

/// A row number as Python gives it, any whole number: past what an i64 holds, the most one holds of its sign.
fn row(at: &Bound<'_, PyAny>) -> PyResult<i64> {
    match at.extract::<i64>() {
        Err(e) if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(at.py()) => {
            Ok(if at.lt(0)? { i64::MIN } else { i64::MAX })
        }
        held => held,
    }
}

/// An open `.rvr` file: its header read and checked, its arrays read when asked for.
#[pyclass(frozen, module = "rowver._rowver")]
struct File {
    reader: Reader,
}

impl File {
    /// A problem reading this file, in plain words.
    fn located(&self, error: &rowver::Error) -> PyErr {
        failed(rowver::plain(self.reader.path(), error))
    }
}

#[pymethods]
impl File {
    /// Open the file at `path`, checking its header.
    #[new]
    fn new(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        match py.detach(|| Reader::open(&path)) {
            Ok(reader) => Ok(File {
                reader: reader.trusting(),
            }),
            Err(rowver::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(
                pyo3::exceptions::PyFileNotFoundError::new_err(rowver::plain_io(&path, &e)),
            ),
            Err(e) => Err(failed(rowver::plain(&path, &e))),
        }
    }

    /// The JSON header's bytes, as the file holds them.
    #[getter]
    fn header_text<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let text = self.reader.header_text().map_err(|e| self.located(&e))?;
        Ok(PyBytes::new(py, &text))
    }

    /// The names of the file's tool keys (docs/FORMAT.md §11), in the header's order.
    fn tools(&self) -> Vec<String> {
        rowver::rvr::tool_names(self.reader.header())
            .map(str::to_owned)
            .collect()
    }

    /// Every hole the header records, as (sample, file, first entry, entry after the last, kind, why).
    fn holes(&self) -> Vec<(String, String, u64, u64, String, String)> {
        rowver::rvr::holes(self.reader.header())
            .into_iter()
            .map(|h| (h.sample, h.file, h.first, h.last, h.kind, h.why))
            .collect()
    }

    /// Whether the file is empty: its header alone, its columns let go by `rvr empty`.
    #[getter]
    fn empty(&self) -> bool {
        self.reader.empty()
    }

    /// Rows `start..stop` of the columns (or `sample`, `entry`) of these names, the columns' chunks
    /// that hold them decoded in one pool of threads; `None` for a name the file holds no column of.
    fn rows<'py>(
        &self,
        py: Python<'py>,
        names: Vec<String>,
        start: &Bound<'py, PyAny>,
        stop: &Bound<'py, PyAny>,
    ) -> PyResult<Vec<Option<Column<'py>>>> {
        let (start, stop) = self
            .reader
            .asked(&names.join(", "), row(start)?, row(stop)?)
            .map_err(|e| self.located(&e))?;
        let read = py.detach(|| self.reader.slices(&names, start, stop));
        Ok(handed(py, read.map_err(|e| self.located(&e))?))
    }

    /// Each chunk's first row and the row after its last, in order (docs/FORMAT.md §7).
    fn chunks(&self) -> PyResult<Vec<(usize, usize)>> {
        self.reader.chunk_spans().map_err(|e| self.located(&e))
    }

    /// The columns (or `sample`, `entry`) of these names: every chunk of the stored ones decoded in one pool of
    /// threads, then the derived ones worked out together; `None` for a name the file holds no column of.
    fn columns<'py>(
        &self,
        py: Python<'py>,
        names: Vec<String>,
    ) -> PyResult<Vec<Option<Column<'py>>>> {
        let read = py.detach(|| self.reader.columns(&names));
        Ok(handed(py, read.map_err(|e| self.located(&e))?))
    }
}

/// Check a file completely, as `rvr validate` does: its rows and columns, or `Error` saying what
/// is wrong.
#[pyfunction]
fn validate(py: Python<'_>, path: PathBuf) -> PyResult<(usize, usize)> {
    let checked = py.detach(|| rowver::rvr::validate_file(&path));
    let summary = checked.map_err(|e| failed(rowver::plain(&path, &e)))?;
    Ok((summary.rows, summary.columns))
}

/// Make the JSON object `text` holds the tool key `key` of the file at `path` (docs/SPEC.md §11.9).
#[pyfunction]
fn set_tool(py: Python<'_>, path: PathBuf, key: String, text: Vec<u8>) -> PyResult<()> {
    // The value's own failures are its, not the file's: "the value given for the tool key k …"; a name refused is the file's.
    let value =
        rowver::change::tool_data(&key, &text).map_err(|e| match rowver::rvr::tool_key(&key) {
            Ok(()) => failed(format!("the value given for {e}")),
            Err(_) => failed(rowver::plain(&path, &e)),
        })?;
    let set = || rowver::change::set_tool(&rowver::change::Locked::take(&path)?, &key, value);
    py.detach(set).map_err(|e| failed(rowver::plain(&path, &e)))
}

/// Drop the tool keys `keys` of the file at `path` in one change: each one's bytes as the header held them, every copy of a key held more than once.
#[pyfunction]
fn drop_tool<'py>(
    py: Python<'py>,
    path: PathBuf,
    keys: Vec<String>,
) -> PyResult<Vec<Vec<Bound<'py, PyBytes>>>> {
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let dropped = || rowver::change::drop_tool(&rowver::change::Locked::take(&path)?, &keys);
    let dropped = py
        .detach(dropped)
        .map_err(|e| failed(rowver::plain(&path, &e)))?;
    Ok(dropped
        .iter()
        .map(|copies| copies.iter().map(|bytes| PyBytes::new(py, bytes)).collect())
        .collect())
}

/// One argument to `rvr`: text, or the bytes the system gave (`os.fsencode`).
#[derive(FromPyObject)]
enum Arg {
    Bytes(Vec<u8>),
    Text(String),
}

/// Run `rvr` with these arguments, as the command does; its exit status.
#[pyfunction(name = "rvr")]
fn run_rvr(py: Python<'_>, args: Vec<Arg>) -> u8 {
    let args: Vec<std::ffi::OsString> = args
        .into_iter()
        .map(|arg| match arg {
            Arg::Text(text) => text.into(),
            #[cfg(unix)]
            Arg::Bytes(bytes) => std::os::unix::ffi::OsStringExt::from_vec(bytes),
            #[cfg(not(unix))]
            Arg::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned().into(),
        })
        .collect();
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
    module.add_function(wrap_pyfunction!(set_tool, module)?)?;
    module.add_function(wrap_pyfunction!(drop_tool, module)?)?;
    module.add_function(wrap_pyfunction!(run_rvr, module)?)?;
    module.add_function(wrap_pyfunction!(quiet_on_closed_pipe, module)?)?;
    Ok(())
}
