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
//! branches = ["Muon_pt", "MET", "genWeight"]
//! ht = "sum(Jet_pt)"
//! rho = "branch(sample.rho)"
//!
//! [totals]
//! sumw = "genWeight"
//!
//! [derived]
//! met_over_ht = "MET / ht"
//! weight = "genWeight / sample.sumw"
//! ```
//!
//! Every table, key and function is described in this crate's README.

use crate::check::{Checks, Sort};
use crate::parse::{parse, parsed, Expression};
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

/// One line of `[totals]`: a sum over every event read before the cuts, or over another `tree`'s entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Total {
    pub name: String,
    pub value: Expression,
    pub tree: Option<String>,
}

/// A parsed recipe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipe {
    pub title: Option<String>,
    /// The rows of one chunk of the file's arrays, as `chunk_rows` gives it (FORMAT.md §7).
    pub chunk_rows: Option<usize>,
    /// The names `[define]` gives expressions; none is stored.
    pub defines: BTreeMap<String, Expression>,
    /// The cuts in the order written, which is the order of the cutflow.
    pub keep: Vec<Cut>,
    /// The stored columns in the order written, each with the expression that makes it.
    pub columns: Vec<(String, Expression)>,
    /// The number kind a column is stored as, where it is not the kind it is computed in.
    pub stored: BTreeMap<String, crate::Kind>,
    /// The totals in the order written.
    pub totals: Vec<Total>,
    /// The derived columns in the order written, each with its expression as written.
    pub derived: Vec<(String, String)>,
    /// The defined names in the order written, each with how many levels deep it is (README "How deep").
    pub depths: Vec<(String, usize)>,
}

/// The rows of one chunk of a file's arrays when the recipe gives no `chunk_rows` (FORMAT.md §7).
pub const CHUNK_ROWS: usize = 65_536;

/// The fewest and the most rows a chunk may have (FORMAT.md §3).
pub const CHUNK_BOUNDS: (usize, usize) = (1_024, 16_777_216);

/// The names every row has (its sample, entry and file), which no column may have.
pub const RESERVED: [&str; 3] = ["sample", "entry", "file"];

/// The metadata a press gives every sample beside the keys of samples.toml, which no expression sees.
pub const ADDED: [&str; 3] = ["events_read", "events_total", "events_missing"];

/// Whether text is one plain name, as an expression would read it.
pub fn plain(text: &str) -> bool {
    matches!(parse(text), Ok(Expression::Name(name)) if name == text)
}

/// Add every name an expression mentions: branches, defined names, and any text (`branch('…')`'s).
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

/// Add what a `branch(…)` not given text may name: its sample fields to `fields`, its text and names to `texts`.
pub fn branch_names(expression: &Expression, fields: &mut BTreeSet<String>, texts: &mut BTreeSet<String>) {
    match expression {
        Expression::Call(function, arguments) if function == "branch" && named(arguments).is_none() => {
            for argument in arguments {
                fields_of(argument, fields);
                gather(argument, texts, true, false);
            }
        }
        _ => expression.children().for_each(|child| branch_names(child, fields, texts)),
    }
}

/// The text a call's only value is, as `branch('Jet.pt')` names its branch.
fn named(arguments: &[Expression]) -> Option<&String> {
    match arguments {
        [Expression::Constant(crate::Constant::Text(held))] => Some(held),
        _ => None,
    }
}

/// Add the bare names an expression holds; with `text` the text it holds too, and with
/// `branches` the text each `branch` names.
fn gather(expression: &Expression, found: &mut BTreeSet<String>, text: bool, branches: bool) {
    let held = match expression {
        Expression::Constant(crate::Constant::Text(held)) if text => Some(held),
        Expression::Name(name) => Some(name),
        Expression::Call(function, arguments) if branches && function == "branch" => named(arguments),
        _ => None,
    };
    match held {
        Some(held) => drop(found.insert(held.clone())),
        None => expression.children().for_each(|child| gather(child, found, text, branches)),
    }
}

/// Add the sample fields an expression reads (`sample.KEY`).
pub fn fields_of(expression: &Expression, found: &mut BTreeSet<String>) {
    match expression {
        Expression::Sample(field) => drop(found.insert(field.clone())),
        _ => expression.children().for_each(|child| fields_of(child, found)),
    }
}

