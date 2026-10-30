//! A recipe as it is saved: TOML naming expressions, cuts and columns, each value one
//! expression.
//!
//! ```toml
//! format = "rvr-recipe"
//! version = 1
//!
//! [define]
//! good_muon = "Muon_pt > 15 and Muon_tightId"
//!
//! [keep]
//! two_muons = "count(good_muon) == 2"
//! hem = { if = "sample.year == 2018", apply = "count(Jet_pt[Jet_eta < -1.3] > 30) == 0" }
//!
//! [columns]
//! branches = ["Muon_pt", "MET"]
//! ht = "sum(Jet_pt)"
//! rho = "branch(sample.rho)"
//! ```
//!
//! Every table, key and function is described in this crate's README.

use crate::parse::{parse, Expression};
use crate::{problem, Result};
use std::collections::{BTreeMap, BTreeSet};
use toml::{Table, Value};

/// One named condition of a recipe; every cut must pass for an event to be kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub name: String,
    /// The samples the cut applies to, when it is not all of them.
    pub when: Option<Expression>,
    pub pass: Expression,
}

impl Cut {
    /// Its expressions: the condition of the samples it applies to, if any, then the cut.
    pub fn expressions(&self) -> impl Iterator<Item = &Expression> {
        self.when.iter().chain([&self.pass])
    }
}

/// A parsed recipe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipe {
    pub title: Option<String>,
    /// The names `[define]` gives expressions; none is stored.
    pub defines: BTreeMap<String, Expression>,
    /// The cuts in the order written, which is the order of the cutflow.
    pub keep: Vec<Cut>,
    /// The stored columns in the order written, each with the expression that makes it.
    pub columns: Vec<(String, Expression)>,
    /// The number kind a column is stored as, where it is not the kind it is computed in.
    pub stored: BTreeMap<String, crate::Kind>,
    /// The derived columns in the order written, each with its expression as written.
    pub derived: Vec<(String, String)>,
}

/// Read an expression held in a TOML value, naming where it sits in any complaint.
fn expression(value: &Value, place: &str) -> Result<Expression> {
    let text = value
        .as_str()
        .ok_or_else(|| problem(format!("{place} must be an expression in quotes")))?;
    parse(text).map_err(|error| problem(format!("{place}: {}", error.0)))
}

/// Read a table held under a key, or none when the key is absent.
fn table<'a>(root: &'a Table, key: &str) -> Result<Option<&'a Table>> {
    match root.get(key) {
        None => Ok(None),
        Some(Value::Table(table)) => Ok(Some(table)),
        Some(_) => Err(problem(format!("[{key}] must be a table"))),
    }
}

/// Refuse keys a table does not define, so that a misspelt key is not silently ignored.
fn only(found: &Table, allowed: &[&str], place: &str) -> Result<()> {
    match found.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(problem(format!(
            "{place} has no setting named {key}; it takes {}",
            allowed.join(", ")
        ))),
        None => Ok(()),
    }
}

/// The names an expression gives each row, which no column may have: its sample, its entry, and
/// its sample's file.
pub const RESERVED: [&str; 3] = ["sample", "entry", "file"];

/// Whether text is one plain name, as an expression would read it.
pub fn plain(text: &str) -> bool {
    matches!(parse(text), Ok(Expression::Name(name)) if name == text)
}

/// Add every name an expression mentions: branches, defined names, and text that may name a
/// branch (`branch('…')`).
pub fn names(expression: &Expression, found: &mut BTreeSet<String>) {
    gather(expression, found, true, false);
}

/// Add the bare names an expression uses, branches and defined names, and no text.
pub fn uses(expression: &Expression, found: &mut BTreeSet<String>) {
    gather(expression, found, false, false);
}

/// Add the columns an expression reads: its bare names, and each `branch('…')`'s.
pub fn reads(expression: &Expression, found: &mut BTreeSet<String>) {
    gather(expression, found, false, true);
}

/// Add the bare names an expression holds; with `text` the text it holds too, and with
/// `branches` the text each `branch` names.
fn gather(expression: &Expression, found: &mut BTreeSet<String>, text: bool, branches: bool) {
    use Expression::*;
    match expression {
        Constant(crate::Constant::Text(held)) if text => {
            found.insert(held.clone());
        }
        Constant(_) | Sample(_) => {}
        Name(name) => {
            found.insert(name.clone());
        }
        Call(function, arguments) if branches && function == "branch" => match arguments.as_slice() {
            [Constant(crate::Constant::Text(held))] => {
                found.insert(held.clone());
            }
            _ => arguments.iter().for_each(|argument| gather(argument, found, text, branches)),
        },
        Call(_, arguments) => arguments
            .iter()
            .for_each(|argument| gather(argument, found, text, branches)),
        In(left, list) => {
            gather(left, found, text, branches);
            list.iter().for_each(|member| gather(member, found, text, branches));
        }
        Negate(inner) | Not(inner) => gather(inner, found, text, branches),
        Binary(_, left, right) | Pick(left, right) => {
            gather(left, found, text, branches);
            gather(right, found, text, branches);
        }
    }
}

