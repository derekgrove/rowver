//! `rvr try`: one expression over the first events of a study's sample, printed event by event,
//! to see what it gives before pressing. It reads the recipe for the names it defines, and with
//! `--kept` shows only the events the recipe's cuts keep.

use crate::gather::kept;
use crate::open::{entries as located, first_readable, hint, sample_in, Branches};
use crate::options::Options;
use crate::plan::Choose;
use rowver::rvr::derived::fields;
use rvr_recipe::{per_event, Array, Columns, Constant, Picked, Scope, Store, Values};
use std::path::Path;

pub(crate) const USAGE: &str = "usage:
  rvr try \"EXPRESSION\" [DIR] [--events N] [--sample NAME] [--kept]
      what EXPRESSION gives on the first N events (10) of the first sample (or NAME) of the
      study in DIR (.); --kept shows only the events the recipe's [keep] cuts keep";

/// How many entries are read at a time while looking for kept events.
const RUN: u64 = 10_000;

/// `rvr try` with its options: the exit status.
pub fn main(options: &Options) -> u8 {
    let wanted = match options.value("--events").map(|n| crate::options::count("--events", n, i64::MAX as u64)).transpose() {
        Ok(wanted) => usize::try_from(wanted.unwrap_or(10)).unwrap_or(usize::MAX),
        Err(why) => return crate::wrong("try", &why),
    };
    crate::report("try", run(options, wanted).map(|()| 0))
}

fn run(options: &Options, wanted: usize) -> Result<(), String> {
    let words = options.plain();
    let (text, dir) = match words[..] {
        [text] => (text, "."),
        [text, dir, ..] => (text, dir),
        [] => return Err("try takes the expression to try".into()),
    };
    let only_kept = options.has("--kept");
    let dir = Path::new(dir);
    let mut texts = crate::texts::read(dir)?;
    if crate::tables::offer(dir, &texts.recipe, "try", "go on with the try")? {
        texts = crate::texts::read(dir)?;
    }
    // Read as a press reads it: its validators, then the agreement check.
    let checked = crate::plan::checked(&texts, &Choose::Options(Default::default()))?;
    let (recipe, lock) = (checked.recipe, checked.lock);
    let index = sample_in(&lock, options.value("--sample"))?;
    let expression = recipe.expression(text).map_err(|e| format!("the expression: {}", e.0.replace('\n', "\nthe expression: ")))?;
    let sample = &lock.samples[index];
    let (root, file) = first_readable(&lock, index, dir)?;
    let at = &file.at;
    let tree = root.tree_at(&sample.tree).map_err(|e| format!("{at}: its tree {:?}: {e}", sample.tree))?;
    let names: Vec<String> = tree.walk().into_iter().map(|(_, b)| b.name.clone()).collect();
    let mut metadata = fields(&sample.metadata);
    metadata.insert("file".into(), Constant::Text(at.clone()));
    // A total is known once every event is read: here it stands in as 1, as in a press's trial.
    let mut read = std::collections::BTreeSet::new();
    rvr_recipe::fields_of(&expression, &mut read);
    for total in recipe.totals.iter().filter(|total| read.contains(&total.name)) {
        println!("sample.{} is a total of [totals], known once every event is read: it stands in as 1 here.", total.name);
    }
    rowver::rvr::derived::standing_in(&mut metadata, &recipe);
    let total = tree.entries.max(0) as u64;
    let shown_name = at.rsplit('/').next().unwrap_or(at);
    let (mut shown, mut first, mut kind) = (0, 0, None);
    while shown < wanted && first < total {
        let last = match only_kept {
            true => (first + RUN).min(total),
            false => (first + (wanted - shown) as u64).min(total),
        };
        let entries = (last - first) as usize;
        let within = |(error, part): (rvr_recipe::Error, std::ops::Range<usize>)| format!("{shown_name}, {}: {}{}", located(&error, first, &part), error.0, hint(&error, &names, &recipe));
        // In fewer entries at a time where their pairs would be too many together.
        let branches = Branches::over(&root, &tree, first as i64, last as i64);
        // The cuts first, then the expression over the events they keep alone, as a press works a column out.
        let keep = match only_kept {
            true => {
                let mut cuts = |run: &dyn Columns, from: usize, to: usize| kept(&recipe, &Scope::new(run, &metadata, &recipe.defines), vec![true; to - from], &mut vec![0; recipe.keep.len()]);
                rvr_recipe::over_runs(&branches, entries, &mut cuts).map_err(within)?.concat()
            }
            false => vec![true; entries],
        };
        let at: Vec<usize> = keep.iter().enumerate().filter(|(_, keep)| **keep).map(|(at, _)| at).take(wanted - shown).collect();
        if at.is_empty() {
            first = last;
            continue;
        }
        let mut work = |run: &dyn Columns, from: usize, to: usize| Scope::new(run, &metadata, &recipe.defines).evaluate(&expression).and_then(|value| per_event(value, to - from));
        let results = match at.len() == entries {
            true => rvr_recipe::over_runs(&branches, entries, &mut work).map_err(within)?,
            false => rvr_recipe::over_runs(&Picked::new(&branches, &at, entries), at.len(), &mut work).map_err(crate::take::unpicked(&at)).map_err(within)?,
        };
        let result = Array::joined(results).map_err(|e| within(crate::take::unpicked(&at)((e, 0..at.len()))))?;
        // Shown as a column of it would hold them: a branch alone as read, any other each NaN the one quiet NaN.
        let result = if recipe.alone(&expression) { result } else { result.settled() };
        if kind.is_none() {
            let described = described(&result);
            println!("{}: {shown_name}, tree {} ({described})", sample.name, sample.tree);
            kind = Some(described);
        }
        for (row, offset) in at.iter().enumerate() {
            println!("  entry {:>8}   {}", first + *offset as u64, value(&result, row));
            shown += 1;
        }
        first = last;
    }
    if shown < wanted {
        let why = if only_kept { " the cuts keep" } else { "" };
        println!("The file has no more events{why}: {shown} shown of its {}.", crate::samples::some(total as usize, "entry", "entries"));
    }
    Ok(())
}

/// What a result is: its kind and whether it holds one value or a list per event.
fn described(array: &Array) -> String {
    match array {
        Array::Event(values) => format!("{}, one value per event", values.kind.name()),
        Array::Object { values, .. } => format!("{}, a list per event", values.kind.name()),
        Array::Constant(_) => "one value for every event".into(),
    }
}

/// One event's value as text, as rowver reads values: a number, true or false, or a list of them.
fn value(array: &Array, event: usize) -> String {
    let one = |values: &Values, at: usize| {
        let single = match &values.store {
            Store::Whole(held) => Values::whole(values.kind, vec![held[at]]),
            Store::Float(held) => Values::float(values.kind, vec![held[at]]),
        };
        let mut out = String::new();
        let _ = rowver::rvr::push_shown(values.kind.name(), &single.to_bytes(), &mut out);
        out
    };
    match array {
        Array::Event(values) => one(values, event),
        Array::Object { offsets, values } => {
            let members: Vec<String> = (offsets[event]..offsets[event + 1]).map(|at| one(values, at)).collect();
            format!("[{}]", members.join(", "))
        }
        Array::Constant(constant) => format!("{constant:?}"),
    }
}