/// Whether an expression names a branch by anything but text, as `branch(sample.KEY)`.
fn by_sample(expression: &Expression) -> bool {
    matches!(expression, Expression::Call(function, arguments) if function == "branch" && named(arguments).is_none())
        || expression.children().any(by_sample)
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

/// The keys a recipe has at its top: these and no others.
const TOP: [&str; 9] = ["format", "version", "title", "chunk_rows", "define", "keep", "columns", "totals", "derived"];

/// The tables every recipe writes, even empty, in their order.
pub const TABLES: [&str; 5] = ["define", "keep", "columns", "totals", "derived"];

/// `tables` in brackets, the last joined by `word`: "[define], [keep] and [totals]".
fn listed(tables: &[&str], word: &str) -> String {
    let named: Vec<String> = tables.iter().map(|table| format!("[{table}]")).collect();
    match named.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} {word} {last}", rest.join(", ")),
        _ => named.concat(),
    }
}

/// Why a recipe that leaves out the tables `missing` is refused, and what to add.
fn lacks(missing: &[&str]) -> String {
    let add = match missing {
        [one] => format!("Add the line [{one}]."),
        _ => format!("Add {}, each on a line of its own.", listed(missing, "and")),
    };
    format!("A recipe has no {} table: every recipe has its five tables, {}, each written even when it is empty. {add}", listed(missing, "or"), listed(&TABLES, "and"))
}

/// A recipe as it is read: every problem found so far, and what each name read gives.
struct Reading {
    said: Vec<String>,
    /// The tables of the five it leaves out.
    lacking: Vec<&'static str>,
    /// What each defined name gives, as far as its text tells.
    sorts: BTreeMap<String, Sort>,
    /// How many levels deep each name is, a defined or derived name as deep as its expression.
    depths: BTreeMap<String, usize>,
}

impl Reading {
    /// Say a problem at a place.
    fn say(&mut self, place: &str, said: impl std::fmt::Display) {
        self.said.push(format!("{place}: {said}"));
    }

    /// The expression a TOML value holds, read and checked, with what it gives and how deep it is;
    /// none, said, when it is not one. `place` names where it sits.
    fn expression(&mut self, value: &Value, place: &str) -> Option<(Expression, Sort, usize)> {
        let Some(text) = value.as_str() else {
            self.said.push(format!("{place} must be an expression in quotes"));
            return None;
        };
        let depths = &self.depths;
        let (expression, depth) = match parsed(text, &|name| depths.get(name).copied().unwrap_or(1)) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.say(place, error.0);
                return None;
            }
        };
        let mut checks = Checks { sorts: &self.sorts, said: Vec::new() };
        let sort = checks.sort(&expression);
        for said in checks.said {
            self.say(place, said);
        }
        Some((expression, sort, depth))
    }

    /// The table under `key`, or none when it is absent (or, said, not a table).
    fn table<'a>(&mut self, root: &'a Table, key: &str) -> Option<&'a Table> {
        match root.get(key) {
            None => None,
            Some(Value::Table(table)) => Some(table),
            Some(_) => {
                self.said.push(format!("[{key}] must be a table"));
                None
            }
        }
    }

    /// Say each key of `found` that `allowed` does not hold.
    fn only(&mut self, found: &Table, allowed: &[&str], place: &str) {
        for key in found.keys().filter(|key| !allowed.contains(&key.as_str())) {
            self.said.push(crate::unknown(place, key, allowed));
        }
    }
}

impl Recipe {
    /// Whether an expression is a branch alone: a name the recipe does not define, or `branch(…)`.
    pub fn alone(&self, expression: &Expression) -> bool {
        match expression {
            Expression::Name(name) => !self.defines.contains_key(name),
            Expression::Call(function, _) => function == "branch",
            _ => false,
        }
    }

    /// Whether a `[columns]` column keeps its NaNs' bits, a branch alone with no `stored` kind (FORMAT.md §4).
    pub fn keeps_bits(&self, column: &str, expression: &Expression) -> bool {
        self.alone(expression) && !self.stored.contains_key(column)
    }

    /// The rows of one chunk of the file's arrays: `chunk_rows`, or `CHUNK_ROWS`.
    pub fn chunk_rows(&self) -> usize {
        self.chunk_rows.unwrap_or(CHUNK_ROWS)
    }

    /// Read a recipe from its TOML text, refusing it with every problem it has, one a line.
    pub fn read(text: &str) -> Result<Self> {
        let root: Table = text.parse().map_err(|error: toml::de::Error| problem(unparsed(text, &error)))?;
        let mut reading = Reading { said: Vec::new(), lacking: Vec::new(), sorts: BTreeMap::new(), depths: BTreeMap::new() };
        let recipe = reading.recipe(&root);
        match reading.said.is_empty() {
            true => Ok(recipe),
            false => Err(crate::Error(reading.said.join("\n"), None, crate::Cause::Lacking(reading.lacking))),
        }
    }

