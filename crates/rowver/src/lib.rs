//! RowVer in Rust: read, validate, and write `.rvr` files as
//! [the format](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md) specifies them.

#![warn(missing_docs)]

pub mod choice;
mod chunks;
mod codec;
mod decimal;
mod json;
mod numeric;
pub mod parallel;
pub mod rvr;
pub mod sign;
pub mod study;
pub mod write;

use std::path::Path;

/// The `.rvr` version this crate reads and writes; it reads no other.
pub const VERSION: u64 = 15;

/// What went wrong reading, checking or writing a file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file, or part of one, is not as the format says: why, and what to do.
    #[error("{0}")]
    Invalid(String),
    /// Reading or writing failed; `plain` says it in words.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Reading or writing this file failed, said in words.
    #[error("{}", plain_io(.0, .1))]
    File(std::path::PathBuf, std::io::Error),
    /// A header's JSON does not parse.
    #[error("the header is not JSON as docs/FORMAT.md §1 reads it ({0}): the file is damaged, or the program that wrote it does not follow the format; copy it again, or press its study again with rvr")]
    Json(#[from] serde_json::Error),
    /// A header's JSON parses, but breaks a rule of docs/FORMAT.md §1.
    #[error("the header breaks docs/FORMAT.md §1: {fact} (line {line}, column {column}): the program that wrote it does not follow the format; press its study again with rvr")]
    Breaks {
        /// What breaks the rule, and where in the header.
        fact: String,
        /// The top-level key it lies under.
        under: Option<String>,
        /// The line of the header's text it is on.
        line: usize,
        /// The column of that line.
        column: usize,
    },
}

/// A value, or the `Error` that stopped it.
pub type Result<T> = std::result::Result<T, Error>;

/// What went wrong with the file at `path` (or the file the failure names), in plain words.
pub fn plain(path: impl AsRef<Path>, error: &Error) -> String {
    match error {
        Error::Io(e) => plain_io(path, e),
        e @ Error::File(..) => e.to_string(),
        e => named(path.as_ref(), &e.to_string()),
    }
}

/// What went wrong with the file at `path` (or the file the failure names), without what to do.
pub fn plain_what(path: impl AsRef<Path>, error: &Error) -> String {
    match error {
        Error::Io(e) => told(path.as_ref(), e).0,
        Error::File(at, e) => told(at, e).0,
        e => named(path.as_ref(), &e.to_string()),
    }
}

/// `text` with the file's name before it, unless it begins with that name already.
fn named(path: &Path, text: &str) -> String {
    let shown = path.display().to_string();
    match text.starts_with(&shown) {
        true => text.to_owned(),
        false => format!("{shown}: {text}"),
    }
}

/// `error` said of the file at `path`: a failed read or write tagged with it, a refusal naming it.
pub(crate) fn about(path: &Path, error: Error) -> Error {
    match error {
        Error::Io(e) => match tagged_with(&e) {
            Some(_) => Error::Io(e),
            None => Error::File(path.to_path_buf(), e),
        },
        Error::Invalid(why) => Error::Invalid(named(path, &why)),
        e @ (Error::Json(_) | Error::Breaks { .. }) => Error::Invalid(named(path, &e.to_string())),
        e => e,
    }
}

/// A failed read or write of the file at `path`, carried through readers and writers.
#[derive(Debug)]
struct Tagged {
    path: std::path::PathBuf,
    error: std::io::Error,
}

impl std::fmt::Display for Tagged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&plain_io(&self.path, &self.error))
    }
}

impl std::error::Error for Tagged {}

/// `error`, of the same kind, naming the file at `path` it is about.
pub(crate) fn tagged(path: &Path, error: std::io::Error) -> std::io::Error {
    match tagged_with(&error) {
        Some(_) => error,
        None => std::io::Error::new(error.kind(), Tagged { path: path.to_path_buf(), error }),
    }
}

/// The file a failed read or write names, and its own failure, when `tagged` named one.
fn tagged_with(error: &std::io::Error) -> Option<(&Path, &std::io::Error)> {
    let tag = error.get_ref()?.downcast_ref::<Tagged>()?;
    Some((&tag.path, &tag.error))
}

/// A failed read or write of the file at `path` (or the file it names), in plain words with what to do.
pub fn plain_io(path: impl AsRef<Path>, error: &std::io::Error) -> String {
    let (what, fix) = told(path.as_ref(), error);
    match fix {
        "" => what,
        fix => format!("{what}: {fix}"),
    }
}

/// A failed read or write of a file a line names already, said of "it".
pub fn plain_it(error: &std::io::Error) -> String {
    plain_io("it", error)
}

/// The operating system's description of a failed read or write, without its error number.
pub fn described(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rsplit_once(" (os error ") {
        Some((said, number)) if number.ends_with(')') => said.to_owned(),
        _ => text,
    }
}

/// A failed read or write of the file at `path` (or the file it names): what went wrong, and what to do.
fn told(path: &Path, error: &std::io::Error) -> (String, &'static str) {
    use std::io::ErrorKind::*;
    if let Some((path, error)) = tagged_with(error) {
        return told(path, error);
    }
    let shown = path.display();
    match error.kind() {
        NotFound => (format!("{shown} is not there"), ""),
        IsADirectory => (format!("{shown} is a folder, not a file"), "name a file in it"),
        NotADirectory => (format!("{shown}: part of its path is a file, not a folder"), ""),
        UnexpectedEof => (format!("{shown} changed while it was read (emptied, or written again)"), "open it again"),
        PermissionDenied => (format!("{shown} is not yours to read or write"), "ls -l says whose it is"),
        AlreadyExists => (format!("{shown} is there already"), "move it away, or name another"),
        StorageFull | QuotaExceeded => (format!("{shown} could not be written: its disk is full"), "free some space"),
        ReadOnlyFilesystem => (format!("{shown} could not be written: its disk is read-only"), ""),
        _ => (format!("{shown} could not be read or written ({})", described(error)), "check its disk, and try again"),
    }
}

impl From<&'static str> for Error {
    /// Describe a failure that has only a message.
    fn from(message: &'static str) -> Self {
        Error::Invalid(message.into())
    }
}

/// Describe a failed artifact validation.
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// The README's examples, compiled as tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
