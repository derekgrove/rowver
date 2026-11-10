//! The `derived` section: the columns a file keeps as the expressions that compute them, each
//! with the checksums and facts of its values. The records are checked with the header; the
//! values are worked out here a run of rows at a time, each run within one chunk of the file and
//! one file read, from the chunks of the columns they use, and measured chunk by chunk.

use super::{exactly, list, nonempty, text, Held, Reader, MEND};
use crate::{invalid, Result};
use rvr_recipe::{parse, per_event, Array, Columns, Constant, Expression, Kind, Recipe, Scope, Values};
use serde_json::{Map, Value};
use std::cell::RefCell;
use crate::chunks::Rows;
use crate::write::Measuring;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// Check the derived section against the file's own header (docs/FORMAT.md §10): one derived
/// column or more, each record's fields, a name no stored, sample or other derived column has,
/// an expression, a kind, and the forms of its checksums and facts.
pub(super) fn validate(derived: &Value, data: &Value) -> Result<()> {
    exactly(derived, "derived", &["columns"], &[])?;
    let records = list(derived, "derived", "columns")?;
    if records.is_empty() {
        return Err(invalid("derived.columns is empty, and a file with no derived column has no derived key (docs/FORMAT.md §10): take it out"));
    }
    let samples = list(data, "data", "samples")?.len();
    let mut names: HashSet<&str> = HashSet::new();
    for column in list(data, "data", "cols")?.iter().chain(list(data, "data", "sample_cols")?) {
        names.insert(column["name"].as_str().unwrap_or_default());
    }
    for (index, record) in records.iter().enumerate() {
        let name = nonempty(record, &format!("derived.columns[{index}]"), "name")?;
        let place = format!("derived.columns[{name:?}]");
        if rvr_recipe::taken(name) || !names.insert(name) {
            return Err(invalid(format!("{place}: a derived column's name is one no stored, sample or other derived column has, nor {} (docs/FORMAT.md §10): name it otherwise in the recipe, and press again", rvr_recipe::given())));
        }
        if text(record, &place, "expression")?.trim().is_empty() {
            return Err(invalid(format!("{place}.expression is blank, and a derived column is kept as the expression that computes it (docs/FORMAT.md §10): {MEND}")));
        }
        super::columns::record_of(record, &place, &["name", "expression"], false, samples)?;
    }
    Ok(())
}

/// The metadata fields an expression can read: numbers, text, and truth values.
pub fn fields(metadata: &Map<String, Value>) -> BTreeMap<String, Constant> {
    let read = |value: &Value| match value {
        Value::Bool(truth) => Some(Constant::Truth(*truth)),
        Value::String(text) => Some(Constant::Text(text.clone())),
        Value::Number(number) => match number.as_i64() {
            Some(whole) if crate::json::whole(number) => Some(Constant::Whole(whole)),
            _ => number.as_f64().map(Constant::Float),
        },
        _ => None,
    };
    metadata.iter().filter_map(|(name, value)| Some((name.clone(), read(value)?))).collect()
}

/// `metadata` with each total of `recipe` standing in as 1, as a derived column is tried before every event is read.
pub fn standing_in(metadata: &mut BTreeMap<String, Constant>, recipe: &Recipe) {
    for total in &recipe.totals {
        metadata.entry(total.name.clone()).or_insert(Constant::Float(1.0));
    }
}

/// A derived column's values over the `rows` rows of `columns`, worked out a run at a time; a data error names its row.
pub fn evaluate(expression: &Expression, columns: &dyn Columns, metadata: &BTreeMap<String, Constant>, rows: usize) -> rvr_recipe::Result<Array> {
    let defines = BTreeMap::new();
    let mut work = |run: &dyn Columns, from: usize, to: usize| {
        Scope::new(run, metadata, &defines).evaluate(expression).and_then(|value| per_event(value, to - from))
    };
    rvr_recipe::over_runs(columns, rows, &mut work).map_err(|(e, _)| e).and_then(Array::joined).map(Array::settled)
}

