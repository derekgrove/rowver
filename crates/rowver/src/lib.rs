//! RowVer in Rust: read, validate, and write `.rvr` files as
//! [the format](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md) specifies them.

pub mod choice;
mod codec;
mod json;
mod numeric;
pub mod parallel;
pub mod rvr;
pub mod sign;
pub mod study;
mod table;
pub mod write;

/// The text of this crate's own code, for a tool that shows which code read a file.
pub mod source {
    /// `rvr.rs`: reading and validating a file.
    pub const READER: &str = include_str!("rvr.rs");
    /// `table.rs`: the columns of a file held in memory.
    pub const TABLE: &str = include_str!("table.rs");
}

/// The `.rvr` version this crate reads and writes; it reads no other.
pub const VERSION: u64 = 15;

pub use rvr::CheckpointSummary;
pub use table::{Column, ColumnInfo, EventIdentity, EventView, NumericBuffer, Table};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid JSON header: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

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
