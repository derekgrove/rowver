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

/// A row number as Python gives it, any whole number: past what an i64 holds, the most one holds of its sign.
fn row(at: &Bound<'_, PyAny>) -> PyResult<i64> {
    match at.extract::<i64>() {
        Err(e) if e.is_instance_of::<pyo3::exceptions::PyOverflowError>(at.py()) => Ok(if at.lt(0)? { i64::MIN } else { i64::MAX }),
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
            Ok(reader) => Ok(File { reader }),
            Err(rowver::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(pyo3::exceptions::PyFileNotFoundError::new_err(rowver::plain_io(&path, &e))),
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
        rowver::rvr::tool_names(self.reader.header()).map(str::to_owned).collect()
    }

    /// Every hole the header records, as (sample, file, first entry, entry after the last, kind, why).
    fn holes(&self) -> Vec<(String, String, u64, u64, String, String)> {
        rowver::rvr::holes(self.reader.header()).into_iter().map(|h| (h.sample, h.file, h.first, h.last, h.kind, h.why)).collect()
    }

    /// Whether the file is empty: its header alone, its columns let go by `rvr empty`.
    #[getter]
    fn empty(&self) -> bool {
        self.reader.empty()
    }

    /// Each row's index into the file's samples.
    fn sample<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray1<i32>>> {
        let values = py
            .detach(|| self.reader.sample())
            .map_err(|e| self.located(&e))?;
        Ok(values.into_pyarray(py))
    }

    /// Each row's entry number in its file's tree.
    fn entry<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray1<i64>>> {
        let values = py
            .detach(|| self.reader.entry())
            .map_err(|e| self.located(&e))?;
        Ok(values.into_pyarray(py))
    }

    /// Rows `start..stop` of the column (or `sample`, `entry`) of this name, only the chunks holding
    /// them read; `None` for a name the file holds no column of.
    fn slice<'py>(&self, py: Python<'py>, name: String, start: &Bound<'py, PyAny>, stop: &Bound<'py, PyAny>) -> PyResult<Option<Column<'py>>> {
        let (start, stop) = self.reader.asked(&name, row(start)?, row(stop)?).map_err(|e| self.located(&e))?;
        let read = py.detach(|| match name.as_str() {
            "sample" => self.reader.sample_slice(start, stop).map(|v| Some(Held { kind: "i4".into(), bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(), offsets: None })),
            "entry" => self.reader.entry_slice(start, stop).map(|v| Some(Held { kind: "i8".into(), bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(), offsets: None })),
            _ => self.reader.slice(&name, start, stop),
        });
        Ok(read.map_err(|e| self.located(&e))?.map(|held| handed(py, held)))
    }

    /// The columns of these names: every chunk of the stored ones decoded in one pool of threads,
    /// then the derived ones worked out together; `None` for a name the file holds no column of.
    fn columns<'py>(
        &self,
        py: Python<'py>,
        names: Vec<String>,
    ) -> PyResult<Vec<Option<Column<'py>>>> {
        let listed = rowver::rvr::derived::listed(self.reader.header());
        let derived: Vec<String> = listed.into_iter().map(|(name, _)| name).filter(|name| names.contains(name)).collect();
        let read = py.detach(|| {
            let plain = |e: rowver::Error| rowver::plain(self.reader.path(), &e);
            let stored = self.reader.stored_columns(&names).map_err(plain)?;
            let worked = rowver::rvr::derived::columns(&self.reader, &derived).map_err(plain)?;
            let mut worked: std::collections::HashMap<&String, _> = derived.iter().zip(worked).collect();
            Ok::<_, String>(names.iter().zip(stored).map(|(name, held)| worked.remove(name).or(held)).collect::<Vec<_>>())
        });
        Ok(read
            .map_err(failed)?
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
    let summary = checked.map_err(|e| failed(rowver::plain(&path, &e)))?;
    Ok((summary.rows, summary.columns))
}

/// Make the JSON object `text` holds the tool key `key` of the file at `path` (docs/SPEC.md §11.9).
#[pyfunction]
fn set_tool(py: Python<'_>, path: PathBuf, key: String, text: Vec<u8>) -> PyResult<()> {
    // The value's own failures are its, not the file's: "the value given for the tool key k …"; a name refused is the file's.
    let value = rowver::change::tool_data(&key, &text).map_err(|e| match rowver::rvr::tool_key(&key) {
        Ok(()) => failed(format!("the value given for {e}")),
        Err(_) => failed(rowver::plain(&path, &e)),
    })?;
    let set = || rowver::change::set_tool(&rowver::change::Locked::take(&path)?, &key, value);
    py.detach(set).map_err(|e| failed(rowver::plain(&path, &e)))
}

/// Drop the tool keys `keys` of the file at `path` in one change: each object's text, as the header held it.
#[pyfunction]
fn drop_tool(py: Python<'_>, path: PathBuf, keys: Vec<String>) -> PyResult<Vec<String>> {
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let dropped = || rowver::change::drop_tool(&rowver::change::Locked::take(&path)?, &keys);
    py.detach(dropped).map_err(|e| failed(rowver::plain(&path, &e)))
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