/// Add one run's values, of the kind and shape of those before, to a column over every row.
fn add(whole: &mut Option<Held>, array: Array) {
    let (values, offsets) = match array {
        Array::Event(values) => (values, None),
        Array::Object { offsets, values } => (values, Some(offsets)),
        Array::Constant(_) => unreachable!("per_event gives one value per row"),
    };
    let held = whole.get_or_insert_with(|| Held {
        kind: values.kind.name().into(),
        bytes: Vec::new(),
        offsets: offsets.as_ref().map(|_| Arc::new(vec![0])),
    });
    let bytes = values.to_bytes();
    match (held.offsets.as_mut().map(Arc::make_mut), &offsets) {
        // A run's lists need not begin at its first value: only the values they hold are kept.
        (Some(all), Some(run)) => {
            let (from, to) = (run.first().copied().unwrap_or(0), run.last().copied().unwrap_or(0));
            let w = values.kind.width();
            held.bytes.extend_from_slice(&bytes[from * w..to * w]);
            let base = all[all.len() - 1];
            all.extend(run.iter().skip(1).map(|offset| base + offset - from));
        }
        _ => held.bytes.extend(bytes),
    }
}

/// One run of rows a derived column is worked out over: within one chunk (none: no rows) and one file.
struct Span<'h> {
    chunk: Option<usize>,
    first: usize,
    last: usize,
    sample: &'h str,
    path: &'h str,
}

/// The runs of a file, in row order: each file's rows cut at the chunks' edges (FORMAT.md §7). Over
/// no rows (`hollow`, an empty file), one run of no rows a file with rows; for a file of no rows,
/// one run of no rows of its first sample, which still says what kind of column each is. Refused
/// when its files hold rows past its chunks', which its samples' rows make.
fn spans<'h>(header: &'h Value, chunks: &[Rows], hollow: bool) -> Result<Vec<Span<'h>>> {
    let (mut out, mut first, mut k) = (Vec::new(), 0, 0);
    for read in header["data"]["files"].as_array().into_iter().flatten() {
        let rows = super::whole_count(&read["rows"]).unwrap_or(0) as usize;
        let (sample, path) = (read["sample"].as_str().unwrap_or_default(), read["path"].as_str().unwrap_or_default());
        if hollow {
            if rows > 0 {
                out.push(Span { chunk: None, first: 0, last: 0, sample, path });
            }
            continue;
        }
        let (mut at, end) = (first, first + rows);
        while at < end {
            while k < chunks.len() && chunks[k].2 <= at {
                k += 1;
            }
            let Some(chunk) = chunks.get(k) else {
                return Err(invalid(format!("the header's files hold more rows than its samples: data.files' rows go past the {} its samples' rows give (docs/FORMAT.md §3): {MEND}", chunks.last().map_or(0, |c| c.2))));
            };
            let to = chunk.2.min(end);
            out.push(Span { chunk: Some(k), first: at, last: to, sample, path });
            at = to;
        }
        first = end;
    }
    if out.is_empty() {
        let sample = header["data"]["samples"][0]["name"].as_str().unwrap_or_default();
        out.push(Span { chunk: None, first: 0, last: 0, sample, path: "" });
    }
    Ok(out)
}

/// What an expression of a run sees of its sample: the sample's metadata as a press gives it, and the file.
fn metadata(header: &Value, sample: &str, path: &str) -> BTreeMap<String, Constant> {
    let mut held = header["made"]["files"][sample]["metadata"].as_object().map(fields).unwrap_or_default();
    held.retain(|key, _| !rvr_recipe::ADDED.contains(&key.as_str()));
    held.insert("file".into(), Constant::Text(path.into()));
    held
}

