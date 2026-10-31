//! Walk an expression's tree over the columns of one run of events.

use crate::array::{lacking, Array, Constant, Operator};
use crate::functions as f;
use crate::kind::Kind;
use crate::parse::Expression;
use crate::{problem, Result};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Every function an expression can call, as the README lists them.
pub const FUNCTIONS: &[&str] = &[
    "count", "sum", "any", "all", "least", "greatest", "least_index", "greatest_index", "first",
    "nth", "index", "sorted", "order", "take", "listed", "abs", "sign", "sqrt", "exp", "log", "cos",
    "sin", "cosh", "sinh", "hypot", "min", "max", "clip", "isfinite", "if", "bit_and",
    "shift_right", "delta_phi", "delta_r", "mass", "dr_nearest", "dr_second_nearest",
    "dr_farthest", "dr_second_farthest", "nearest_index", "second_nearest_index",
    "farthest_index", "second_farthest_index", "pair_first", "pair_second", "pair_dr",
    "pair_mass", "contains", "branch", "stored",
];

/// Where an expression's columns come from.
pub trait Columns {
    /// The named column over the events in hand, or `None` when there is no such column.
    fn column(&self, name: &str) -> Result<Option<Array>>;
    /// Whether there is a column of this name, without reading it.
    fn has(&self, name: &str) -> bool;
}

/// Everything an expression can refer to: the columns, the sample's metadata, and the names a
/// recipe defines. A defined name that is also a column's is refused when it is used, so that a
/// name means one thing.
pub struct Scope<'a> {
    pub columns: &'a dyn Columns,
    /// The sample's metadata fields.
    pub sample: &'a BTreeMap<String, Constant>,
    pub defines: &'a BTreeMap<String, Expression>,
    /// Each defined name's value, or why it has none, once worked out, so that a name used many
    /// times is worked out once; `None` while it is being worked out.
    known: RefCell<BTreeMap<String, Option<std::result::Result<Array, String>>>>,
}

/// How many values, in words: `1 value`, `2 values`.
fn how_many(count: usize) -> String {
    format!("{count} value{}", if count == 1 { "" } else { "s" })
}

/// What a name that is no column is refused with.
fn unknown(name: &str) -> crate::Error {
    problem(format!("There is no column named {name}"))
}

impl<'a> Scope<'a> {
    /// A scope over these columns, for a sample with this metadata and a recipe with these
    /// defined names.
    pub fn new(
        columns: &'a dyn Columns,
        sample: &'a BTreeMap<String, Constant>,
        defines: &'a BTreeMap<String, Expression>,
    ) -> Self {
        Scope {
            columns,
            sample,
            defines,
            known: RefCell::default(),
        }
    }

    /// Evaluate a condition to one truth value for each of `events` events.
    pub fn mask(&self, expression: &Expression, events: usize) -> Result<Vec<bool>> {
        match self.evaluate(expression)? {
            Array::Constant(Constant::Truth(truth)) => Ok(vec![truth; events]),
            Array::Constant(Constant::Missing(key)) => Err(lacking(&key, "a condition")),
            Array::Event(values) if values.len() == events => Ok(values
                .truths("A condition")?
                .iter()
                .map(|truth| *truth != 0)
                .collect()),
            _ => Err(problem(
                "A condition must give one true-or-false value per event",
            )),
        }
    }

    /// Fetch a column that must exist.
    fn column(&self, name: &str) -> Result<Array> {
        self.columns.column(name)?.ok_or_else(|| unknown(name))
    }

    /// Evaluate an expression to a constant, one value per event, or a list per event.
    pub fn evaluate(&self, expression: &Expression) -> Result<Array> {
        use Expression::*;
        Ok(match expression {
            Constant(constant) => Array::Constant(constant.clone()),
            Name(name) => return self.name(name),
            Sample(field) => Array::Constant(
                self.sample
                    .get(field)
                    .cloned()
                    .unwrap_or_else(|| crate::Constant::Missing(field.clone())),
            ),
            Negate(inner) => match self.evaluate(inner)? {
                Array::Constant(crate::Constant::Float(value)) => {
                    Array::Constant(crate::Constant::Float(-value))
                }
                Array::Constant(crate::Constant::Whole(value)) => {
                    Array::Constant(crate::Constant::Whole(-value))
                }
                other => Array::Constant(crate::Constant::Whole(-1))
                    .combine(Operator::Multiply, &other)?,
            },
            Not(inner) => self.evaluate(inner)?.combine(
                Operator::Equal,
                &Array::Constant(crate::Constant::Truth(false)),
            )?,
            Binary(operator, left, right) => {
                let left = self.evaluate(left)?;
                let truth = |wanted: bool| matches!(&left, Array::Constant(crate::Constant::Truth(found)) if *found == wanted);
                let decided = (*operator == Operator::And && truth(false))
                    || (*operator == Operator::Or && truth(true));
                if decided {
                    left
                } else {
                    left.combine(*operator, &self.evaluate(right)?)?
                }
            }
            Pick(values, mask) => f::pick(&self.evaluate(values)?, &self.evaluate(mask)?)?,
            In(left, list) => {
                let left = self.evaluate(left)?;
                let mut found = Array::Constant(crate::Constant::Truth(false));
                for member in list {
                    let equal = left.combine(Operator::Equal, &self.evaluate(member)?)?;
                    found = found.combine(Operator::Or, &equal)?;
                }
                found
            }
            Call(name, arguments) => return self.call(name, arguments),
        })
    }

