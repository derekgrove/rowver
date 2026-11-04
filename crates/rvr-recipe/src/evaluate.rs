//! Walk an expression's tree over the columns of one run of events.

use crate::array::{lacking, not_truth, Array, Constant, Operator};
use crate::functions as f;
use crate::kind::Kind;
use crate::parse::Expression;
use crate::{problem, Result};
use hep_columns::vectors::Momentum;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;

/// Every function an expression can call, with the fewest and most values it takes (`usize::MAX`: any number).
pub const FUNCTIONS: &[(&str, usize, usize)] = &[
    ("count", 1, 1), ("sum", 1, 1), ("any", 1, 1), ("all", 1, 1), ("least", 2, 2), ("greatest", 2, 2),
    ("least_index", 2, 2), ("greatest_index", 2, 2), ("first", 2, 2), ("nth", 3, 3), ("index", 1, 1),
    ("sorted", 1, 2), ("order", 1, 2), ("take", 3, 3), ("listed", 2, 2), ("join", 2, usize::MAX),
    ("abs", 1, 1), ("sign", 1, 1), ("sqrt", 1, 1), ("exp", 1, 1), ("log", 1, 1), ("log10", 1, 1),
    ("cos", 1, 1), ("sin", 1, 1), ("tan", 1, 1), ("acos", 1, 1), ("asin", 1, 1), ("atan", 1, 1),
    ("atan2", 2, 2), ("cosh", 1, 1), ("sinh", 1, 1), ("tanh", 1, 1), ("hypot", 2, 2), ("floor", 1, 1),
    ("ceil", 1, 1), ("round", 1, 1), ("min", 2, 2), ("max", 2, 2), ("clip", 3, 3), ("isfinite", 1, 1),
    ("if", 3, 3), ("bit_and", 2, 2), ("shift_right", 2, 2), ("delta_phi", 2, 2), ("delta_r", 4, 4),
    ("mass", 3, 4), ("pt", 3, 4), ("eta", 3, 4), ("phi", 3, 4), ("rapidity", 3, 4), ("dr_nearest", 4, 5),
    ("dr_second_nearest", 4, 5), ("dr_farthest", 4, 5), ("dr_second_farthest", 4, 5),
    ("nearest_index", 4, 5), ("second_nearest_index", 4, 5), ("farthest_index", 4, 5),
    ("second_farthest_index", 4, 5), ("pair_first", 1, 1), ("pair_second", 1, 1), ("pair_dr", 2, 2),
    ("pair_mass", 3, 4), ("pair_pt", 3, 4), ("pair_eta", 3, 4), ("pair_phi", 3, 4),
    ("pair_rapidity", 3, 4), ("contains", 2, 2), ("branch", 1, 1), ("stored", 2, 2),
];

/// Why `name` cannot be called with `count` values, when it cannot: no function has that name, or
/// it takes another number of values.
pub(crate) fn uncallable(name: &str, count: usize) -> Option<String> {
    let Some(&(_, least, most)) = FUNCTIONS.iter().find(|(known, ..)| *known == name) else {
        let near = crate::meant(name, FUNCTIONS.iter().map(|(known, ..)| *known));
        return Some(format!("There is no function named {name}.{near} Every function, with an example, is in the recipe reference: {}", crate::REFERENCE));
    };
    if (least..=most).contains(&count) {
        return None;
    }
    let takes = match (least, most) {
        (1, 1) => "1 value".to_owned(),
        (n, m) if n == m => format!("{n} values"),
        (n, usize::MAX) => format!("{n} values or more"),
        (n, m) => format!("{n} or {m} values"),
    };
    let which = match (least, most) {
        (3, 4) => " (pt, eta, phi, and optionally a mass)",
        (4, 5) => " (eta and phi of one list, eta and phi of the other, and optionally the value for a member with none to match)",
        _ => "",
    };
    Some(format!("{name} takes {takes}{which}, not {count}"))
}

/// Where an expression's columns come from.
pub trait Columns {
    /// The named column over the events in hand, or `None` when there is no such column.
    fn column(&self, name: &str) -> Result<Option<Array>>;
    /// Whether there is a column of this name, without reading it.
    fn has(&self, name: &str) -> bool;
}

/// Which rows of other columns a `Picked` holds.
enum Rows<'a> {
    Run(usize, usize),
    At(&'a [usize]),
}

