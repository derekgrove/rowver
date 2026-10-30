//! Walk an expression's tree over the columns of one run of events.

use crate::array::{Array, Constant, Operator};
use crate::functions as f;
use crate::kind::Kind;
use crate::parse::Expression;
use crate::{problem, Result};
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
    /// Whether any column's name begins with this, as a collection's attributes do.
    fn begins(&self, start: &str) -> bool;
}

/// A named piece of logic with the names it takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Helper {
    pub takes: Vec<String>,
    pub value: Expression,
}

/// Everything an expression can refer to.
pub struct Scope<'a> {
    pub columns: &'a dyn Columns,
    /// The sample's metadata fields.
    pub sample: &'a BTreeMap<String, Constant>,
    /// Named objects: each a collection, usually with a `where` condition.
    pub objects: &'a BTreeMap<String, Expression>,
    /// Collections whose attributes are columns of any name: for each, every attribute and
    /// its column, in the order written and never empty.
    pub collections: &'a BTreeMap<String, Vec<(String, String)>>,
    pub helpers: &'a BTreeMap<String, Helper>,
}

/// The members of a collection that pass a condition.
#[derive(Clone, Debug)]
struct Members {
    collection: String,
    mask: Option<Array>,
}

#[derive(Clone, Debug)]
enum Value {
    Array(Array),
    Members(Members),
}

/// What is in force while part of an expression is evaluated.
#[derive(Clone, Default)]
struct Frame {
    /// The collection whose attributes bare names refer to, inside a `where`.
    collection: Option<String>,
    /// The arguments of the helper being evaluated.
    given: Vec<(String, Value)>,
    /// How many objects and helpers are being expanded inside one another.
    depth: usize,
}

/// How deeply objects and helpers may refer to one another before it is taken for a loop.
const DEEPEST: usize = 40;

/// How many values, in words: `1 value`, `2 values`.
fn how_many(count: usize) -> String {
    format!("{count} value{}", if count == 1 { "" } else { "s" })
}

/// What a name that is no column is refused with.
fn unknown(name: &str) -> crate::Error {
    problem(format!("There is no column named {name}"))
}

