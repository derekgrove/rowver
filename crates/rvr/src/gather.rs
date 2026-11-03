//! What a press works out over one run of a file's entries: the cuts and totals over its chosen
//! entries, the stored columns over its kept events, gathered by column on their way to the spool,
//! and the derived columns tried over them (docs/SPEC.md §4.2-§4.6).

use crate::holes::{Failed, Fault};
use rowver::write;
use rowver::NumericBuffer;
use rvr_recipe::{Array, Columns, Constant, Kind, Recipe, Scope, Values};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// One stored column gathered over the kept events of a file, of the kind the Plan gives it.
pub(crate) struct Gathered {
    /// The column's name.
    pub(crate) name: String,
    /// Its kind, as the Plan gives it.
    pub(crate) kind: Kind,
    /// Its values gathered so far, as little-endian bytes.
    pub(crate) bytes: Vec<u8>,
    /// For a list column, how many members each kept event holds.
    pub(crate) counts: Option<Vec<usize>>,
}

impl Gathered {
    /// Nothing yet of a column of `kind`, a list per event when `listed`.
    pub(crate) fn new(name: &str, (kind, listed): (Kind, bool)) -> Self {
        Gathered { name: name.to_owned(), kind, bytes: Vec::new(), counts: listed.then(Vec::new) }
    }
}

/// Which events one cut lets through: those it does not apply to, and those that pass it.
fn lets_through(cut: &rvr_recipe::Cut, scope: &Scope, entries: usize) -> Result<Vec<bool>, rvr_recipe::Error> {
    let place = format!("[keep] {}", cut.name);
    let applies = match &cut.when {
        None => None,
        Some(when) => match scope.evaluate(when).map_err(|e| e.at(&place))? {
            Array::Constant(Constant::Missing(_)) => return Ok(vec![true; entries]),
            value => Some(Scope::condition(value, entries).map_err(|e| e.at(&place))?),
        },
    };
    if applies.as_ref().is_some_and(|applies| !applies.contains(&true)) {
        return Ok(vec![true; entries]);
    }
    let mut pass = scope.mask(&cut.pass, entries).map_err(|e| e.at(&place))?;
    for (pass, applies) in pass.iter_mut().zip(applies.iter().flatten()) {
        *pass |= !applies;
    }
    Ok(pass)
}

/// Apply the cuts to one run of a file's events, counting what each leaves.
pub(crate) fn kept(recipe: &Recipe, scope: &Scope, mut keep: Vec<bool>, cutflow: &mut [u64]) -> Result<Vec<bool>, rvr_recipe::Error> {
    let entries = keep.len();
    for (index, cut) in recipe.keep.iter().enumerate() {
        for (keep, through) in keep.iter_mut().zip(lets_through(cut, scope, entries)?) {
            *keep &= through;
        }
        cutflow[index] += keep.iter().filter(|keep| **keep).count() as u64;
    }
    Ok(keep)
}

/// Add every event of `scope` to each total of `[totals]` over the tree read, one at a time in
/// entry order, so a file's sum does not depend on its runs.
pub(crate) fn summed(recipe: &Recipe, scope: &Scope, events: usize, totals: &mut [f64]) -> Result<(), rvr_recipe::Error> {
    let lines = recipe.totals.iter().filter(|total| total.tree.is_none());
    for (total, sum) in lines.zip(totals) {
        add_to(sum, &total.name, scope.evaluate(&total.value).and_then(|v| rvr_recipe::per_event(v, events)))?;
    }
    Ok(())
}

/// Add to `sum` every value, in order: one number per entry, and a sum that stays finite (a data
/// error at the entry whose value makes it not).
pub(crate) fn add_to(sum: &mut f64, name: &str, array: rvr_recipe::Result<Array>) -> Result<(), rvr_recipe::Error> {
    let place = format!("[totals] {name}");
    let values = match array.map_err(|e| e.at(&place))? {
        Array::Event(values) => values,
        _ => return Err(rvr_recipe::problem(format!("{place}: a total sums one number per event, and this gives a list for each: sum the list, as sum(...)"))),
    };
    for index in 0..values.len() {
        *sum += values.number(index);
        if !sum.is_finite() {
            return Err(rvr_recipe::data(index, format!("{place}: its sum is not a finite number once this entry's value, {:?}, is added, and a file holds only finite totals: give such events a finite value, as if(isfinite(x), x, 0)", values.number(index))));
        }
    }
    Ok(())
}

/// A stored column over the events of `scope`: worked out, then given the kind the recipe asks it
/// to be stored as, by the recipe reference's conversion rule.
pub(crate) fn stored(recipe: &Recipe, scope: &Scope, (column, expression): &(String, rvr_recipe::Expression), events: usize) -> rvr_recipe::Result<Array> {
    // An error that begins with the column's name already (a branch of that name that could not be
    // read) is not named again.
    let named = |e: rvr_recipe::Error| match e.0.starts_with(&format!("{column}: ")) {
        true => e,
        false => e.at(&format!("[columns] {column}")),
    };
    let array = scope.evaluate(expression).and_then(|value| rvr_recipe::per_event(value, events)).map_err(named)?;
    match recipe.stored.get(column) {
        Some(kind) => array.convert(*kind).map_err(|e| e.at(&format!("[columns] {column}"))),
        None => Ok(array),
    }
}

