//! What an expression is checked for when a recipe is read, before any event is: every function it
//! calls exists and is given its number of values, the fixed values some take are of their kind,
//! and a true-or-false value stands wherever one is needed, as far as the text tells.

use crate::array::{text_refused, Constant, Operator};
use crate::evaluate::uncallable;
use crate::parse::Expression;
use std::collections::BTreeMap;

/// What an expression gives, as far as its text tells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Sort {
    Truth,
    Number,
    Text,
    /// A branch, a sample's field, or what depends on one.
    Unknown,
}

use Sort::*;

/// What two values of one result give together: a truth only when both are.
fn either(a: Sort, b: Sort) -> Sort {
    match (a, b) {
        (Truth, Truth) => Truth,
        (Number | Truth, Number | Truth) => Number,
        _ => Unknown,
    }
}

/// Words for a value that must be a true-or-false one and is not; none when it may be.
pub(crate) fn untrue(sort: Sort, what: &str) -> Option<String> {
    match sort {
        Number => Some(format!("{what} needs a true-or-false value, not a number: to test a number, compare it, as x != 0")),
        Text => Some(format!("{what} needs a true-or-false value, not text")),
        _ => None,
    }
}

/// Whether function `name` takes a list per event, never a fixed value, as its value `at`.
fn list_at(name: &str, at: usize) -> bool {
    match name {
        "count" | "sum" | "any" | "all" | "least" | "greatest" | "least_index" | "greatest_index" | "first" | "nth"
        | "index" | "sorted" | "order" | "pair_first" | "pair_second" => at == 0,
        "take" | "listed" | "pair_dr" => at < 2,
        "join" => true,
        "dr_nearest" | "dr_second_nearest" | "dr_farthest" | "dr_second_farthest" | "nearest_index" | "second_nearest_index"
        | "farthest_index" | "second_farthest_index" | "mass" | "pt" | "eta" | "phi" | "rapidity" | "pair_mass" | "pair_pt"
        | "pair_eta" | "pair_phi" | "pair_rapidity" => at < 4,
        _ => false,
    }
}

/// The checks of one recipe's expressions: what each defined name gives, and the problems found.
pub(crate) struct Checks<'a> {
    pub sorts: &'a BTreeMap<String, Sort>,
    pub said: Vec<String>,
}