    /// Read one expression as a line of this recipe's `[columns]`, refused with every problem it has.
    pub fn expression(&self, text: &str) -> Result<Expression> {
        // What each defined name gives, in the order written: each uses only those above it.
        let mut sorts = BTreeMap::new();
        for (name, _) in &self.depths {
            let sort = Checks { sorts: &sorts, said: Vec::new() }.sort(&self.defines[name]);
            sorts.insert(name.clone(), sort);
        }
        let depths: BTreeMap<&str, usize> = self.depths.iter().map(|(name, depth)| (name.as_str(), *depth)).collect();
        let (expression, _) = parsed(text, &|name| depths.get(name).copied().unwrap_or(1))?;
        let mut checks = Checks { sorts: &sorts, said: Vec::new() };
        checks.sort(&expression);
        match checks.said.is_empty() {
            true => Ok(expression),
            false => Err(problem(checks.said.join("\n"))),
        }
    }
}

impl Reading {
    /// The recipe `root` holds, with every problem it has said.
    fn recipe(&mut self, root: &Table) -> Recipe {
        self.only(root, &TOP, "A recipe");
        if root.get("format").and_then(Value::as_str) != Some("rvr-recipe") || root.get("version").and_then(Value::as_integer) != Some(1) {
            self.said.push("A recipe starts with format = \"rvr-recipe\" and version = 1".into());
        }
        self.lacking = TABLES.into_iter().filter(|table| !root.contains_key(*table)).collect();
        if !self.lacking.is_empty() {
            self.said.push(lacks(&self.lacking));
        }
        let mut recipe = Recipe::default();
        match root.get("title") {
            None => {}
            Some(Value::String(title)) => recipe.title = Some(title.clone()),
            Some(_) => self.said.push("title must be text, as title = \"my study\"".into()),
        }
        let (fewest, most) = (CHUNK_BOUNDS.0 as i64, CHUNK_BOUNDS.1 as i64);
        match root.get("chunk_rows") {
            None => {}
            Some(Value::Integer(rows)) if (fewest..=most).contains(rows) => recipe.chunk_rows = Some(*rows as usize),
            Some(_) => self.said.push("chunk_rows must be a whole number from 1,024 to 16,777,216, the rows of one chunk of the file's arrays (FORMAT.md §7): leave it out for 65,536".into()),
        }
        self.defines(root, &mut recipe);
        self.keep(root, &mut recipe);
        let before = self.said.len();
        self.columns(root, &mut recipe);
        if recipe.columns.is_empty() && self.said.len() == before {
            self.said.push("The recipe stores nothing: under [columns], list branches (branches = [\"MET_pt\"]) or name computed columns (ht = \"sum(Jet_pt)\")".into());
        }
        self.totals(root, &mut recipe);
        // A total is known once every event is read: only a derived column may use one.
        let total = |expression: &Expression| {
            let mut fields = BTreeSet::new();
            fields_of(expression, &mut fields);
            fields.into_iter().find(|field| recipe.totals.iter().any(|t| &t.name == field))
        };
        let places: Vec<(String, &Expression)> = recipe.defines.iter().map(|(name, e)| (format!("[define] {name}"), e))
            .chain(recipe.keep.iter().flat_map(|cut| cut.expressions().map(move |e| (format!("[keep] {}", cut.name), e))))
            .chain(recipe.columns.iter().map(|(name, e)| (format!("[columns] {name}"), e)))
            .chain(recipe.totals.iter().map(|t| (format!("[totals] {}", t.name), &t.value)))
            .collect();
        for (place, expression) in places {
            if let Some(field) = total(expression) {
                self.said.push(format!("{place} uses sample.{field}, a total of [totals], known only once every event is read: use it under [derived]"));
            }
        }
        self.derived(root, &mut recipe);
        recipe
    }

    /// `[define]`: names for expressions, each using only the names above it.
    fn defines(&mut self, root: &Table, recipe: &mut Recipe) {
        let Some(defines) = self.table(root, "define") else { return };
        let every: BTreeSet<&String> = defines.keys().collect();
        let mut above: BTreeSet<&String> = BTreeSet::new();
        for (name, value) in defines {
            let place = format!("[define] {name}");
            if !plain(name) || name == "sample" {
                self.say(&place, "a defined name is one plain name an expression can use, such as good_muon");
            }
            if let Some((value, sort, depth)) = self.expression(value, &place) {
                let mut used = BTreeSet::new();
                uses(&value, &mut used);
                for later in used.iter().filter(|used| every.contains(used) && !above.contains(used)) {
                    let itself = if later == name { ", itself" } else { ", which is defined below it" };
                    self.said.push(format!("{place} uses {later}{itself}: a defined name may use only the names defined above it"));
                }
                self.sorts.insert(name.clone(), sort);
                self.depths.insert(name.clone(), depth);
                recipe.depths.push((name.clone(), depth));
                recipe.defines.insert(name.clone(), value);
            }
            above.insert(name);
        }
    }