/// The columns of a file an expression can name: its stored columns, a chunk at a time, and its
/// derived columns, worked out a run at a time.
struct Store<'a> {
    file: &'a Reader,
    derived: &'a [(String, String)],
    /// Each stored column's (and `sample`'s and `entry`'s) chunk read last, by name: the chunk, and its rows' values.
    chunk: RefCell<BTreeMap<String, (usize, Rc<Held>)>>,
    /// Each derived column's expression, read once.
    parsed: RefCell<BTreeMap<String, Rc<Expression>>>,
    /// The derived columns being worked out, which none of them may use.
    working: RefCell<BTreeSet<String>>,
    /// How a column an expression asked for failed, said as it is rather than as the expression's.
    failed: RefCell<Option<crate::Error>>,
    chunks: Arc<Vec<Rows>>,
}

impl<'a> Store<'a> {
    /// The store of a file's columns; with no chunks for work over no rows (`hollow`), which reads
    /// none, and for an empty file, whose chunks are never made.
    fn new(file: &'a Reader, derived: &'a [(String, String)], hollow: bool) -> Result<Self> {
        let (chunk, parsed, working, failed) = Default::default();
        let chunks = if hollow || file.empty() { Arc::default() } else { file.chunks()? };
        Ok(Store { file, derived, chunk, parsed, working, failed, chunks })
    }

    /// A stored column of no rows, of the kind its record gives.
    fn none_of(record: &Value) -> Held {
        let jagged = record["kind"] == "jagged";
        let kind = if jagged { &record["content"]["kind"] } else { &record["kind"] };
        Held { kind: kind.as_str().unwrap_or_default().into(), bytes: Vec::new(), offsets: jagged.then(|| Arc::new(vec![0])) }
    }

    /// What a refusal calls derived column `name`: its place in the header, or for one being
    /// added, its name.
    fn place(&self, name: &str) -> String {
        match listed(self.file.header()).iter().any(|(n, _)| n == name) {
            true => format!("derived.columns[{name:?}]"),
            false => format!("derived column {name}"),
        }
    }

    /// The rows of chunk `k` of the stored column `name` (or `sample` or `entry`, with no record),
    /// read once while that chunk's runs are worked on.
    fn chunk(&self, name: &str, record: Option<&Value>, k: usize) -> Result<Rc<Held>> {
        if let Some((_, held)) = self.chunk.borrow().get(name).filter(|(at, _)| *at == k) {
            return Ok(held.clone());
        }
        let (first, end) = (self.chunks[k].1, self.chunks[k].2);
        let held = Rc::new(match (record, name) {
            (Some(record), _) => self.file.stored_rows(name, record, first, end)?,
            (None, "sample") => Held { kind: "i4".into(), bytes: self.file.sample_slice(first, end)?.iter().flat_map(|v| v.to_le_bytes()).collect(), offsets: None },
            (None, "entry") => Held { kind: "i8".into(), bytes: self.file.entry_slice(first, end)?.iter().flat_map(|v| v.to_le_bytes()).collect(), offsets: None },
            (None, name) => return Err(invalid(format!("a bug in rowver: the rows of {name} were asked for, and the file holds no such column: please report it"))),
        });
        self.chunk.borrow_mut().insert(name.to_owned(), (k, held.clone()));
        Ok(held)
    }

    /// Derived column `name`'s expression, read once.
    fn parsed(&self, name: &str, expression: &str) -> Result<Rc<Expression>> {
        if let Some(parsed) = self.parsed.borrow().get(name) {
            return Ok(parsed.clone());
        }
        let parsed = Rc::new(parse(expression).map_err(|e| invalid(format!("{}: {}", self.place(name), e.0)))?);
        self.parsed.borrow_mut().insert(name.to_owned(), parsed.clone());
        Ok(parsed)
    }
}

/// One run's rows of a file's columns, as an expression reads them: each read or worked out once.
struct Run<'s, 'a> {
    store: &'s Store<'a>,
    chunk: Option<usize>,
    first: usize,
    last: usize,
    sample: String,
    metadata: BTreeMap<String, Constant>,
    held: RefCell<BTreeMap<String, Option<Array>>>,
}

