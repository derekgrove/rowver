//! The recipe of `rvr press`: TOML whose expressions are evaluated over whole columns of events.
//!
//! An expression is read into a tree once (`parse`) and walked for each run of events
//! (`Scope::evaluate`); nothing is compiled. Values keep the number kinds and the conversions
//! that NumPy 2.5.1 and awkward 2.12.0 give the same operations, so a selection keeps the same
//! events as the Python recipe it replaces. This crate's README describes the whole language.

mod array;
mod document;
mod evaluate;
mod functions;
mod kind;
mod parse;

pub use array::{Array, Constant, Operator, Store, Values};
pub use document::{names, plain, reads, uses, Cut, Recipe, RESERVED};
pub use evaluate::{per_event, Columns, Scope, FUNCTIONS};
pub use kind::Kind;
pub use parse::{parse, Expression};

/// Why an expression could not be read or evaluated, in words for the person who wrote it.
#[derive(Debug, thiserror::Error, PartialEq)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

/// Describe a problem with an expression or its inputs.
pub(crate) fn problem(message: impl Into<String>) -> Error {
    Error(message.into())
}
