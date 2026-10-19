//! A recipe as it is saved: TOML naming objects, helpers, cuts, and columns, each value
//! one expression.
//!
//! ```toml
//! format = "rvr-recipe"
//! version = 1
//!
//! [collection.lep]
//! pt = "PT_lep"
//! eta = "Eta_lep"
//!
//! [object.good_muon]
//! from = "Muon"
//! where = "tight and pt > 15"
//!
//! [helper.hem_veto]
//! pass = "count(Jet where eta < -1.3 and phi < -0.87) == 0"
//!
//! [keep]
//! two_muons = "count(good_muon) == 2"
//! hem = { if = "sample.year == 2018", apply = "hem_veto()" }
//!
//! [copy]
//! branches = ["Muon_pt", "MET"]
//! rho = "branch(sample.rho)"
//! ```
//!
//! Every key is described in the README of rvr ("recipe.toml"). The layout is not yet an accepted
//! contract: it may change while rowver 14 is proposed.

use crate::evaluate::Helper;
use crate::parse::{parse, Expression};
use crate::{problem, Result};
use std::collections::BTreeMap;
use toml::{Table, Value};

/// One named condition of a recipe; every cut must pass for an event to be kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub name: String,
    /// The samples the cut applies to, when it is not all of them.
    pub when: Option<Expression>,
    pub pass: Expression,
}

/// A parsed recipe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipe {
    pub title: Option<String>,
    pub objects: BTreeMap<String, Expression>,
    /// Collections whose attributes are columns of any name: for each, every attribute and
    /// its column, in the order written and never empty. The first column's lists say how
    /// many members the collection has.
    pub collections: BTreeMap<String, Vec<(String, String)>>,
    pub helpers: BTreeMap<String, Helper>,
    /// The cuts in the order written, which is the order of the cutflow.
    pub keep: Vec<Cut>,
    /// The copied columns in order: the name written to the file, and the branch it comes from.
    pub copy: Vec<(String, Expression)>,
    /// The computed columns, in order, written after the copied ones.
    pub columns: Vec<(String, Expression)>,
    /// The number kind a computed column is stored as, where it is not the kind it is computed in.
    pub stored: BTreeMap<String, crate::Kind>,
}

/// Read an expression held in a TOML value, naming where it sits in any complaint.
fn expression(value: &Value, place: &str) -> Result<Expression> {
    let text = value
        .as_str()
        .ok_or_else(|| problem(format!("{place} must be an expression in quotes")))?;
    parse(text).map_err(|error| problem(format!("{place}: {}", error.0)))
}

/// Read a table held under a key, or an empty one when the key is absent.
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

/// Whether text is one plain name, as an expression would read it.
fn plain(text: &str) -> bool {
    matches!(parse(text), Ok(Expression::Name(name)) if name == text)
}