impl Columns for Run<'_, '_> {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        if let Some(held) = self.held.borrow().get(name) {
            return Ok(held.clone());
        }
        let array = self.read(name)?;
        self.held.borrow_mut().insert(name.to_owned(), array.clone());
        Ok(array)
    }

    fn has(&self, name: &str) -> bool {
        self.store.file.record(name).is_some() || self.store.derived.iter().any(|(n, _)| n == name)
    }
}

impl Run<'_, '_> {
    /// A failure of a column the expression asked for, kept to be said as it is.
    fn failed(&self, e: crate::Error) -> rvr_recipe::Error {
        let said = rvr_recipe::problem(e.to_string());
        self.store.failed.borrow_mut().get_or_insert(e);
        said
    }

    /// The column's rows of the run: a stored one's, from its chunk; a derived one's, worked out.
    fn read(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        if let Some(record) = self.store.file.record(name) {
            let (held, at) = match self.chunk {
                None => (Rc::new(Store::none_of(record)), 0),
                Some(k) => (self.store.chunk(name, Some(record), k).map_err(|e| self.failed(e))?, self.store.chunks[k].1),
            };
            return array_of(&held, self.first - at, self.last - at).map(Some);
        }
        match self.store.derived.iter().find(|(n, _)| n == name) {
            Some((_, expression)) => self.derived(name, expression).map(Some).map_err(|e| self.failed(e)),
            None => Ok(None),
        }
    }

    /// Derived column `name` worked out over the run's rows; a column it uses that failed is said
    /// as it failed, and its own expression failing, in a column a newer rvr recorded, says to update.
    fn derived(&self, name: &str, expression: &str) -> Result<Array> {
        if let Some(Some(held)) = self.held.borrow().get(name) {
            return Ok(held.clone());
        }
        let place = self.store.place(name);
        if !self.store.working.borrow_mut().insert(name.to_owned()) {
            return Err(invalid(format!("{place} uses itself, directly or through other derived columns. {}", super::DERIVED_MEND)));
        }
        let worked = self.store.parsed(name, expression).and_then(|parsed| {
            evaluate(&parsed, self, &self.metadata, self.last - self.first).map_err(|e| {
                if let Some(failed) = self.store.failed.borrow_mut().take() {
                    return failed;
                }
                let said = match e.1 {
                    Some(at) => invalid(format!("sample {}: {place}, row {}: {}", self.sample, self.first + at, e.0)),
                    None => invalid(format!("sample {}: {place}: {}", self.sample, e.0)),
                };
                let header = self.store.file.header();
                let recorded = listed(header).iter().any(|(n, x)| n == name && x == expression);
                match unread(header).filter(|_| recorded) {
                    Some(hint) => invalid(format!("{said}. {hint}")),
                    None => said,
                }
            })
        });
        self.store.working.borrow_mut().remove(name);
        let worked = worked?;
        self.held.borrow_mut().insert(name.to_owned(), Some(worked.clone()));
        Ok(worked)
    }
}

/// Rows `from..to` of a column held, as an expression reads them.
fn array_of(held: &Held, from: usize, to: usize) -> rvr_recipe::Result<Array> {
    let kind = Kind::named(&held.kind)?;
    let (bytes, offsets) = held.part(from, to).map_err(|e| rvr_recipe::problem(e.to_string()))?;
    Ok(match offsets {
        None => Array::Event(Values::from_bytes(kind, bytes)),
        Some(offsets) => Array::Object { offsets: Arc::new(offsets), values: Values::from_bytes(kind, bytes) },
    })
}

/// A run's values as a column measures them: their little-endian bytes, and a list's counts.
fn parts(array: &Array) -> (Vec<u8>, Option<Vec<usize>>) {
    match array {
        Array::Event(values) => (values.to_bytes(), None),
        Array::Object { offsets, values } => {
            let (from, to) = (offsets.first().copied().unwrap_or(0), offsets.last().copied().unwrap_or(0));
            let w = values.kind.width();
            (values.to_bytes()[from * w..to * w].to_vec(), Some(offsets.windows(2).map(|p| p[1] - p[0]).collect()))
        }
        Array::Constant(_) => unreachable!("per_event gives one value per row"),
    }
}