/// Some rows of other columns, each column cut once when first asked for; the columns themselves when every row is kept.
pub struct Picked<'a> {
    columns: &'a dyn Columns,
    rows: Rows<'a>,
    whole: bool,
    cut: RefCell<BTreeMap<String, Option<Array>>>,
}

impl<'a> Picked<'a> {
    /// The rows at `positions` (ascending, distinct) of `columns`, which cover `rows`.
    pub fn new(columns: &'a dyn Columns, positions: &'a [usize], rows: usize) -> Self {
        Picked { columns, rows: Rows::At(positions), whole: positions.len() == rows, cut: RefCell::default() }
    }

    /// Rows `from..to` of `columns`, which cover `rows`.
    pub(crate) fn run(columns: &'a dyn Columns, from: usize, to: usize, rows: usize) -> Self {
        Picked { columns, rows: Rows::Run(from, to), whole: from == 0 && to == rows, cut: RefCell::default() }
    }
}

impl Columns for Picked<'_> {
    fn column(&self, name: &str) -> Result<Option<Array>> {
        if self.whole {
            return self.columns.column(name);
        }
        if let Some(held) = self.cut.borrow().get(name) {
            return Ok(held.clone());
        }
        let held = self.columns.column(name)?.map(|array| match self.rows {
            Rows::Run(from, to) => array.rows(from, to),
            Rows::At(positions) => array.picked(positions),
        });
        self.cut.borrow_mut().insert(name.to_owned(), held.clone());
        Ok(held)
    }

    fn has(&self, name: &str) -> bool {
        self.columns.has(name)
    }
}

/// `work` over the `rows` rows of `columns` with `in_runs`, each part its slice; a failure with the rows it lies in.
pub fn over_runs<T>(columns: &dyn Columns, rows: usize, work: &mut dyn FnMut(&dyn Columns, usize, usize) -> Result<T>) -> std::result::Result<Vec<T>, (crate::Error, Range<usize>)> {
    let mut part = 0..rows;
    let done = crate::in_runs(0, rows, &mut |from, to| {
        part = from..to;
        work(&Picked::run(columns, from, to, rows), from, to).map_err(|e| e.later(from))
    });
    done.map_err(|e| (e, part))
}

/// Everything an expression can refer to: the columns, the sample's metadata, and the recipe's defined names.
pub struct Scope<'a> {
    pub columns: &'a dyn Columns,
    /// The sample's metadata fields.
    pub sample: &'a BTreeMap<String, Constant>,
    pub defines: &'a BTreeMap<String, Expression>,
    /// Each defined name's value, or why it has none, once worked out, so that a name used many
    /// times is worked out once; `None` while it is being worked out.
    known: RefCell<BTreeMap<String, Option<Result<Array>>>>,
}

impl<'a> Scope<'a> {
    /// A scope over these columns, this sample's metadata and these defined names.
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

    /// Each defined name worked out so far without fault, with its value.
    pub fn worked_out(&self) -> Vec<(String, Array)> {
        self.known.borrow().iter().filter_map(|(name, value)| Some((name.clone(), value.clone()?.ok()?))).collect()
    }

    /// The same scope, the defined names `known` already worked out over these same events.
    pub fn knowing(self, known: impl IntoIterator<Item = (String, Array)>) -> Self {
        self.known.borrow_mut().extend(known.into_iter().map(|(name, value)| (name, Some(Ok(value)))));
        self
    }

    /// Evaluate a condition to one truth value for each of `events` events.
    pub fn mask(&self, expression: &Expression, events: usize) -> Result<Vec<bool>> {
        Self::condition(self.evaluate(expression)?, events)
    }

    /// A condition's value as one truth value for each of `events` events.
    pub fn condition(value: Array, events: usize) -> Result<Vec<bool>> {
        match value {
            Array::Constant(Constant::Truth(truth)) => Ok(vec![truth; events]),
            Array::Constant(Constant::Missing(key)) => Err(lacking(&key, "a condition")),
            Array::Constant(Constant::Text(text)) => Err(problem(format!("A condition needs a true-or-false value, not the text {text:?}"))),
            Array::Constant(_) => Err(not_truth("A condition", "a number")),
            Array::Event(values) if values.len() == events => Ok(values.truths("A condition")?.iter().map(|truth| *truth != 0).collect()),
            Array::Event(_) => Err(problem("A condition must give one true-or-false value per event")),
            Array::Object { .. } => Err(problem("A condition must give one true-or-false value per event, not a list: reduce the list, as any(…), all(…) or count(…) > 0")),
        }
    }