/// Whether an expression names a branch by anything but text, as `branch(sample.KEY)`.
fn by_sample(expression: &Expression) -> bool {
    use Expression::*;
    match expression {
        Call(function, arguments) => {
            let text = matches!(arguments.as_slice(), [Constant(crate::Constant::Text(_))]);
            (function == "branch" && !text) || arguments.iter().any(by_sample)
        }
        In(left, list) => by_sample(left) || list.iter().any(by_sample),
        Negate(inner) | Not(inner) => by_sample(inner),
        Binary(_, left, right) | Pick(left, right) => by_sample(left) || by_sample(right),
        Constant(_) | Sample(_) | Name(_) => false,
    }
}

/// Say why a recipe's text is not TOML: the line, and what the parser found there. The
/// parser has no words for some mistakes, such as a name with no value; those are said here,
/// so that a problem is never an empty sentence.
fn unparsed(text: &str, error: &toml::de::Error) -> String {
    let line = error.span().map(|span| {
        let before = &text.as_bytes()[..span.start.min(text.len())];
        before.iter().filter(|byte| **byte == b'\n').count() + 1
    });
    let found = match error.message().trim() {
        "" => "a name has no value after its =",
        said => said,
    };
    match line {
        Some(line) => format!("The recipe is not valid TOML, line {line}: {found}"),
        None => format!("The recipe is not valid TOML: {found}"),
    }
}