/// One derived column worked out: its record from its kind on (when measured), and its values (when kept).
type Done = (Option<Map<String, Value>>, Option<Held>);

/// One derived column being worked out: its record from its kind on (when measured), and its
/// values over every row (when kept).
struct Worked {
    measured: Option<Measuring>,
    whole: Option<Held>,
    shape: Option<(Kind, bool)>,
}

/// What is handed each run's rows `first..last` of the columns asked for.
type Each<'f> = dyn FnMut(usize, usize, Vec<Held>) -> Result<()> + 'f;

/// How the derived columns are worked out: over no rows (`hollow`), kept whole, over only the
/// rows `range`, not measured; and, for `each`, each run's rows of the columns `stored` (with
/// `sample` and `entry`) and of the derived ones.
#[derive(Default)]
struct Ask<'f> {
    hollow: bool,
    whole: bool,
    range: Option<(usize, usize)>,
    unmeasured: bool,
    stored: &'f [String],
    each: Option<&'f mut Each<'f>>,
}

/// The values of a run's array as a column holds them.
fn held_of(array: &Array) -> Held {
    let (bytes, counts) = parts(array);
    let kind = match array {
        Array::Event(values) | Array::Object { values, .. } => values.kind.name().to_owned(),
        Array::Constant(_) => unreachable!("per_event gives one value per row"),
    };
    let offsets = counts.map(|counts| std::iter::once(0).chain(counts.iter().scan(0, |at, n| { *at += n; Some(*at) })).collect::<Vec<_>>()).map(Arc::new);
    Held { kind, bytes, offsets }
}

/// The derived columns `names` (with their expressions) of the store's file, worked out run by
/// run, every one over each run: each measured (unless over no rows, or only the rows asked),
/// and kept whole (over those rows) when asked.
fn worked(store: &Store, names: &[(String, String)], mut ask: Ask) -> Result<Vec<Done>> {
    let (hollow, whole, range) = (ask.hollow, ask.whole, ask.range);
    // An empty file's rows are let go: its expressions are tried over none, so a function's name
    // is checked first, and then it is refused as empty.
    if !hollow && store.file.empty() {
        worked(store, names, Ask { hollow: true, ..Ask::default() })?;
        return Err(invalid(super::EMPTY));
    }
    let header = store.file.header();
    let samples = header["data"]["samples"].as_array().map_or(0, Vec::len);
    let mut out: Vec<Worked> = names.iter().map(|_| Worked { measured: None, whole: None, shape: None }).collect();
    let mut chunk = None;
    let measure = !hollow && range.is_none() && !ask.unmeasured;
    let (from, to) = range.unwrap_or((0, usize::MAX));
    let cut = spans(header, &store.chunks, hollow)?.into_iter().filter_map(|span| {
        let (first, last) = (span.first.max(from), span.last.min(to));
        (first < last || (span.first == span.last && range.is_none())).then_some(Span { first, last, ..span })
    });
    for span in cut {
        // The chunk before, every run of it worked out, is let go.
        if span.chunk != chunk {
            store.chunk.borrow_mut().clear();
            chunk = span.chunk;
        }
        let run = Run { store, chunk: span.chunk, first: span.first, last: span.last, sample: span.sample.into(), metadata: metadata(header, span.sample, span.path), held: RefCell::default() };
        for ((name, expression), done) in names.iter().zip(&mut out) {
            let values = run.derived(name, expression)?;
            let (kind, listed) = match &values {
                Array::Object { values, .. } => (values.kind, true),
                Array::Event(values) => (values.kind, false),
                Array::Constant(_) => unreachable!("per_event gives one value per row"),
            };
            if *done.shape.get_or_insert((kind, listed)) != (kind, listed) {
                return Err(invalid(format!("sample {}: {} is not the same kind of column for every sample: give each sample's value the same type", span.sample, store.place(name))));
            }
            if measure {
                let measuring = match &mut done.measured {
                    Some(measuring) => measuring,
                    measured => measured.insert(Measuring::new(name, kind.name(), listed, &store.chunks)?),
                };
                let (bytes, counts) = parts(&values);
                measuring.take(&bytes, counts.as_deref()).map_err(|e| invalid(format!("derived column {e}")))?;
            }
            if whole {
                add(&mut done.whole, values);
            }
        }
        // A run of no rows, in no chunk, says only what kind each column is: nothing to hand on.
        if let (Some(each), Some(k)) = (ask.each.as_mut(), span.chunk) {
            let mut held = Vec::new();
            for name in ["sample", "entry"].into_iter().chain(ask.stored.iter().map(String::as_str)) {
                let record = store.file.record(name);
                let at = store.chunks[k].1;
                held.push(store.chunk(name, record, k)?.rows(span.first - at, span.last - at)?);
            }
            for (name, expression) in names {
                held.push(held_of(&run.derived(name, expression)?));
            }
            each(span.first, span.last, held)?;
        }
    }
    out.into_iter()
        .map(|done| Ok((done.measured.map(|m| m.finish(samples)).transpose()?, done.whole)))
        .collect()
}