    /// Fetch a column that must exist.
    fn column(&self, name: &str) -> Result<Array> {
        self.columns.column(name)?.ok_or_else(|| crate::missing(name))
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
            Negate(inner) => f::negated(&self.evaluate(inner)?)?,
            Not(inner) => f::denied(&self.evaluate(inner)?)?,
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
                Some(value) => value.clone(),
                None => Err(problem(format!(
                    "{name} refers to itself, directly or through other defined names"
                ))),
            };
        }
        let value = match self.columns.has(name) {
            true => Err(crate::clash(name)),
            false => {
                self.known.borrow_mut().insert(name.to_owned(), None);
                self.evaluate(defined)
            }
        };
        self.known.borrow_mut().insert(name.to_owned(), Some(value.clone()));
        value
    }

    /// Call a function: its name and number of values are checked before any value is worked out.
    fn call(&self, name: &str, arguments: &[Expression]) -> Result<Array> {
        if self.defines.contains_key(name) {
            return Err(problem(format!("{name} is a defined name, not a function: write it without brackets, as {name}")));
        }
        if let Some(why) = uncallable(name, arguments.len()) {
            return Err(problem(why));
        }
        // A fixed condition works out only the value it chooses.
        if name == "if" {
            let condition = self.evaluate(&arguments[0])?;
            return match condition {
                Array::Constant(crate::Constant::Truth(truth)) => self.evaluate(&arguments[if truth { 1 } else { 2 }]),
                condition => f::choose(&condition, &self.evaluate(&arguments[1])?, &self.evaluate(&arguments[2])?),
            };
        }
        let arrays: Vec<Array> = arguments.iter().map(|argument| self.evaluate(argument)).collect::<Result<_>>()?;
        self.apply(name, &arrays)
    }

    /// Work a function out over its values, once each is worked out: a frame of its own, off the
    /// path an expression's depth is walked along.
    #[inline(never)]
    fn apply(&self, name: &str, arrays: &[Array]) -> Result<Array> {
        let one = |function: fn(&Array) -> Result<Array>| function(&arrays[0]);
        let float = |function: fn(f64) -> f64| f::float_map(&arrays[0], function);
        // What `of` measures of each event's list added together, or of each pair of it.
        let kinematic = |of: fn(Momentum) -> f64, pairs: bool| match pairs {
            true => f::pair_summed(arrays, name, of),
            false => f::summed(arrays, name, of),
        };
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
            "log10" => float(libm::log10)?,
            "tan" => float(libm::tan)?,
            "acos" => float(libm::acos)?,
            "asin" => float(libm::asin)?,
            "atan" => float(libm::atan)?,
            "tanh" => float(libm::tanh)?,
            // floor and ceil keep whole numbers and true-or-false values; round takes no
            // true-or-false values.
            "floor" => f::same_map(&arrays[0], "floor", |v| v, libm::floor)?,
            "ceil" => f::same_map(&arrays[0], "ceil", |v| v, libm::ceil)?,
            "round" => one(f::rounded)?,
            "atan2" => f::float_pair(&arrays[0], &arrays[1], libm::atan2)?,
            "min" => f::pairwise(&arrays[0], &arrays[1], false)?,
            "max" => f::pairwise(&arrays[0], &arrays[1], true)?,
            "hypot" => f::float_pair(&arrays[0], &arrays[1], libm::hypot)?,
            "least" => f::extreme(&arrays[0], &arrays[1], false)?,
            "greatest" => f::extreme(&arrays[0], &arrays[1], true)?,
            "first" => f::nth(&arrays[0], 0, &arrays[1], "first")?,
            "nth" => {
                let position = match &arrays[1] {
                    Array::Constant(crate::Constant::Whole(position)) => position,
                    Array::Constant(crate::Constant::Missing(key)) => return Err(lacking(key, "the position in nth")),
                    _ => return Err(problem("nth needs a whole number for the position, counting from 0, such as nth(Jet_pt, 1, -1)")),
                };
                let position = usize::try_from(*position).map_err(|_| problem("nth counts positions from 0, so its position is 0 or more"))?;
                f::nth(&arrays[0], position, &arrays[2], "nth")?
            }
            "listed" => f::listed(&arrays[0], &arrays[1])?,
            "dr_nearest" | "dr_second_nearest" | "dr_farthest" | "dr_second_farthest" | "nearest_index"
            | "second_nearest_index" | "farthest_index" | "second_farthest_index" => {
                let rank = name.contains("second") as usize;
                let farthest = name.contains("farthest");
                let position = name.ends_with("_index");
                f::ranked(arrays, rank, farthest, position, name)?
            }
            "delta_phi" => f::formula(&[&arrays[0], &arrays[1]], "delta_phi", |v| hep_columns::angles::delta_phi(v[0], v[1]))?,
            "delta_r" => {
                let parts = [&arrays[0], &arrays[1], &arrays[2], &arrays[3]];
                f::formula(&parts, "delta_r", |v| hep_columns::angles::delta_r(v[0], v[1], v[2], v[3]))?
            }
            "clip" => {
                let floor = f::pairwise(&arrays[0], &arrays[1], true)?;
                f::pairwise(&floor, &arrays[2], false)?
            }
            "bit_and" => f::bits(&arrays[0], &arrays[1], "bit_and", |v, m, _| hep_columns::bits::bit_and(v, m))?,
            "shift_right" => {
                if matches!(arrays[1], Array::Constant(crate::Constant::Whole(places)) if places < 0) {
                    return Err(problem("shift_right cannot shift by a negative number of bits"));
                }
                f::bits(&arrays[0], &arrays[1], "shift_right", |v, n, unsigned| hep_columns::bits::shift_right(v, n.min(64) as u32, unsigned))?
            }
            "least_index" => f::extreme_at(&arrays[0], &arrays[1], false)?,
            "greatest_index" => f::extreme_at(&arrays[0], &arrays[1], true)?,
            "order" | "sorted" => {
                let descending = match arrays.get(1) {
                    None => false,
                    Some(direction) => match text(direction, "the direction")?.as_str() {
                        "descending" => true,
                        "ascending" => false,
                        other => return Err(problem(format!("{name} takes 'ascending' or 'descending' as its direction, not {other:?}"))),
                    },
                };
                f::order(&arrays[0], descending, name == "order")?
            }
            "take" => f::take(&arrays[0], &arrays[1], &arrays[2])?,
            "join" => f::join(arrays)?,
            "pair_first" => one(|array| f::pair_member(array, false))?,
            "pair_second" => one(|array| f::pair_member(array, true))?,
            "pair_dr" => f::pair_dr(&arrays[0], &arrays[1])?,
            "pair_mass" => kinematic(Momentum::mass, true)?,
            "pair_pt" => kinematic(Momentum::pt, true)?,
            "pair_eta" => kinematic(Momentum::eta, true)?,
            "pair_phi" => kinematic(Momentum::phi, true)?,
            "pair_rapidity" => kinematic(Momentum::rapidity, true)?,
            "contains" => {
                let found = match (&arrays[0], &arrays[1]) {
                    (Array::Constant(crate::Constant::Text(whole)), Array::Constant(crate::Constant::Text(part))) => whole.contains(part.as_str()),
                    (Array::Constant(crate::Constant::Missing(_)), _) | (_, Array::Constant(crate::Constant::Missing(_))) => false,
                    _ => return Err(problem("contains needs two pieces of text, as contains(sample.kind, 'sig')")),
                };
                Array::Constant(crate::Constant::Truth(found))
            }
            "mass" => kinematic(Momentum::mass, false)?,
            "pt" => kinematic(Momentum::pt, false)?,
            "eta" => kinematic(Momentum::eta, false)?,
            "phi" => kinematic(Momentum::phi, false)?,
            "rapidity" => kinematic(Momentum::rapidity, false)?,
            "branch" => self.column(&text(&arrays[0], "a branch name")?)?,
            "stored" => {
                let kind = Kind::named(&text(&arrays[1], "a number kind")?)?;
                arrays[0].convert(kind)?
            }
            _ => unreachable!("uncallable refuses every name FUNCTIONS does not hold"),
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