    /// Resolve a bare name: a defined name, worked out once, or else a column.
    fn name(&self, name: &str) -> Result<Array> {
        let Some(defined) = self.defines.get(name) else {
            return self.column(name);
        };
        if let Some(known) = self.known.borrow().get(name) {
            return match known {
                Some(value) => value.clone().map_err(problem),
                None => Err(problem(format!(
                    "{name} refers to itself, directly or through other defined names"
                ))),
            };
        }
        let value = match self.columns.has(name) {
            true => Err(problem(format!("{name} is a name [define] gives an expression, and also the name of a branch of this file: give the defined name another, so that each name means one thing"))),
            false => {
                self.known.borrow_mut().insert(name.to_owned(), None);
                self.evaluate(defined)
            }
        };
        let kept = match &value {
            Ok(array) => Ok(array.clone()),
            Err(error) => Err(error.0.clone()),
        };
        self.known.borrow_mut().insert(name.to_owned(), Some(kept));
        value
    }

    /// Call a function.
    fn call(&self, name: &str, arguments: &[Expression]) -> Result<Array> {
        if name == "if" && arguments.len() == 3 {
            if let Array::Constant(crate::Constant::Truth(truth)) = self.evaluate(&arguments[0])? {
                return self.evaluate(&arguments[if truth { 1 } else { 2 }]);
            }
        }
        if self.defines.contains_key(name) {
            return Err(problem(format!(
                "{name} is a defined name, not a function: write it without brackets, as {name}"
            )));
        }
        let arrays: Vec<Array> = arguments
            .iter()
            .map(|argument| self.evaluate(argument))
            .collect::<Result<_>>()?;
        let arity = |count: usize| {
            if arrays.len() == count {
                Ok(())
            } else {
                Err(problem(format!(
                    "{name} takes {}, not {}",
                    how_many(count),
                    arrays.len()
                )))
            }
        };
        let one =
            |function: fn(&Array) -> Result<Array>| arity(1).and_then(|_| function(&arrays[0]));
        let float =
            |function: fn(f64) -> f64| arity(1).and_then(|_| f::float_map(&arrays[0], function));
        let text = |array: &Array, what: &str| match array {
            Array::Constant(crate::Constant::Text(text)) => Ok(text.clone()),
            Array::Constant(crate::Constant::Missing(key)) => {
                Err(lacking(key, &format!("{what} in {name}")))
            }
            _ => Err(problem(format!("{name} needs {what} as text"))),
        };
        Ok(match name {
            "count" => one(f::count)?,
            "sum" => one(f::sum)?,
            "any" => one(|array| f::any_all(array, false))?,
            "all" => one(|array| f::any_all(array, true))?,
            "index" => one(f::index)?,
            "isfinite" => one(f::finite)?,
            "abs" => one(f::magnitude)?,
            "sign" => one(f::sign)?,
            "sqrt" => float(f64::sqrt)?,
            "cos" => float(libm::cos)?,
            "sin" => float(libm::sin)?,
            "cosh" => float(libm::cosh)?,
            "sinh" => float(libm::sinh)?,
            "exp" => float(libm::exp)?,
            "log" => float(libm::log)?,
            "min" => {
                arity(2).and_then(|_| f::pairwise(&arrays[0], &arrays[1], f::Extreme::Least))?
            }
            "max" => {
                arity(2).and_then(|_| f::pairwise(&arrays[0], &arrays[1], f::Extreme::Greatest))?
            }
            "hypot" => arity(2).and_then(|_| f::hypot(&arrays[0], &arrays[1]))?,
            "least" => arity(2).and_then(|_| f::extreme(&arrays[0], &arrays[1], false))?,
            "greatest" => arity(2).and_then(|_| f::extreme(&arrays[0], &arrays[1], true))?,
            "first" => arity(2).and_then(|_| f::nth(&arrays[0], 0, &arrays[1]))?,
            "nth" => {
                arity(3)?;
                let Array::Constant(crate::Constant::Whole(position)) = &arrays[1] else {
                    return Err(problem(
                        "nth needs a whole number for the position, counting from 0",
                    ));
                };
                let position = usize::try_from(*position)
                    .map_err(|_| problem("nth counts positions from 0"))?;
                f::nth(&arrays[0], position, &arrays[2])?
            }
            "listed" => arity(2).and_then(|_| f::listed(&arrays[0], &arrays[1]))?,
            "if" => arity(3).and_then(|_| f::choose(&arrays[0], &arrays[1], &arrays[2]))?,
            "dr_nearest"
            | "dr_second_nearest"
            | "dr_farthest"
            | "dr_second_farthest"
            | "nearest_index"
            | "second_nearest_index"
            | "farthest_index"
            | "second_farthest_index" => {
                if arrays.len() != 4 && arrays.len() != 5 {
                    return Err(problem(format!(
                        "{name} takes 4 or 5 values, not {}",
                        arrays.len()
                    )));
                }
                let rank = name.contains("second") as usize;
                let farthest = name.contains("farthest");
                let position = name.ends_with("_index");
                f::ranked(&arrays, rank, farthest, position, name)?
            }
            "delta_phi" => arity(2).and_then(|_| {
                f::formula(&[&arrays[0], &arrays[1]], "delta_phi", |v| {
                    hep_columns::angles::delta_phi(v[0], v[1])
                })
            })?,
            "delta_r" => arity(4).and_then(|_| {
                let parts = [&arrays[0], &arrays[1], &arrays[2], &arrays[3]];
                f::formula(&parts, "delta_r", |v| {
                    hep_columns::angles::delta_r(v[0], v[1], v[2], v[3])
                })
            })?,
            "clip" => {
                arity(3)?;
                let floor = f::pairwise(&arrays[0], &arrays[1], f::Extreme::Greatest)?;
                f::pairwise(&floor, &arrays[2], f::Extreme::Least)?
            }
            "bit_and" => arity(2).and_then(|_| {
                f::bits(&arrays[0], &arrays[1], "bit_and", |v, m, _| {
                    hep_columns::bits::bit_and(v, m)
                })
            })?,
            "shift_right" => {
                arity(2)?;
                if matches!(arrays[1], Array::Constant(crate::Constant::Whole(places)) if places < 0)
                {
                    return Err(problem("shift_right cannot shift by a negative number"));
                }
                f::bits(&arrays[0], &arrays[1], "shift_right", |v, n, unsigned| {
                    hep_columns::bits::shift_right(v, n.min(64) as u32, unsigned)
                })?
            }
            "least_index" => arity(2).and_then(|_| f::extreme_at(&arrays[0], &arrays[1], false))?,
            "greatest_index" => {
                arity(2).and_then(|_| f::extreme_at(&arrays[0], &arrays[1], true))?
            }
            "order" | "sorted" => {
                let descending = match arrays.len() {
                    1 => false,
                    2 => match text(&arrays[1], "the direction")?.as_str() {
                        "descending" => true,
                        "ascending" => false,
                        _ => {
                            return Err(problem(format!(
                                "{name} takes 'ascending' or 'descending' as its direction"
                            )))
                        }
                    },
                    _ => return Err(problem(format!("{name} takes 1 or 2 values"))),
                };
                f::order(&arrays[0], descending, name == "order")?
            }
            "take" => arity(3).and_then(|_| f::take(&arrays[0], &arrays[1], &arrays[2]))?,
            "pair_first" => one(|array| f::pair_member(array, false))?,
            "pair_second" => one(|array| f::pair_member(array, true))?,
            "pair_dr" => arity(2).and_then(|_| f::pair_dr(&arrays[0], &arrays[1]))?,
            "pair_mass" => {
                if arrays.len() != 3 && arrays.len() != 4 {
                    return Err(problem(
                        "pair_mass takes pt, eta, phi, and optionally a mass",
                    ));
                }
                f::pair_mass(&arrays)?
            }
            "contains" => {
                arity(2)?;
                let found = match (&arrays[0], &arrays[1]) {
                    (
                        Array::Constant(crate::Constant::Text(whole)),
                        Array::Constant(crate::Constant::Text(part)),
                    ) => whole.contains(part.as_str()),
                    (Array::Constant(crate::Constant::Missing(_)), _)
                    | (_, Array::Constant(crate::Constant::Missing(_))) => false,
                    _ => return Err(problem("contains needs two pieces of text")),
                };
                Array::Constant(crate::Constant::Truth(found))
            }
            "mass" => {
                if arrays.len() != 3 && arrays.len() != 4 {
                    return Err(problem("mass takes pt, eta, phi, and optionally a mass"));
                }
                f::mass(&arrays)?
            }
            "branch" => arity(1).and_then(|_| self.column(&text(&arrays[0], "a branch name")?))?,
            "stored" => {
                arity(2)?;
                let kind = Kind::named(&text(&arrays[1], "a number kind")?)?;
                match &arrays[0] {
                    Array::Constant(_) => {
                        return Err(problem("stored needs a column, not a fixed value"))
                    }
                    Array::Event(values) => Array::Event(values.cast(kind)),
                    Array::Object { offsets, values } => Array::Object {
                        offsets: Arc::clone(offsets),
                        values: values.cast(kind),
                    },
                }
            }
            _ => {
                return Err(problem(format!(
                    "There is no function named {name}"
                )))
            }
        })
    }
}

/// Give a constant result one value per event.
pub fn per_event(array: Array, events: usize) -> Result<Array> {
    Ok(match array {
        Array::Constant(constant) => Array::Event(constant.alone(events)?),
        other => other,
    })
}
