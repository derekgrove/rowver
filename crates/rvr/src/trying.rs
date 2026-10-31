//! `rvr try`: one expression over the first events of a study's sample, printed event by event,
//! to see what it gives before pressing. It reads the recipe for the names it defines, and with
//! `--kept` shows only the events the recipe's cuts keep.

use crate::press::{self, count, fields, hint, kept, Branches};
use crate::Words;
use rvr_recipe::{parse, per_event, Array, Constant, Kind, Scope, Store, Values};
use std::path::Path;

const USAGE: &str = "usage:
  rvr try \"EXPRESSION\" [DIR] [--events N] [--sample NAME] [--kept]
      what EXPRESSION gives on the first N events (10) of the first sample (or NAME) of the
      study in DIR (.); --kept shows only the events the recipe's [keep] cuts keep";

/// How many entries are read at a time while looking for kept events.
const RUN: u64 = 10_000;

/// `rvr try …` with the words after `try`: the exit status (2 for a wrong use of it).
pub fn main(args: &[&str]) -> u8 {
    match run(args) {
        Ok(()) => 0,
        Err(error) if error.ends_with(USAGE) => {
            eprintln!("rvr try: {error}");
            2
        }
        Err(error) => {
            eprintln!("rvr try: {error}");
            1
        }
    }
}

fn run(args: &[&str]) -> Result<(), String> {
    let words = Words::read(args, &["--events", "--sample"], &["--kept"], USAGE)?;
    let (text, dir) = match words.plain[..] {
        [text] => (text, "."),
        [text, dir] => (text, dir),
        _ => return Err(format!("try takes one expression and at most one study folder.\n{USAGE}")),
    };
    let wanted: usize = match words.value("--events") {
        None => 10,
        Some(n) => n.replace('_', "").parse().ok().filter(|n| *n > 0).ok_or_else(|| {
            format!("--events takes a number of events above 0, not {n}")
        })?,
    };
    let only_kept = words.has("--kept");
    let expression = parse(text).map_err(|e| format!("the expression: {}", e.0))?;
    let dir = Path::new(dir);
    let (mut texts, mut lock) = press::texts(dir)?;
    if crate::tables::offer(dir, &texts.recipe, "try")? {
        (texts, lock) = press::texts(dir)?;
    }
    let recipe = press::study_recipe(&texts.recipe)?;
    let index = press::sample_at(&lock, words.value("--sample"))?;
    let sample = &lock.sample[index];
    let (root, file) = press::first_readable(&lock, index, dir, true)?;
    let at = &file.at;
    let tree = root.tree_at(&sample.tree).map_err(|e| format!("{at}: its tree {:?}: {e}", sample.tree))?;
    let names: Vec<String> = tree.walk().into_iter().map(|(_, b)| b.name.clone()).collect();
    let mut metadata = fields(&sample.metadata);
    metadata.insert("file".into(), Constant::Text(at.clone()));
    // A total is known once every event is read: here it stands in as 1, as in a press's trial.
    for total in &recipe.totals {
        let named = format!("sample.{}", total.name);
        let reads = text.match_indices(&named).any(|(at, _)| !text[at + named.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_'));
        if reads {
            println!("sample.{} is a total of [totals], known once every event is read: it stands in as 1 here.", total.name);
        }
        metadata.entry(total.name.clone()).or_insert(Constant::Float(1.0));
    }
    let total = tree.entries.max(0) as u64;
    let shown_name = at.rsplit('/').next().unwrap_or(at);
    let (mut shown, mut first, mut kind) = (0, 0, None);
    let mut cutflow = vec![0; recipe.keep.len()];
    while shown < wanted && first < total {
        let last = match only_kept {
            true => (first + RUN).min(total),
            false => (first + (wanted - shown) as u64).min(total),
        };
        let entries = (last - first) as usize;
        let branches = Branches::over(&root, &tree, first as i64, last as i64);
        let scope = Scope::new(&branches, &metadata, &recipe.defines);
        let within = |error: String| {
            let entries = format!("entries {} to {}", count(first), count(last - 1));
            format!("{shown_name}, {entries}: {error}{}", hint(&error, &names, &recipe, file.lfn.is_some()))
        };
        let keep = match only_kept {
            true => kept(&recipe, &scope, vec![true; entries], &mut cutflow).map_err(within)?,
            false => vec![true; entries],
        };
        let result = scope
            .evaluate(&expression)
            .and_then(|value| per_event(value, entries))
            .map_err(|e| within(e.0))?;
        if kind.is_none() {
            let described = described(&result);
            println!("{}: {shown_name}, tree {} ({described})", sample.name, sample.tree);
            kind = Some(described);
        }
        for (offset, keep) in keep.iter().enumerate() {
            if *keep && shown < wanted {
                println!("  entry {:>8}   {}", first + offset as u64, value(&result, offset));
                shown += 1;
            }
        }
        first = last;
    }
    if shown < wanted {
        let why = if only_kept { " the cuts keep" } else { "" };
        println!("The file has no more events{why}: {shown} shown of its {} entries.", count(total));
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

/// One event's value as text: a number, true or false, or a list of them.
fn value(array: &Array, event: usize) -> String {
    let one = |values: &Values, at: usize| match &values.store {
        Store::Whole(held) if values.kind == Kind::B1 => (held[at] != 0).to_string(),
        Store::Whole(held) => held[at].to_string(),
        Store::Float(held) if values.kind == Kind::named("f4").unwrap() => (held[at] as f32).to_string(),
        Store::Float(held) => held[at].to_string(),
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