impl Scope<'_> {
    /// Evaluate an expression to a constant, one value per event, or a list per event.
    pub fn evaluate(&self, expression: &Expression) -> Result<Array> {
        let value = self.value(expression, &Frame::default())?;
        self.array(value, "This expression")
    }

    /// Evaluate a condition to one truth value for each of `events` events.
    pub fn mask(&self, expression: &Expression, events: usize) -> Result<Vec<bool>> {
        match self.evaluate(expression)? {
            Array::Constant(Constant::Truth(truth)) => Ok(vec![truth; events]),
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

    /// Count one more object or helper being expanded, refusing what looks like a loop.
    fn deeper(&self, frame: &Frame, name: &str) -> Result<usize> {
        if frame.depth >= DEEPEST {
            return Err(problem(format!(
                "{name} refers to itself, directly or through other names"
            )));
        }
        Ok(frame.depth + 1)
    }

    /// Require an array where a collection was given.
    fn array(&self, value: Value, what: &str) -> Result<Array> {
        match value {
            Value::Array(array) => Ok(array),
            Value::Members(members) => Err(problem(format!(
                "{what} needs values, but {} is a collection; name one of its attributes, such as {}.pt",
                members.collection, members.collection
            ))),
        }
    }

    /// Fetch a column that must exist.
    fn column(&self, name: &str) -> Result<Array> {
        self.columns.column(name)?.ok_or_else(|| unknown(name))
    }

    /// The column a `[collection.*]` names for an attribute, where the collection is a named
    /// one: `None` for any other collection, and `Some(None)` for an attribute it lacks.
    fn named(&self, collection: &str, name: &str) -> Option<Option<&String>> {
        let named = self.collections.get(collection)?;
        let found = named.iter().find(|(attribute, _)| attribute == name);
        Some(found.map(|(_, column)| column))
    }

    /// One attribute of a whole collection: the column a `[collection.*]` names for it, or
    /// for any other collection `Collection_attribute`, as NanoAOD spells it.
    fn whole(&self, collection: &str, name: &str) -> Result<Array> {
        match self.named(collection, name) {
            None => self.column(&format!("{collection}_{name}")),
            Some(Some(column)) => self.columns.column(column)?.ok_or_else(|| {
                problem(format!(
                    "{collection}.{name} is the column {column}, and there is no column of that name"
                ))
            }),
            Some(None) => {
                let has = self.collections[collection].iter();
                let has: Vec<&str> = has.map(|(attribute, _)| attribute.as_str()).collect();
                Err(problem(format!(
                    "{collection} has no attribute named {name}; it has {}",
                    has.join(", ")
                )))
            }
        }
    }

    /// One attribute of a collection's passing members.
    fn attribute(&self, members: &Members, name: &str) -> Result<Array> {
        let all = self.whole(&members.collection, name)?;
        match &members.mask {
            Some(mask) => f::pick(&all, mask),
            None => Ok(all),
        }
    }

    /// How many members a whole collection has per event: for a named collection the lengths
    /// of the lists of the first column it names, for any other its `nCollection` column.
    fn sizes(&self, collection: &str) -> Result<Array> {
        match self.collections.get(collection) {
            Some(named) => f::lengths(&self.whole(collection, &named[0].0)?),
            None => self.column(&format!("n{collection}")),
        }
    }

    /// How many members of a collection pass, per event.
    fn size(&self, members: &Members) -> Result<Array> {
        match &members.mask {
            Some(mask) => f::count(mask),
            None => self.sizes(&members.collection),
        }
    }

    /// Evaluate one node.
    fn value(&self, expression: &Expression, frame: &Frame) -> Result<Value> {
        use Expression::*;
        let array = |node: &Expression, what: &str| self.array(self.value(node, frame)?, what);
        Ok(Value::Array(match expression {
            Constant(constant) => Array::Constant(constant.clone()),
            Name(name) => return self.name(name, frame),
            Field(left, name) => match left.as_ref() {
                Name(sample) if sample == "sample" => Array::Constant(
                    self.sample
                        .get(name)
                        .cloned()
                        .unwrap_or(crate::Constant::Missing),
                ),
                _ => match self.value(left, frame)? {
                    Value::Members(members) => self.attribute(&members, name)?,
                    Value::Array(_) => {
                        return Err(problem(format!(
                            ".{name} follows something that is not a collection"
                        )))
                    }
                },
            },
            Negate(inner) => match array(inner, "A minus")? {
                Array::Constant(crate::Constant::Float(value)) => {
                    Array::Constant(crate::Constant::Float(-value))
                }
                Array::Constant(crate::Constant::Whole(value)) => {
                    Array::Constant(crate::Constant::Whole(-value))
                }
                other => Array::Constant(crate::Constant::Whole(-1))
                    .combine(Operator::Multiply, &other)?,
            },
            Not(inner) => array(inner, "not")?.combine(
                Operator::Equal,
                &Array::Constant(crate::Constant::Truth(false)),
            )?,
            Binary(operator, left, right) => {
                let left = array(left, "An operator")?;
                let truth = |wanted: bool| matches!(&left, Array::Constant(crate::Constant::Truth(found)) if *found == wanted);
                let decided = (*operator == Operator::And && truth(false))
                    || (*operator == Operator::Or && truth(true));
                if decided {
                    left
                } else {
                    left.combine(*operator, &array(right, "An operator")?)?
                }
            }
            Pick(values, mask) => {
                let mask = match self.value(mask, frame)? {
                    Value::Members(Members {
                        mask: Some(mask), ..
                    }) => mask,
                    Value::Array(mask) => mask,
                    Value::Members(_) => {
                        return Err(problem("A pick needs a condition, not a whole collection"))
                    }
                };
                f::pick(&array(values, "A pick")?, &mask)?
            }
            Where(left, condition) => {
                let Value::Members(members) = self.value(left, frame)? else {
                    return Err(problem(
                        "where follows a collection, such as Jet where pt > 30",
                    ));
                };
                let inside = Frame {
                    collection: Some(members.collection.clone()),
                    given: frame.given.clone(),
                    depth: frame.depth,
                };
                let mut mask = self.array(self.value(condition, &inside)?, "A where condition")?;
                let marks = match &mask {
                    Array::Object { offsets, values } if values.kind == Kind::B1 => offsets,
                    Array::Object { .. } => {
                        return Err(problem(
                            "The condition after where must be true or false for each member",
                        ))
                    }
                    _ => {
                        return Err(problem(format!(
                            "The condition after where says nothing about each {}",
                            members.collection
                        )))
                    }
                };
                // A named collection has no count column to check against; each attribute's
                // own lists are checked when the mask picks from them.
                if let Some(Array::Event(sizes)) =
                    self.columns.column(&format!("n{}", members.collection))?
                {
                    let fits = sizes.len() + 1 == marks.len()
                        && marks
                            .windows(2)
                            .enumerate()
                            .all(|(event, pair)| (pair[1] - pair[0]) as f64 == sizes.number(event));
                    if !fits {
                        return Err(problem(format!(
                            "The condition after where is not about the members of {}",
                            members.collection
                        )));
                    }
                }
                if let Some(earlier) = &members.mask {
                    mask = earlier.combine(Operator::And, &mask)?;
                }
                return Ok(Value::Members(Members {
                    collection: members.collection,
                    mask: Some(mask),
                }));
            }
            In(left, list) => {
                let left = array(left, "in")?;
                let mut found = Array::Constant(crate::Constant::Truth(false));
                for member in list {
                    let equal = left.combine(Operator::Equal, &array(member, "in")?)?;
                    found = found.combine(Operator::Or, &equal)?;
                }
                found
            }
            Call(name, arguments) => return self.call(name, arguments, frame),
        }))
    }

    /// Resolve a bare name: an argument, an attribute inside `where`, an object, a named
    /// collection, a column, or a collection that columns begin with followed by `_`.
    fn name(&self, name: &str, frame: &Frame) -> Result<Value> {
        if let Some((_, value)) = frame.given.iter().rev().find(|(given, _)| given == name) {
            return Ok(value.clone());
        }
        if let Some(collection) = &frame.collection {
            match self.named(collection, name) {
                Some(Some(_)) => return self.whole(collection, name).map(Value::Array),
                Some(None) => {}
                None => {
                    let column = format!("{collection}_{name}");
                    if let Some(column) = self.columns.column(&column)? {
                        return Ok(Value::Array(column));
                    }
                }
            }
        }
        if let Some(object) = self.objects.get(name) {
            let inside = Frame {
                depth: self.deeper(frame, name)?,
                ..Frame::default()
            };
            return self.value(object, &inside);
        }
        if self.collections.contains_key(name) {
            return Ok(Value::Members(Members {
                collection: name.to_owned(),
                mask: None,
            }));
        }
        if let Some(column) = self.columns.column(name)? {
            return Ok(Value::Array(column));
        }
        if !self.columns.begins(&format!("{name}_")) {
            return Err(unknown(name));
        }
        Ok(Value::Members(Members {
            collection: name.to_owned(),
            mask: None,
        }))
    }

    /// Call a named operation or a helper.
    fn call(&self, name: &str, arguments: &[Expression], frame: &Frame) -> Result<Value> {
        if name == "if" && arguments.len() == 3 {
            let condition = self.array(self.value(&arguments[0], frame)?, "if")?;
            if let Array::Constant(crate::Constant::Truth(truth)) = condition {
                return self.value(&arguments[if truth { 1 } else { 2 }], frame);
            }
        }
        if let Some(helper) = self.helpers.get(name) {
            if helper.takes.len() != arguments.len() {
                return Err(problem(format!(
                    "{name} takes {}, not {}",
                    how_many(helper.takes.len()),
                    arguments.len()
                )));
            }
            let mut given = Vec::new();
            for (takes, argument) in helper.takes.iter().zip(arguments) {
                given.push((takes.clone(), self.value(argument, frame)?));
            }
            return self.value(
                &helper.value,
                &Frame {
                    collection: None,
                    given,
                    depth: self.deeper(frame, name)?,
                },
            );
        }
        let values: Vec<Value> = arguments
            .iter()
            .map(|argument| self.value(argument, frame))
            .collect::<Result<_>>()?;
        let needs = |count: usize| {
            if values.len() == count {
                Ok(())
            } else {
                Err(problem(format!(
                    "{name} takes {}, not {}",
                    how_many(count),
                    values.len()
                )))
            }
        };
        if name == "count" {
            needs(1)?;
            return Ok(Value::Array(match &values[0] {
                Value::Members(members) => self.size(members)?,
                Value::Array(array) => f::count(array)?,
            }));
        }
        let arrays: Vec<Array> = values
            .into_iter()
            .map(|value| self.array(value, name))
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
            Array::Constant(crate::Constant::Missing) => {
                Err(problem(format!("{name}: the sample has no such field")))
            }
            _ => Err(problem(format!("{name} needs {what} as text"))),
        };
        Ok(Value::Array(match name {
            "sum" => one(f::sum)?,
            "any" => one(|array| f::any_all(array, false))?,
            "all" => one(|array| f::any_all(array, true))?,
            "index" => one(f::index)?,
            "isfinite" => one(f::finite)?,
            "abs" => one(f::magnitude)?,
            "sign" => one(f::sign)?,
            "sqrt" => float(f64::sqrt)?,
            "cos" => float(f64::cos)?,
            "sin" => float(f64::sin)?,
            "cosh" => float(f64::cosh)?,
            "sinh" => float(f64::sinh)?,
            "exp" => float(f64::exp)?,
            "log" => float(f64::ln)?,
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
                    (Array::Constant(crate::Constant::Missing), _)
                    | (_, Array::Constant(crate::Constant::Missing)) => false,
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
                    "There is no operation or helper named {name}"
                )))
            }
        }))
    }
}

/// Give a constant result one value per event.
pub fn per_event(array: Array, events: usize) -> Result<Array> {
    Ok(match array {
        Array::Constant(constant) => Array::Event(constant.alone(events)?),
        other => other,
    })
}