impl Recipe {
    /// Read a recipe from its TOML text.
    pub fn read(text: &str) -> Result<Self> {
        let root: Table = text
            .parse()
            .map_err(|error: toml::de::Error| problem(unparsed(text, &error)))?;
        if root.get("format").and_then(Value::as_str) != Some("rvr-recipe")
            || root.get("version").and_then(Value::as_integer) != Some(1)
        {
            return Err(problem(
                "A recipe starts with format = \"rvr-recipe\" and version = 1",
            ));
        }
        // The tables of the earlier language, each said in what it became.
        let became = [
            ("copy", "the branches it stored are listed under [columns] as branches = [...]"),
            ("column", "its columns are lines of [columns], NAME = \"expression\""),
            ("object", "write it as a line of [define], a mask: good_muon = \"Muon_pt > 20 and Muon_tightId\", used as Muon_pt[good_muon]"),
            ("helper", "write it as a line of [define], good = \"expression\", used by its name alone"),
            ("collection", "name each branch in full, as the file spells it, or through branch('…')"),
        ];
        if let Some((table, how)) = became.iter().find(|(table, _)| root.contains_key(*table)) {
            return Err(problem(format!(
                "A recipe has no [{table}] table: {how}. A recipe's tables are [define], [keep], [columns] and [derived]."
            )));
        }
        only(
            &root,
            &["format", "version", "title", "define", "keep", "columns", "derived"],
            "A recipe",
        )?;
        let mut recipe = Self {
            title: root.get("title").and_then(Value::as_str).map(str::to_owned),
            ..Self::default()
        };
        let defines = table(&root, "define")?;
        let every: BTreeSet<&String> = defines.iter().flat_map(|table| table.keys()).collect();
        for (name, value) in defines.into_iter().flatten() {
            let place = format!("[define] {name}");
            if !plain(name) || name == "sample" {
                return Err(problem(format!(
                    "{place}: a defined name is one plain name an expression can use, such as good_muon"
                )));
            }
            let value = expression(value, &place)?;
            let mut used = BTreeSet::new();
            uses(&value, &mut used);
            let later = used
                .iter()
                .find(|used| every.contains(used) && !recipe.defines.contains_key(*used));
            if let Some(later) = later {
                let itself = if later == name {
                    ", itself"
                } else {
                    ", which is defined below it"
                };
                return Err(problem(format!("{place} uses {later}{itself}: a defined name may use only the names defined above it")));
            }
            recipe.defines.insert(name.clone(), value);
        }
        for (name, value) in table(&root, "keep")?.into_iter().flatten() {
            let place = format!("[keep] {name}");
            let cut = match value {
                Value::Table(cut) => {
                    if cut.contains_key("pass") {
                        return Err(problem(format!("{place}: a cut for some samples says what it applies with apply = \"…\", not pass")));
                    }
                    only(cut, &["if", "apply"], &place)?;
                    let pass = cut.get("apply").ok_or_else(|| {
                        problem(format!("{place} needs apply = \"…\": the cut, for the samples its if is true for"))
                    })?;
                    let when = cut
                        .get("if")
                        .map(|when| expression(when, &format!("{place} if")))
                        .transpose()?;
                    Cut {
                        name: name.clone(),
                        when,
                        pass: expression(pass, &place)?,
                    }
                }
                plain => Cut {
                    name: name.clone(),
                    when: None,
                    pass: expression(plain, &place)?,
                },
            };
            recipe.keep.push(cut);
        }
        for (name, value) in table(&root, "columns")?.into_iter().flatten() {
            if name == "branches" {
                let list = value
                    .as_array()
                    .ok_or_else(|| problem("[columns] branches must be a list of branch names"))?;
                for branch in list {
                    let branch = branch.as_str().filter(|branch| !branch.is_empty());
                    let branch = branch.ok_or_else(|| {
                        problem("[columns] branches must be a list of branch names")
                    })?;
                    if recipe.defines.contains_key(branch) {
                        return Err(problem(format!("[columns] branches lists {branch}, a name [define] gives an expression: list branches only, and store the expression as a column of its own, such as {branch}_stored = \"{branch}\"")));
                    }
                    let copied = Expression::Name(branch.to_owned());
                    recipe.columns.push((branch.to_owned(), copied));
                }
                continue;
            }
            let place = format!("[columns] {name}");
            let value = match value {
                Value::Table(column) => {
                    only(column, &["value", "stored"], &place)?;
                    if let Some(kind) = column.get("stored") {
                        let kind = kind.as_str().ok_or_else(|| {
                            problem(format!(
                                "{place} stored must be a number kind such as \"i4\""
                            ))
                        })?;
                        recipe
                            .stored
                            .insert(name.clone(), crate::Kind::named(kind)?);
                    }
                    column
                        .get("value")
                        .ok_or_else(|| problem(format!("{place} needs a value")))?
                }
                plain => plain,
            };
            recipe
                .columns
                .push((name.clone(), expression(value, &place)?));
        }
        for (name, _) in &recipe.columns {
            if name.is_empty() || RESERVED.contains(&name.as_str()) {
                return Err(problem(format!("A column cannot be named \"{name}\"")));
            }
        }
        let mut names: Vec<&String> = recipe.columns.iter().map(|(name, _)| name).collect();
        names.sort();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(problem(format!("Two columns are named {}", pair[0])));
        }
        // A derived column is computed from what the file holds: the stored columns and the
        // derived columns above it.
        let mut held: BTreeSet<String> = recipe.columns.iter().map(|(name, _)| name.clone()).collect();
        let derived = table(&root, "derived")?;
        let every: BTreeSet<&String> = derived.iter().flat_map(|table| table.keys()).collect();
        for (name, value) in derived.into_iter().flatten() {
            let place = format!("[derived] {name}");
            if !plain(name) || RESERVED.contains(&name.as_str()) {
                return Err(problem(format!(
                    "{place}: a derived column is named with one plain name, such as ht_over_met"
                )));
            }
            if held.contains(name) || recipe.defines.contains_key(name) {
                return Err(problem(format!("{place}: {name} is already the name of a column or of a line of [define]; give the derived column another")));
            }
            let parsed = expression(value, &place)?;
            if by_sample(&parsed) {
                return Err(problem(format!("{place}: branch(sample.KEY) names another column for each sample, which a derived column cannot be checked against before the press: name the column, as branch('Jet.pt')")));
            }
            let mut used = BTreeSet::new();
            reads(&parsed, &mut used);
            if let Some(missing) = used.iter().find(|used| !held.contains(*used)) {
                let why = if recipe.defines.contains_key(missing) {
                    format!("{missing} is a line of [define], which the file does not keep")
                } else if every.contains(missing) {
                    format!("{missing} is a derived column below it")
                } else {
                    format!("the file does not store {missing}")
                };
                return Err(problem(format!("{place} uses {missing}, but {why}: a derived column uses only the columns the file stores, under [columns], and the derived columns above it")));
            }
            held.insert(name.clone());
            recipe.derived.push((name.clone(), value.as_str().unwrap_or_default().to_owned()));
        }
        Ok(recipe)
    }
}