    /// `[keep]`: the cuts, in order, each a condition or `{ if, apply }`.
    fn keep(&mut self, root: &Table, recipe: &mut Recipe) {
        let Some(keep) = self.table(root, "keep") else { return };
        for (name, value) in keep {
            let place = format!("[keep] {name}");
            if !plain(name) {
                self.say(&place, "a cut is named with one plain name, such as two_muons");
            }
            let (when, pass) = match value {
                Value::Table(cut) => {
                    self.only(cut, &["if", "apply"], &place);
                    let when = cut.get("if").map(|when| self.condition(when, &format!("{place} if")));
                    let pass = match cut.get("apply") {
                        Some(pass) => self.condition(pass, &place),
                        None => {
                            self.said.push(format!("{place} needs apply = \"…\": the cut, for the samples its if is true for"));
                            None
                        }
                    };
                    match when {
                        Some(None) => (None, None),
                        when => (when.flatten(), pass),
                    }
                }
                Value::String(_) => (None, self.condition(value, &place)),
                _ => {
                    self.said.push(format!("{place} must be a condition in quotes, or {{ if = \"…\", apply = \"…\" }}"));
                    (None, None)
                }
            };
            if let Some(pass) = pass {
                recipe.keep.push(Cut { name: name.clone(), when, pass });
            }
        }
    }

    /// A cut's condition: an expression that gives a true-or-false value.
    fn condition(&mut self, value: &Value, place: &str) -> Option<Expression> {
        let (expression, sort, _) = self.expression(value, place)?;
        if let Some(said) = crate::check::untrue(sort, "A cut's condition") {
            self.say(place, said);
        }
        Some(expression)
    }

    /// `[columns]`: the branches stored as they are, and the columns computed.
    fn columns(&mut self, root: &Table, recipe: &mut Recipe) {
        let Some(columns) = self.table(root, "columns") else { return };
        let mut named: Vec<String> = Vec::new();
        for (name, value) in columns {
            if name == "branches" {
                let Some(list) = value.as_array() else {
                    self.said.push("[columns] branches must be a list of branch names, as branches = [\"MET_pt\"]".into());
                    continue;
                };
                for branch in list {
                    let Some(branch) = branch.as_str().filter(|branch| !branch.is_empty()) else {
                        self.said.push(format!("[columns] branches must be a list of branch names, and {branch} is not one"));
                        continue;
                    };
                    if RESERVED.contains(&branch) {
                        self.said.push(format!("[columns] branches lists {branch}, a name every row has already ({}): store the branch under another name, as {branch}_branch = \"branch('{branch}')\"", RESERVED.join(", ")));
                    }
                    if recipe.defines.contains_key(branch) {
                        self.said.push(format!("[columns] branches lists {branch}, a name [define] gives an expression: list branches only, and store the expression as a column of its own, such as {branch}_stored = \"{branch}\""));
                    }
                    named.push(branch.to_owned());
                    recipe.columns.push((branch.to_owned(), Expression::Name(branch.to_owned())));
                }
                continue;
            }
            let place = format!("[columns] {name}");
            named.push(name.clone());
            if !plain(name) || RESERVED.contains(&name.as_str()) {
                self.say(&place, "a column is named with one plain name, such as ht, and not sample, entry or file");
            }
            let value = match value {
                Value::Table(column) => {
                    self.only(column, &["value", "stored"], &place);
                    if let Some(kind) = column.get("stored") {
                        match kind.as_str().map(crate::Kind::named) {
                            Some(Ok(kind)) => drop(recipe.stored.insert(name.clone(), kind)),
                            Some(Err(e)) => self.say(&format!("{place} stored"), e.0),
                            None => self.said.push(format!("{place} stored must be a number kind in quotes, such as \"i4\"")),
                        }
                    }
                    match column.get("value") {
                        Some(value) => value,
                        None => {
                            self.said.push(format!("{place} needs value = \"…\": the expression it stores"));
                            continue;
                        }
                    }
                }
                value => value,
            };
            if let Some((expression, sort, _)) = self.expression(value, &place) {
                if sort == Sort::Text {
                    self.say(&place, "a column holds numbers or true-or-false values, not text");
                }
                recipe.columns.push((name.clone(), expression));
            }
        }
        let mut sorted = named.clone();
        sorted.sort();
        sorted.dedup();
        for name in sorted.iter().filter(|name| named.iter().filter(|n| n == name).count() > 1) {
            self.said.push(format!("Two columns are named {name}: give one another name"));
        }
    }