/// The records of `derived`: one of `before` with the same name and expression, or worked out.
pub fn records(file: &Reader, derived: &[(String, String)], before: &[Value]) -> Result<Vec<Value>> {
    let store = Store::new(file, derived, false)?;
    let kept = |name: &str, expression: &str| before.iter().find(|r| r["name"] == name && r["expression"] == expression).cloned();
    let todo: Vec<(String, String)> = derived.iter().filter(|(n, x)| kept(n, x).is_none()).cloned().collect();
    let mut measured = worked(&store, &todo, Ask::default())?.into_iter();
    let mut out = Vec::new();
    for (name, expression) in derived {
        if let Some(record) = kept(name, expression) {
            out.push(record);
            continue;
        }
        let mut record = Map::new();
        record.insert("name".into(), name.as_str().into());
        record.insert("expression".into(), expression.as_str().into());
        record.extend(measured.next().and_then(|(m, _)| m).unwrap_or_default());
        out.push(Value::Object(record));
    }
    Ok(out)
}

/// Derived columns of a file, worked out and each checked against the file's record of it.
pub fn columns(file: &Reader, names: &[String]) -> Result<Vec<Held>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let derived = listed(file.header());
    let store = Store::new(file, &derived, false)?;
    let wanted = named(&derived, names)?;
    let done = worked(&store, &wanted, Ask { whole: true, ..Ask::default() })?;
    let mut wholes = Vec::new();
    for ((name, expression), (measured, whole)) in wanted.iter().zip(done) {
        agrees(&store, name, expression, &measured.unwrap_or_default())?;
        file.checked(name, true);
        wholes.push(whole.expect("a derived column worked out whole"));
    }
    Ok(wholes)
}

/// Each derived column `names` asks for, with its expression.
fn named(derived: &[(String, String)], names: &[String]) -> Result<Vec<(String, String)>> {
    names.iter().map(|name| derived.iter().find(|(n, _)| n == name).cloned().ok_or_else(|| invalid(format!("the file has no derived column named {name}")))).collect()
}

/// Every row of a file a run at a time to `each`: sample, entry, `stored`, then the derived columns, checked first.
pub fn walk(file: &Reader, stored: &[String], each: &mut dyn FnMut(usize, usize, Vec<Held>) -> Result<()>) -> Result<()> {
    checked(file)?;
    let derived = listed(file.header());
    let store = Store::new(file, &derived, false)?;
    worked(&store, &derived, Ask { stored, each: Some(each), unmeasured: true, ..Ask::default() })?;
    Ok(())
}