impl Checks<'_> {
    /// What `expression` gives, saying every problem its text shows.
    pub fn sort(&mut self, expression: &Expression) -> Sort {
        use Expression::{Binary, Call, In, Name, Negate, Not, Pick, Sample};
        match expression {
            Expression::Constant(Constant::Whole(_) | Constant::Float(_)) => Number,
            Expression::Constant(Constant::Truth(_)) => Truth,
            Expression::Constant(Constant::Text(_)) => Text,
            Expression::Constant(Constant::Missing(_)) => Unknown,
            Name(name) => self.sorts.get(name).copied().unwrap_or(Unknown),
            Sample(field) => {
                if crate::ADDED.contains(&field.as_str()) {
                    self.said.push(format!("sample.{}", crate::array::recorded(field)));
                }
                Unknown
            }
            Negate(inner) => {
                match self.sort(inner) {
                    Truth => self.said.push("- needs a number, not a true-or-false value: write if(x, -1, 0), or not x".into()),
                    Text => self.said.push("- needs a number, not text".into()),
                    _ => {}
                }
                Number
            }
            Not(inner) => {
                let sort = self.sort(inner);
                self.said.extend(untrue(sort, "not"));
                Truth
            }
            Binary(operator, left, right) => {
                let (a, b) = (self.sort(left), self.sort(right));
                self.binary(*operator, a, b)
            }
            Pick(values, mask) => {
                if fixed(values) {
                    self.said.push("A pick needs a list per event to pick from, not a fixed value".into());
                }
                let sort = self.sort(mask);
                self.said.extend(untrue(sort, "A mask"));
                self.sort(values)
            }
            In(left, list) => {
                self.sort(left);
                for member in list {
                    self.sort(member);
                }
                Truth
            }
            Call(name, arguments) => self.call(name, arguments),
        }
    }

    /// What an operator gives of values that give `a` and `b`, saying what it cannot take.
    fn binary(&mut self, operator: Operator, a: Sort, b: Sort) -> Sort {
        use Operator::*;
        match operator {
            And | Or => {
                self.said.extend(untrue(a, &format!("{}'s left side", operator.word())));
                self.said.extend(untrue(b, &format!("{}'s right side", operator.word())));
                Truth
            }
            _ if operator.arithmetic() => {
                if a == Text || b == Text {
                    self.said.push(text_refused(operator));
                } else if (a, b) == (Truth, Truth) {
                    self.said.push("Arithmetic on two true-or-false values: use and, or, or count, or make one a number, as if(x, 1, 0)".into());
                }
                Number
            }
            Equal | NotEqual => Truth,
            _ => {
                if a == Text || b == Text {
                    self.said.push(text_refused(operator));
                }
                Truth
            }
        }
    }

    /// What a call gives, saying every problem with its name, its number of values, and the
    /// fixed values it takes.
    fn call(&mut self, name: &str, arguments: &[Expression]) -> Sort {
        let sorts: Vec<Sort> = arguments.iter().map(|argument| self.sort(argument)).collect();
        if self.sorts.contains_key(name) {
            self.said.push(format!("{name} is a defined name, not a function: write it without brackets, as {name}"));
            return Unknown;
        }
        if let Some(why) = uncallable(name, arguments.len()) {
            self.said.push(why);
            return Unknown;
        }
        for (at, argument) in arguments.iter().enumerate() {
            if list_at(name, at) && fixed(argument) {
                self.said.push(format!("{name} needs a list per event, not a fixed value"));
            }
        }
        let literal = |at: usize| match arguments.get(at) {
            Some(Expression::Constant(constant)) => Some(constant.clone()),
            Some(Expression::Negate(inner)) => match &**inner {
                Expression::Constant(Constant::Whole(value)) => Some(Constant::Whole(-value)),
                Expression::Constant(Constant::Float(value)) => Some(Constant::Float(-value)),
                _ => None,
            },
            _ => None,
        };
        let mut say = |said: String| self.said.push(said);
        match (name, literal(1)) {
            ("nth", Some(Constant::Whole(n))) if n < 0 => say("nth counts positions from 0, so its position is 0 or more".into()),
            ("nth", Some(Constant::Float(_) | Constant::Truth(_) | Constant::Text(_))) => say("nth needs a whole number for the position, counting from 0, such as nth(Jet_pt, 1, -1)".into()),
            ("bit_and" | "shift_right", Some(Constant::Float(_) | Constant::Truth(_) | Constant::Text(_))) => say(format!("{name} needs a whole number as its second value")),
            ("shift_right", Some(Constant::Whole(n))) if n < 0 => say("shift_right cannot shift by a negative number of bits".into()),
            ("order" | "sorted", Some(Constant::Text(direction))) if direction != "ascending" && direction != "descending" => {
                say(format!("{name} takes 'ascending' or 'descending' as its direction, not {direction:?}"))
            }
            ("order" | "sorted", Some(Constant::Whole(_) | Constant::Float(_) | Constant::Truth(_))) => say(format!("{name} needs the direction as text, 'ascending' or 'descending'")),
            ("stored", Some(Constant::Text(kind))) => {
                if let Err(e) = crate::Kind::named(&kind) {
                    say(format!("stored: {}", e.0));
                }
            }
            ("stored", Some(Constant::Whole(_) | Constant::Float(_) | Constant::Truth(_))) => say("stored needs a number kind as text, such as stored(nJet, 'u1')".into()),
            ("contains", _) if sorts.iter().any(|sort| matches!(sort, Number | Truth)) => say("contains needs two pieces of text, as contains(sample.kind, 'sig')".into()),
            _ => {}
        }
        match (name, literal(0)) {
            ("branch", Some(Constant::Whole(_) | Constant::Float(_) | Constant::Truth(_))) => say("branch needs a branch name as text, as branch('Jet.pt')".into()),
            ("stored", Some(_)) => say("stored needs a column, not a fixed value".into()),
            _ => {}
        }
        let index = name.ends_with("_index");
        let otherwise = match name {
            "least" | "greatest" | "first" | "least_index" | "greatest_index" => Some(1),
            "nth" | "take" => Some(2),
            _ if name.starts_with("dr_") || index => Some(4),
            _ => None,
        };
        match otherwise.and_then(literal) {
            Some(Constant::Text(_)) => say(format!("{name} needs a number to use when there is none, not text")),
            Some(Constant::Float(_) | Constant::Truth(_)) if index => say(format!("{name} needs a whole number to use when there is no member, such as -1")),
            _ => {}
        }
        if name == "if" {
            self.said.extend(untrue(sorts[0], "if's condition"));
        }
        match name {
            "any" | "all" | "isfinite" | "contains" | "listed" => Truth,
            "abs" | "sign" | "floor" | "ceil" | "sorted" | "pair_first" | "pair_second" => sorts[0],
            "least" | "greatest" | "first" | "min" | "max" => either(sorts[0], sorts[1]),
            "nth" | "take" => either(sorts[0], sorts[2]),
            "clip" => either(either(sorts[0], sorts[1]), sorts[2]),
            "if" => either(sorts[1], sorts[2]),
            "join" => sorts[1..].iter().fold(sorts[0], |held, sort| either(held, *sort)),
            "branch" => Unknown,
            "stored" => match literal(1) {
                Some(Constant::Text(kind)) if kind == "b1" => Truth,
                Some(Constant::Text(_)) => Number,
                _ => Unknown,
            },
            _ => Number,
        }
    }
}

/// Whether an expression is one fixed value for every event: a number, text or truth written
/// in, or a field of the sample.
fn fixed(expression: &Expression) -> bool {
    matches!(expression, Expression::Constant(_) | Expression::Sample(_))
}
