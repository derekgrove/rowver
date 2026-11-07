//! The recipe of `rvr press`: TOML whose expressions are evaluated over whole columns of events.
//!
//! An expression is read into a tree once (`parse`) and walked for each run of events
//! (`Scope::evaluate`); nothing is compiled. Kinds follow NumPy's type promotion, with the
//! differences this crate's README states; the README describes the whole language, to the bit.

mod array;
mod check;
mod document;
mod evaluate;
mod functions;
mod kind;
mod nearest;
mod parse;

pub use array::{Array, Constant, Operator, Store, Values, QUIET_NAN_F4, QUIET_NAN_F8};
pub use document::{branch_names, fields_of, names, plain, reads, uses, Cut, Recipe, Total, ADDED, CHUNK_BOUNDS, CHUNK_ROWS, RESERVED, TABLES};
pub use evaluate::{over_runs, per_event, Columns, Picked, Scope, FUNCTIONS};
pub use kind::Kind;
pub use nearest::{meant, nearest, slips, unknown};

/// Where the recipe reference, which lists every function with an example, is read.
pub const REFERENCE: &str = "https://gitlab.cern.ch/dgrove/rowver/-/blob/main/crates/rvr-recipe/README.md";
pub use parse::{parse, Expression, DEEPEST};

/// Why an expression could not be read or evaluated: its words, its event (a data error's), and what kind of problem it is.
#[derive(Clone, Debug, thiserror::Error, PartialEq)]
#[error("{0}")]
pub struct Error(pub String, pub Option<usize>, Cause);

/// What kind of problem an error is, beyond its words.
#[derive(Clone, Debug, PartialEq)]
enum Cause {
    /// Any other.
    Other,
    /// The events worked on at once make too many pairs together.
    Crowded,
    /// An expression names a column that is not there: its name.
    Missing(String),
    /// A defined name is also a column's.
    Clash,
    /// A recipe leaves out these of its five tables (and may have other problems).
    Lacking(Vec<&'static str>),
}

/// What reading or evaluating an expression gives.
pub type Result<T> = std::result::Result<T, Error>;

/// Describe a problem with an expression or its inputs.
pub fn problem(message: impl Into<String>) -> Error {
    Error(message.into(), None, Cause::Other)
}

/// Describe a value of event `event` that the expression cannot take.
pub fn data(event: usize, message: impl Into<String>) -> Error {
    Error(message.into(), Some(event), Cause::Other)
}

/// Say that the events worked on at once make more pairs together than `RUN_PAIRS`.
pub(crate) fn crowded(message: impl Into<String>) -> Error {
    Error(message.into(), None, Cause::Crowded)
}

/// Say that an expression names `name`, which is no column.
pub(crate) fn missing(name: &str) -> Error {
    Error(format!("There is no column named {name}"), None, Cause::Missing(name.to_owned()))
}

/// Say that the defined name `name` is also a column's.
pub(crate) fn clash(name: &str) -> Error {
    Error(format!("{name} is a name [define] gives an expression, and also the name of a branch of this file: give the defined name another, so that each name means one thing"), None, Cause::Clash)
}

/// Work `work` out over rows `first..last` in order, halving any run too `crowded` to work on at once.
pub fn in_runs<T>(first: usize, last: usize, work: &mut dyn FnMut(usize, usize) -> Result<T>) -> Result<Vec<T>> {
    let (mut done, mut pending) = (Vec::new(), vec![(first, last)]);
    while let Some((from, to)) = pending.pop() {
        match work(from, to) {
            Err(e) if e.crowded() && to - from > 1 => {
                let middle = from + (to - from) / 2;
                pending.extend([(middle, to), (from, middle)]);
            }
            other => done.push(other?),
        }
    }
    Ok(done)
}

impl Error {
    /// Whether it says only that the events worked on at once make too many pairs together.
    pub fn crowded(&self) -> bool {
        self.2 == Cause::Crowded
    }

    /// The column it names that is not there, when that is what it says.
    pub fn missing(&self) -> Option<&str> {
        match &self.2 {
            Cause::Missing(name) => Some(name),
            _ => None,
        }
    }

    /// Whether it says that a defined name is also a column's.
    pub fn clash(&self) -> bool {
        self.2 == Cause::Clash
    }

    /// The tables of the five a refused recipe leaves out.
    pub fn lacking(&self) -> &[&'static str] {
        match &self.2 {
            Cause::Lacking(tables) => tables,
            _ => &[],
        }
    }

    /// The same error, of event `event` (or of none).
    pub fn of_event(self, event: Option<usize>) -> Error {
        Error(self.0, event, self.2)
    }

    /// The same error, its event counted `by` later (for columns that begin `by` events in).
    pub fn later(self, by: usize) -> Error {
        Error(self.0, self.1.map(|at| at + by), self.2)
    }

    /// Its words on one line, its problems (a recipe's, one a line) joined by "; ".
    pub fn line(&self) -> String {
        self.0.replace('\n', "; ")
    }

    /// The same error, its words after `place` ("[columns] ht: …").
    pub fn at(self, place: &str) -> Error {
        Error(format!("{place}: {}", self.0), self.1, self.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RECIPE functions-exactly
    #[test]
    fn a_crowded_run_is_worked_on_in_halves_in_order_and_a_mistake_ends_it() {
        // Runs of more than 3 rows are too crowded; every other run gives its rows.
        let mut tried = Vec::new();
        let mut work = |from: usize, to: usize| {
            tried.push((from, to));
            match to - from > 3 {
                true => Err(crowded("too many")),
                false => Ok((from..to).collect::<Vec<_>>()),
            }
        };
        let parts = in_runs(0, 10, &mut work).unwrap();
        assert_eq!(parts.concat(), (0..10).collect::<Vec<_>>());
        assert_eq!(tried, [(0, 10), (0, 5), (0, 2), (2, 5), (5, 10), (5, 7), (7, 10)]);
        // One row that is still crowded, or any other mistake, is said as it is.
        assert!(in_runs(0, 1, &mut |_, _| Err::<(), _>(crowded("too many"))).unwrap_err().crowded());
        let mut mistaken = |from: usize, to: usize| match (from, to - from > 4) {
            (_, true) => Err(crowded("too many")),
            (4, _) => Err(data(1, "bad")),
            _ => Ok(()),
        };
        let said = in_runs(0, 8, &mut mistaken).unwrap_err();
        assert_eq!((said.0.as_str(), said.1, said.crowded()), ("bad", Some(1), false));
        assert!(!problem("more pairs than one run of events may hold at once").crowded());
    }

    /// No columns at all: every part of a run works or fails alone.
    struct Nothing;

    impl Columns for Nothing {
        fn column(&self, _: &str) -> Result<Option<Array>> {
            Ok(None)
        }

        fn has(&self, _: &str) -> bool {
            false
        }
    }

    // RECIPE functions-exactly
    #[test]
    fn a_failure_over_runs_names_the_rows_of_the_part_it_lies_in() {
        // Too crowded past 4 rows; the part from row 6 fails with no event of its own.
        let mut work = |_: &dyn Columns, from: usize, to: usize| match (to - from > 4, from) {
            (true, _) => Err(crowded("too many")),
            (_, 6) => Err(problem("unreadable")),
            _ => Ok(()),
        };
        let (error, part) = over_runs(&Nothing, 12, &mut work).unwrap_err();
        assert_eq!((error.0.as_str(), error.1, part), ("unreadable", None, 6..9));
    }
}