    /// `[totals]`: sums over every event read, or over another tree of each file.
    fn totals(&mut self, root: &Table, recipe: &mut Recipe) {
        let Some(totals) = self.table(root, "totals") else { return };
        for (name, value) in totals {
            let place = format!("[totals] {name}");
            if !plain(name) || RESERVED.contains(&name.as_str()) || ADDED.contains(&name.as_str()) {
                self.say(&place, format!("a total is named with one plain name, such as sumw, and not {}", RESERVED.iter().chain(&ADDED).copied().collect::<Vec<_>>().join(", ")));
            }
            if recipe.columns.iter().any(|(n, _)| n == name) || recipe.defines.contains_key(name) {
                self.say(&place, format!("{name} is already the name of a column or a line of [define]; give the total another"));
            }
            let (value, tree) = match value {
                Value::Table(line) => {
                    self.only(line, &["value", "tree"], &place);
                    let tree = line.get("tree").and_then(|tree| match tree.as_str().filter(|tree| !tree.is_empty()) {
                        Some(tree) => Some(tree.to_owned()),
                        None => {
                            self.said.push(format!("{place} tree must name a tree of each file, such as \"Runs\""));
                            None
                        }
                    });
                    match line.get("value") {
                        Some(value) => (value, tree),
                        None => {
                            self.said.push(format!("{place} needs a value: what is summed"));
                            continue;
                        }
                    }
                }
                value => (value, None),
            };
            let Some((value, sort, _)) = self.expression(value, &place) else { continue };
            if sort == Sort::Text {
                self.say(&place, "a total sums numbers, not text");
            }
            let mut used = BTreeSet::new();
            uses(&value, &mut used);
            if let (Some(tree), Some(defined)) = (&tree, used.iter().find(|used| recipe.defines.contains_key(*used))) {
                self.said.push(format!("{place} reads the tree {tree}, and {defined} is a line of [define], over the events read: name that tree's branches"));
            }
            recipe.totals.push(Total { name: name.clone(), value, tree });
        }
    }

    /// `[derived]`: columns kept as expressions over what the file holds, each using only the
    /// stored columns and the derived columns above it.
    fn derived(&mut self, root: &Table, recipe: &mut Recipe) {
        let Some(derived) = self.table(root, "derived") else { return };
        // Defined names are not what a derived column reads: each name is one level, a derived
        // column above as deep as its expression.
        self.sorts.clear();
        self.depths.clear();
        let mut held: BTreeSet<String> = recipe.columns.iter().map(|(name, _)| name.clone()).collect();
        let every: BTreeSet<&String> = derived.keys().collect();
        for (name, value) in derived {
            let place = format!("[derived] {name}");
            if !plain(name) || RESERVED.contains(&name.as_str()) {
                self.say(&place, "a derived column is named with one plain name, such as ht_over_met");
            }
            if held.contains(name) || recipe.defines.contains_key(name) || recipe.totals.iter().any(|t| &t.name == name) {
                self.say(&place, format!("{name} is already the name of a column, a total or a line of [define]; give the derived column another"));
            }
            let Some((parsed, _, depth)) = self.expression(value, &place) else {
                held.insert(name.clone());
                continue;
            };
            self.depths.insert(name.clone(), depth);
            if by_sample(&parsed) {
                self.say(&place, "branch(sample.KEY) names another column for each sample, which a derived column cannot be checked against before the press: name the column, as branch('Jet.pt')");
            }
            let mut used = BTreeSet::new();
            reads(&parsed, &mut used);
            for missing in used.iter().filter(|used| !held.contains(*used)) {
                let why = if recipe.defines.contains_key(missing) {
                    format!("{missing} is a line of [define], which the file does not keep")
                } else if missing == name {
                    format!("{missing} is this derived column itself")
                } else if every.contains(missing) {
                    format!("{missing} is a derived column below it")
                } else {
                    format!("the file does not store {missing}")
                };
                self.said.push(format!("{place} uses {missing}, but {why}: a derived column uses only the columns the file stores, under [columns], and the derived columns above it"));
            }
            held.insert(name.clone());
            recipe.derived.push((name.clone(), value.as_str().unwrap_or_default().to_owned()));
        }
    }
}