/// Add every name an expression mentions: branches, collections, attributes, and helpers.
pub fn names(expression: &Expression, found: &mut std::collections::BTreeSet<String>) {
    use Expression::*;
    match expression {
        Constant(crate::Constant::Text(text)) => {
            found.insert(text.clone());
        }
        Constant(_) => {}
        Name(name) => {
            found.insert(name.clone());
        }
        Field(left, name) => {
            found.insert(name.clone());
            names(left, found);
        }
        Call(_, arguments) => arguments.iter().for_each(|argument| names(argument, found)),
        In(left, list) => {
            names(left, found);
            list.iter().for_each(|member| names(member, found));
        }
        Negate(inner) | Not(inner) => names(inner, found),
        Binary(_, left, right) | Pick(left, right) | Where(left, right) => {
            names(left, found);
            names(right, found);
        }
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
    /// Every name the recipe mentions anywhere, which bounds the branches it can ask for.
    pub fn mentions(&self) -> std::collections::BTreeSet<String> {
        let mut found = std::collections::BTreeSet::new();
        let helpers = self.helpers.values().map(|helper| &helper.value);
        let cuts = self
            .keep
            .iter()
            .flat_map(|cut| cut.when.iter().chain([&cut.pass]));
        let columns = self
            .copy
            .iter()
            .chain(&self.columns)
            .map(|(_, value)| value);
        for expression in self
            .objects
            .values()
            .chain(helpers)
            .chain(cuts)
            .chain(columns)
        {
            names(expression, &mut found);
        }
        // Of a collection the recipe mentions: the column that counts its members, and
        // the column behind each attribute the recipe names anywhere.
        for (name, named) in &self.collections {
            if found.contains(name) {
                let used = named
                    .iter()
                    .enumerate()
                    .filter(|(at, (attribute, _))| *at == 0 || found.contains(attribute));
                let columns: Vec<String> = used.map(|(_, (_, column))| column.clone()).collect();
                found.extend(columns);
            }
        }
        found
    }

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
        only(
            &root,
            &[
                "format",
                "version",
                "title",
                "collection",
                "object",
                "helper",
                "keep",
                "copy",
                "column",
            ],
            "A recipe",
        )?;
        let mut recipe = Self {
            title: root.get("title").and_then(Value::as_str).map(str::to_owned),
            ..Self::default()
        };
        for (name, value) in table(&root, "collection")?.into_iter().flatten() {
            let place = format!("[collection.{name}]");
            let named = value
                .as_table()
                .ok_or_else(|| problem(format!("{place} must be a table")))?;
            if name == "sample" || !plain(name) {
                return Err(problem(format!(
                    "{place}: a collection needs a plain name that an expression can use"
                )));
            }
            let mut columns = Vec::new();
            for (attribute, column) in named {
                if !plain(attribute) {
                    return Err(problem(format!(
                        "{place}: {attribute} is not a name an expression can use"
                    )));
                }
                let column = column.as_str().filter(|column| !column.is_empty());
                let column = column.ok_or_else(|| {
                    problem(format!("{place} {attribute} must name a column in quotes"))
                })?;
                columns.push((attribute.clone(), column.to_owned()));
            }
            if columns.is_empty() {
                return Err(problem(format!("{place} names no column")));
            }
            recipe.collections.insert(name.clone(), columns);
        }
        for (name, value) in table(&root, "object")?.into_iter().flatten() {
            let place = format!("[object.{name}]");
            if recipe.collections.contains_key(name) {
                return Err(problem(format!(
                    "{name} is both a collection and an object; give the object another name"
                )));
            }
            let object = value
                .as_table()
                .ok_or_else(|| problem(format!("{place} must be a table")))?;
            only(object, &["from", "where"], &place)?;
            let from = object.get("from").and_then(Value::as_str);
            let from = from.ok_or_else(|| problem(format!("{place} needs from = a collection")))?;
            let mut members = Expression::Name(from.to_owned());
            if let Some(condition) = object.get("where") {
                let condition = expression(condition, &format!("{place} where"))?;
                members = Expression::Where(Box::new(members), Box::new(condition));
            }
            recipe.objects.insert(name.clone(), members);
        }
        for (name, value) in table(&root, "helper")?.into_iter().flatten() {
            let place = format!("[helper.{name}]");
            let helper = value
                .as_table()
                .ok_or_else(|| problem(format!("{place} must be a table")))?;
            only(helper, &["about", "takes", "pass", "value"], &place)?;
            let body = match (helper.get("pass"), helper.get("value")) {
                (Some(body), None) | (None, Some(body)) => expression(body, &place)?,
                _ => {
                    return Err(problem(format!(
                        "{place} needs exactly one of pass or value"
                    )))
                }
            };
            let takes = match helper.get("takes") {
                None => Vec::new(),
                Some(Value::Array(names)) => names
                    .iter()
                    .map(|name| name.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(|| problem(format!("{place} takes must be a list of names")))?,
                Some(_) => return Err(problem(format!("{place} takes must be a list of names"))),
            };
            recipe
                .helpers
                .insert(name.clone(), Helper { takes, value: body });
        }
        for (name, value) in table(&root, "keep")?.into_iter().flatten() {
            let place = format!("[keep] {name}");
            let cut = match value {
                Value::Table(cut) => {
                    only(cut, &["if", "apply", "pass"], &place)?;
                    let pass = match (cut.get("apply"), cut.get("pass")) {
                        (Some(body), None) | (None, Some(body)) => expression(body, &place)?,
                        _ => {
                            return Err(problem(format!(
                                "{place} needs exactly one of apply or pass"
                            )))
                        }
                    };
                    let when = cut
                        .get("if")
                        .map(|when| expression(when, &format!("{place} if")))
                        .transpose()?;
                    Cut {
                        name: name.clone(),
                        when,
                        pass,
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
        for (name, value) in table(&root, "copy")?.into_iter().flatten() {
            match (name.as_str(), value) {
                ("branches", Value::Array(names)) => {
                    for branch in names {
                        let branch = branch.as_str().ok_or_else(|| {
                            problem("[copy] branches must be a list of branch names")
                        })?;
                        recipe
                            .copy
                            .push((branch.to_owned(), Expression::Name(branch.to_owned())));
                    }
                }
                ("branches", _) => {
                    return Err(problem("[copy] branches must be a list of branch names"))
                }
                _ => {
                    let source = expression(value, &format!("[copy] {name}"))?;
                    let plain = matches!(&source, Expression::Name(_))
                        || matches!(&source, Expression::Call(called, _) if called == "branch");
                    if !plain {
                        return Err(problem(format!("[copy] {name} must name a branch, or use branch(...); a computed column belongs under [column]")));
                    }
                    recipe.copy.push((name.clone(), source));
                }
            }
        }
        for (name, value) in table(&root, "column")?.into_iter().flatten() {
            let place = format!("[column] {name}");
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
        for (name, _) in recipe.copy.iter().chain(&recipe.columns) {
            if name.is_empty() || ["sample", "entry", "file"].contains(&name.as_str()) {
                return Err(problem(format!("A column cannot be named \"{name}\"")));
            }
        }
        let mut names: Vec<&String> = recipe
            .copy
            .iter()
            .chain(&recipe.columns)
            .map(|(name, _)| name)
            .collect();
        names.sort();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(problem(format!("Two columns are named {}", pair[0])));
        }
        Ok(recipe)
    }
}