/// Require derived column `name`'s record to be what working it out here gave, `again`.
fn agrees(store: &Store, name: &str, expression: &str, again: &Map<String, Value>) -> Result<()> {
    let file = store.file;
    let header = file.header();
    let record = header["derived"]["columns"].as_array().into_iter().flatten().find(|r| r["name"] == name).ok_or_else(|| invalid(format!("the file has no derived column named {name}")))?;
    let mut again = again.clone();
    again.insert("name".into(), name.into());
    again.insert("expression".into(), expression.into());
    // A jagged record's only number is its content's count.
    let numbers = match record["kind"].as_str().unwrap_or_default() {
        "jagged" => super::facts::Numbers::Whole,
        kind => super::facts::numbers(kind),
    };
    if super::facts::same(&Value::Object(again), record, numbers) {
        return Ok(());
    }
    let differs = format!("{}, worked out here, is not what the file records of it (its checksums or facts differ)", store.place(name));
    // A newer rvr's record is not to be replaced by this rowver's: it says to update too.
    if let Some(hint) = unread(header) {
        let other = match header["made"]["toolchain"]["program"].as_str() {
            Some("rvr") => "a newer rvr",
            _ => "the program that made it",
        };
        return Err(invalid(format!("{differs}: its header was changed, or {other} works the expression out otherwise. {hint}")));
    }
    // rvr derive changes only a file whose derived columns are what it records: a press of the study
    // the file carries (with the derived columns rvr derive added to it) makes it again.
    let path = crate::shell(&file.path().display().to_string());
    Err(invalid(format!("{differs}: its header was changed, or the rowver that wrote it works the expression out differently. rvr reproduce {path} presses the study it carries again, as this rowver works it out, and compares the two (with --study FOLDER, the folder its lock's relative paths are from).")))
}

/// Rows `from..to` of derived columns `names` of a file, each checked against its record first (once an open file).
pub fn slice(file: &Reader, names: &[String], from: usize, to: usize) -> Result<Vec<Held>> {
    file.within(&names.join(", "), from, to)?;
    let derived = listed(file.header());
    let store = Store::new(file, &derived, from >= to)?;
    let wanted = named(&derived, names)?;
    // No rows: worked out over none, which still says what kind of column each is.
    if from >= to {
        let done = worked(&store, &wanted, Ask { hollow: true, whole: true, ..Ask::default() })?;
        return Ok(done.into_iter().map(|(_, whole)| whole.expect("a derived column worked out")).collect());
    }
    // A record is of every row, so each column is worked out over every row once to check it.
    let unchecked: Vec<(String, String)> = wanted.iter().filter(|(name, _)| !file.checked(name, false)).cloned().collect();
    if !unchecked.is_empty() {
        for ((name, expression), (measured, _)) in unchecked.iter().zip(worked(&store, &unchecked, Ask::default())?) {
            agrees(&store, name, expression, &measured.unwrap_or_default())?;
            file.checked(name, true);
        }
    }
    let done = worked(&store, &wanted, Ask { whole: true, range: Some((from, to)), ..Ask::default() })?;
    Ok(done.into_iter().map(|(_, whole)| whole.expect("a derived column worked out")).collect())
}

/// An empty file's derived columns, worked out over no rows: each of its record's kind and shape
/// (its checksums and facts are of the rows the file no longer holds); how many.
pub(super) fn checked_empty(file: &Reader) -> Result<usize> {
    let derived = listed(file.header());
    let store = Store::new(file, &derived, true)?;
    let records = file.header()["derived"]["columns"].as_array().map_or(&[][..], Vec::as_slice);
    let done = worked(&store, &derived, Ask { hollow: true, whole: true, ..Ask::default() })?;
    for (((name, _), record), (_, whole)) in derived.iter().zip(records).zip(done) {
        let whole = whole.expect("a derived column worked out whole");
        let want = Store::none_of(record);
        if (&whole.kind, whole.offsets.is_some()) != (&want.kind, want.offsets.is_some()) {
            let shape = |held: &Held| if held.offsets.is_some() { format!("jagged of {}", held.kind) } else { held.kind.clone() };
            return Err(invalid(format!("derived.columns[{name:?}] is {}, and its expression, worked out over no rows, gives {} (docs/FORMAT.md §10): {MEND}", shape(&want), shape(&whole))));
        }
    }
    Ok(derived.len())
}