/// Add every event of a column's values to what the file has gathered; one of another kind than
/// the Plan's is a bug.
pub(crate) fn gather(held: &mut Gathered, column: Array) -> Result<(), String> {
    let (values, offsets) = match &column {
        Array::Event(values) => (values, None),
        Array::Object { offsets, values } => (values, Some(offsets)),
        Array::Constant(_) => return Err(format!("[columns] {} gave a fixed value, not a column", held.name)),
    };
    if values.kind != held.kind || offsets.is_some() != held.counts.is_some() {
        return Err(format!("[columns] {} gave {} {}, where the agreement check found it {}", held.name, values.kind.name(), if offsets.is_some() { "lists" } else { "values" }, held.kind.name()));
    }
    let w = held.kind.width();
    let bytes = values.to_bytes();
    match (offsets, &mut held.counts) {
        (Some(offsets), Some(counts)) => {
            let (from, to) = (offsets[0], offsets[offsets.len() - 1]);
            held.bytes.extend_from_slice(&bytes[from * w..to * w]);
            counts.extend(offsets.windows(2).map(|pair| pair[1] - pair[0]));
        }
        _ => held.bytes.extend_from_slice(&bytes),
    }
    Ok(())
}

/// How much each column has gathered: its bytes and its lists.
pub(crate) fn held_lengths(gathered: &[Gathered]) -> Vec<(usize, usize)> {
    gathered.iter().map(|g| (g.bytes.len(), g.counts.as_ref().map_or(0, Vec::len))).collect()
}

/// Put each column back to what it had gathered at `before`.
pub(crate) fn put_back(gathered: &mut [Gathered], before: &[(usize, usize)]) {
    for (held, (bytes, counts)) in gathered.iter_mut().zip(before) {
        held.bytes.truncate(*bytes);
        if let Some(held) = &mut held.counts {
            held.truncate(*counts);
        }
    }
}

/// One run's kept events as an expression reads them: its stored columns, and the derived ones so far.
struct Run(RefCell<BTreeMap<String, Array>>);

impl Columns for Run {
    fn column(&self, name: &str) -> rvr_recipe::Result<Option<Array>> {
        Ok(self.0.borrow().get(name).cloned())
    }

    fn has(&self, name: &str) -> bool {
        self.0.borrow().contains_key(name)
    }
}

/// Work the derived columns out over the events a run kept (`entries`, the file's entries of
/// them), gathered since `before`: a value one cannot take stops the press naming its entry, and
/// anything else is a bug, as the agreement check found each one's kind (`kinds`).
pub(crate) fn derived_tried(recipe: &Recipe, kinds: &[(Kind, bool)], gathered: &[Gathered], before: &[(usize, usize)], sample: &BTreeMap<String, Constant>, entries: &[u64]) -> Result<(), Failed> {
    let mut held = BTreeMap::new();
    for (column, (bytes, counts)) in gathered.iter().zip(before) {
        let values = Values::from_bytes(column.kind, &column.bytes[*bytes..]);
        let array = match &column.counts {
            None => Array::Event(values),
            Some(all) => {
                let mut offsets = vec![0];
                for count in &all[*counts..] {
                    offsets.push(offsets[offsets.len() - 1] + count);
                }
                Array::Object { offsets: Arc::new(offsets), values }
            }
        };
        held.insert(column.name.clone(), array);
    }
    let run = Run(RefCell::new(held));
    // A total is known only once every event is read: the trial gives it a stand-in.
    let mut sample = sample.clone();
    for total in &recipe.totals {
        sample.entry(total.name.clone()).or_insert(Constant::Float(1.0));
    }
    let defines = BTreeMap::new();
    for ((name, expression), want) in recipe.derived.iter().zip(kinds) {
        let bug = |why: String| Failed::new(Fault::Bug, format!("derived column {name}: {why}"));
        let parsed = rvr_recipe::parse(expression).map_err(|e| bug(e.0))?;
        let mut work = |part: &dyn Columns, from: usize, to: usize| Scope::new(part, &sample, &defines).evaluate(&parsed).and_then(|v| rvr_recipe::per_event(v, to - from));
        let array = rvr_recipe::over_runs(&run, entries.len(), &mut work).map_err(|(e, _)| e).and_then(Array::joined);
        let array = match array {
            Ok(array) => array,
            Err(e) => match e.1 {
                Some(row) => return Err(Failed::new(Fault::Data, format!("entry {}: derived column {name}: {}", crate::press::count(entries[row]), e.0))),
                None => return Err(bug(e.0)),
            },
        };
        let found = match &array {
            Array::Event(values) => (values.kind, false),
            Array::Object { values, .. } => (values.kind, true),
            Array::Constant(_) => return Err(bug("a fixed value, not a column".into())),
        };
        if found != *want {
            return Err(bug(format!("it gave {} where the agreement check found {}", found.0.name(), want.0.name())));
        }
        run.0.borrow_mut().insert(name.clone(), array);
    }
    Ok(())
}

/// Send what a file has gathered since it last did to the spool, keeping each column's kind:
/// its columns, then its entry numbers.
pub(crate) fn spooled(spool: &write::Spool, gathered: &mut [Gathered], entry: &mut Vec<i64>, order: write::Order) -> Result<(), String> {
    let failed = |e: rowver::Error| format!("its kept values could not be held on disk ({e}): free disk space beside the output");
    let entries = write::array(&std::mem::take(entry));
    spool.add(gathered.len(), order, write::Values::Flat(entries)).map_err(failed)?;
    for (index, held) in gathered.iter_mut().enumerate() {
        let buffer = NumericBuffer {
            kind: held.kind.name().into(),
            count: held.bytes.len() / held.kind.width(),
            bytes: std::mem::take(&mut held.bytes),
        };
        let values = match &mut held.counts {
            Some(counts) => write::Values::Jagged { counts: std::mem::take(counts), content: buffer },
            None => write::Values::Flat(buffer),
        };
        spool.add(index, order, values).map_err(failed)?;
    }
    Ok(())
}