/// Check a file's derived columns against the recipe it carries and its records, a run at a
/// time, none held whole; how many.
pub fn checked(file: &Reader) -> Result<usize> {
    let derived = listed(file.header());
    if agreed(file.header())? == 0 {
        return Ok(0);
    }
    let store = Store::new(file, &derived, false)?;
    for ((name, expression), (measured, _)) in derived.iter().zip(worked(&store, &derived, Ask::default())?) {
        agrees(&store, name, expression, &measured.unwrap_or_default())?;
        file.checked(name, true);
    }
    Ok(derived.len())
}

/// For a file a newer rvr pressed than this version, or another program: what to do, said after what failed.
fn newer(header: &Value) -> Option<String> {
    let made = header["made"]["toolchain"]["version"].as_str()?;
    let program = header["made"]["toolchain"]["program"].as_str()?;
    if program != "rvr" {
        return Some(format!("The file was made by {program} {made}, not by rvr, and may hold what this rvr does not read: press its study again with rvr to work it out here."));
    }
    let this = env!("CARGO_PKG_VERSION");
    later(made, this).then(|| {
        format!("The file was made by rvr {made}, newer than this one ({this}), and may use what only a newer version reads: update rvr and rowver to {made} or later (pip install -U rowver updates both; or that release's rvr download, or rowver crate).")
    })
}

/// What a newer file's derived column that cannot be worked out here adds to its failure.
fn unread(header: &Value) -> Option<String> {
    newer(header).map(|update| format!("{update} Its stored columns still read, each asked for by name."))
}

/// What to do when a file's recipe or derived columns cannot be worked out here.
pub fn remedy(header: &Value) -> String {
    newer(header).unwrap_or_else(|| "Press its study again.".into())
}

/// Whether version `made` comes after `this`, each MAJOR.MINOR.PATCH (a part that is no number
/// counts as 0), by their three numbers.
fn later(made: &str, this: &str) -> bool {
    let numbers = |version: &str| version.split('.').take(3).map(|part| part.parse().unwrap_or(0)).collect::<Vec<u64>>();
    numbers(made) > numbers(this)
}

/// A file's derived columns, by name and expression, as its header records them.
pub fn listed(header: &Value) -> Vec<(String, String)> {
    let records = header["derived"]["columns"].as_array().into_iter().flatten();
    let text = |r: &Value, key: &str| r[key].as_str().unwrap_or_default().to_owned();
    records.map(|r| (text(r, "name"), text(r, "expression"))).collect()
}

/// Check that the recipe a header carries lists its derived columns; how many there are.
pub(super) fn agreed(header: &Value) -> Result<usize> {
    let derived = listed(header);
    // A recipe this rowver cannot read is checked only when there are derived columns to work out.
    match header["made"]["recipe"]["text"].as_str().map(Recipe::read) {
        Some(Ok(recipe)) if recipe.derived != derived => {
            return Err(invalid(format!("the [derived] table of the recipe it carries does not list the derived columns its header records: one of the two was edited after it was written. {}", remedy(header))))
        }
        Some(Err(e)) if !derived.is_empty() => {
            return Err(invalid(format!("the recipe it carries cannot be read ({}), so its derived columns cannot be worked out. {}", e.line(), remedy(header))))
        }
        None if !derived.is_empty() => return Err(invalid(format!("it has derived columns but carries no recipe to say what they are. {}", remedy(header)))),
        _ => {}
    }
    Ok(derived.len())
}

#[cfg(test)]
mod tests {
    use super::later;

    // FORMAT 10
    #[test]
    fn a_version_is_later_by_its_three_numbers() {
        assert!(later("15.1.0", "15.0.0") && later("16.0.0", "15.9.9") && later("15.0.10", "15.0.9"));
        assert!(!later("15.0.0", "15.0.0") && !later("15.0.0", "15.1.0"));
    }
}
