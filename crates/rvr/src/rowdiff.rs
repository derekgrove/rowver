//! `rvr diff A B --rows` (SPEC §9.4.1): the two files' events joined by identity, sample by
//! sample, then compared value by value, one column at a time.
//!
//! What it holds: each file's key columns (by default its entries, and a rank of its files' ROOT
//! UUIDs, 12 bytes a row), read whole, as every array is; the join, two row numbers (u32) and a
//! flag per event both hold; and, while the values are compared, one column of each file (a derived
//! one worked out alone, a chunk of its inputs at a time).

use crate::samples::some;
use rowver::rvr::{derived, grouped as count, Held, Reader};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// A column read, with the width of its values.
struct Col {
    held: Held,
    w: usize,
}

impl Col {
    fn new(held: Held, path: &str) -> Result<Col, String> {
        let w = rowver::rvr::width(&held.kind).map_err(|e| rowver::plain(path, &e))?;
        Ok(Col { held, w })
    }

    /// The bytes of row `r`: its value, or its list's values.
    fn value(&self, r: usize) -> &[u8] {
        let (from, to) = self.held.offsets.as_ref().map_or((r, r + 1), |o| (o[r], o[r + 1]));
        &self.held.bytes[from * self.w..to * self.w]
    }

    /// Row `r` in words, as `rvr dump --all` prints it.
    fn shown(&self, r: usize) -> String {
        let mut out = String::new();
        crate::dump::push_row(&self.held, r, &mut out);
        out
    }

    /// Row `r`'s values as the hex of their bits, for two whose words are alike (as NaNs).
    fn bits(&self, r: usize) -> String {
        let hex: Vec<String> = self.value(r).chunks(self.w).map(|b| b.iter().rev().fold("0x".to_owned(), |t, x| t + &format!("{x:02x}"))).collect();
        match self.held.offsets {
            Some(_) => format!("[{}]", hex.join(", ")),
            None => hex.concat(),
        }
    }
}

/// What an event is known by in one file: its file's ROOT UUID and which of its sample's files with
/// that UUID it is (as one rank over both files), and its entry; or the values of the key columns.
enum Keys {
    Entry { file: Vec<u32>, entry: Vec<i64> },
    Columns(Vec<Col>),
}

impl Keys {
    /// How row `r` of these keys compares with row `s` of `other`'s.
    fn cmp(&self, r: usize, other: &Keys, s: usize) -> Ordering {
        match (self, other) {
            (Keys::Entry { file, entry }, Keys::Entry { file: f, entry: e }) => (file[r], entry[r]).cmp(&(f[s], e[s])),
            (Keys::Columns(mine), Keys::Columns(theirs)) => mine.iter().zip(theirs).map(|(a, b)| a.value(r).cmp(b.value(s))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal),
            _ => unreachable!("both files are keyed alike"),
        }
    }

    /// Row `r`'s key in words.
    fn said(&self, r: usize, names: &[String], files: &[Read]) -> String {
        match self {
            Keys::Entry { entry, .. } => {
                let path = files.iter().find(|(from, to, _, _)| (*from..*to).contains(&r)).map_or("?", |(_, _, path, _)| path);
                format!("entry {} of {path}", entry[r])
            }
            Keys::Columns(columns) => names.iter().zip(columns).map(|(n, c)| format!("{n} {}", c.shown(r))).collect::<Vec<_>>().join(", "),
        }
    }
}

/// One file read: its rows (from, to), path, and identity: its sample, ROOT UUID, and which of its
/// sample's files with that UUID it is (copies of one file share a UUID).
type Read = (usize, usize, String, (String, String, usize));

/// One file, opened, with each sample's rows and each file read.
struct Side {
    reader: Reader,
    samples: Vec<(String, usize, usize)>,
    files: Vec<Read>,
}

fn side(path: &str) -> Result<Side, String> {
    let reader = Reader::open(path).map_err(|e| rowver::plain(path, &e))?;
    if reader.empty() {
        return Err(format!("{path} is empty: its arrays are gone, so its events cannot be compared; rvr refill {path} presses them again"));
    }
    let data = &reader.header()["data"];
    let (mut samples, mut at) = (Vec::new(), 0);
    for s in data["samples"].as_array().into_iter().flatten() {
        let rows = s["rows"].as_u64().unwrap_or(0) as usize;
        samples.push((s["name"].as_str().unwrap_or("").to_owned(), at, at + rows));
        at += rows;
    }
    let (mut files, mut at) = (Vec::new(), 0);
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    for f in data["files"].as_array().into_iter().flatten() {
        let rows = f["rows"].as_u64().unwrap_or(0) as usize;
        let (sample, file) = (f["sample"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or(""));
        let uuid = reader.header()["made"]["files"][sample]["files"][file]["uuid"].as_str().unwrap_or("").to_owned();
        let copy = seen.entry((sample.to_owned(), uuid.clone())).or_default();
        files.push((at, at + rows, file.to_owned(), (sample.to_owned(), uuid, *copy)));
        *copy += 1;
        at += rows;
    }
    if at > u32::MAX as usize {
        return Err(format!("{path} holds {} rows, more than the 4,294,967,295 (2^32 − 1) rvr diff --rows compares: rvr diff without --rows compares their headers", count(at as u64)));
    }
    Ok(Side { reader, samples, files })
}

/// The keys of a file: by default its rows' files (their sample, UUID and copy, ranked in `uuids`)
/// and entries; else the columns named, each a stored flat column.
fn keys(side: &Side, path: &str, names: &[String], uuids: &BTreeMap<(String, String, usize), u32>) -> Result<Keys, String> {
    let read = |e: rowver::Error| rowver::plain(path, &e);
    if names.is_empty() {
        let entry = side.reader.entry().map_err(read)?;
        let mut file = vec![0; entry.len()];
        for (from, to, _, uuid) in &side.files {
            file[*from..*to].fill(uuids[uuid]);
        }
        return Ok(Keys::Entry { file, entry });
    }
    let header = side.reader.header();
    let mut columns = Vec::new();
    for name in names {
        match header["data"]["cols"].as_array().into_iter().flatten().find(|c| c["name"] == name.as_str()) {
            None if derived::listed(header).iter().any(|(n, _)| n == name) => return Err(format!("--key {name}: {name} is a derived column of {path}, worked out when read, and a key is stored columns: name the stored columns it is worked out from, or others")),
            None => return Err(format!("{path} stores no column {name}, so --key cannot match events by it: name stored columns both files hold")),
            Some(c) if c["kind"] == "jagged" => return Err(format!("--key {name}: {name} is a list column in {path}, and a key is flat columns, one value per event")),
            Some(_) => columns.push(Col::new(side.reader.column(name).map_err(read)?.expect("a stored column reads"), path)?),
        }
    }
    Ok(Keys::Columns(columns))
}

/// The rows of one sample, ordered by key, or the two rows that share a key.
fn ordered(keys: &Keys, from: usize, to: usize) -> Result<Vec<u32>, (usize, usize)> {
    let mut rows: Vec<u32> = (from as u32..to as u32).collect();
    rows.sort_by(|&r, &s| keys.cmp(r as usize, keys, s as usize));
    match rows.windows(2).find(|w| keys.cmp(w[0] as usize, keys, w[1] as usize).is_eq()) {
        Some(w) => Err((w[0] as usize, w[1] as usize)),
        None => Ok(rows),
    }
}

/// A sample both files hold: the events both hold (row in A, row in B, in A's row order), and how
/// many only one holds.
struct Joined {
    name: String,
    pairs: Vec<(u32, u32)>,
    only: [usize; 2],
    first: Option<(usize, String, String, String)>,
    differ: usize,
}

/// Compare two files' events; the exit status (SPEC §9.4.1).
pub fn main(a: &str, b: &str, key: Option<&str>, sample: Option<&str>) -> u8 {
    match compare(a, b, key, sample) {
        Ok(same) => u8::from(!same),
        Err(why) => {
            eprintln!("rvr diff: {why}");
            2
        }
    }
}

fn compare(a: &str, b: &str, key: Option<&str>, only: Option<&str>) -> Result<bool, String> {
    let sides = [side(a)?, side(b)?];
    let names: Vec<String> = key.map(|k| k.split(',').map(str::trim).map(str::to_owned).collect()).unwrap_or_default();
    if names.iter().any(String::is_empty) {
        return Err(format!("--key {:?} names an empty column (two commas together, or one at an end): name the key columns with a comma between each two, as --key run,luminosityBlock,event", key.unwrap_or_default()));
    }
    let uuids: BTreeMap<(String, String, usize), u32> = sides.iter().flat_map(|s| s.files.iter().map(|f| f.3.clone())).collect::<BTreeSet<_>>().into_iter().zip(0..).collect();
    let keyed = [keys(&sides[0], a, &names, &uuids)?, keys(&sides[1], b, &names, &uuids)?];
    if let (Keys::Columns(x), Keys::Columns(y)) = (&keyed[0], &keyed[1]) {
        for ((name, x), y) in names.iter().zip(x).zip(y) {
            if x.held.kind != y.held.kind {
                return Err(format!("--key {name}: {name} is {} in {a} and {} in {b}, and a key is a column both hold with one kind", x.held.kind, y.held.kind));
            }
        }
    }
    if let Some(name) = only.filter(|n| !sides.iter().any(|s| s.samples.iter().any(|(m, _, _)| m == n))) {
        let held: BTreeSet<&str> = sides.iter().flat_map(|s| s.samples.iter().map(|(m, _, _)| m.as_str())).collect();
        return Err(format!("neither {a} nor {b} holds a sample {name}: the samples they hold are {}", held.into_iter().collect::<Vec<_>>().join(", ")));
    }
    let wanted = |name: &str| only.is_none_or(|o| o == name);
    let mut said = Vec::new();
    let mut joined = Vec::new();
    for (path, one, other) in [(a, 0, 1), (b, 1, 0)] {
        for (name, from, to) in sides[one].samples.iter().filter(|(n, _, _)| wanted(n)) {
            if !sides[other].samples.iter().any(|(m, _, _)| m == name) {
                said.push(format!("sample {name}: only {path} holds it ({})", some(to - from, "event", "events")));
            }
        }
    }
    let describe = |side: usize, r: usize| keyed[side].said(r, &names, &sides[side].files);
    for (name, from, to) in sides[0].samples.iter().filter(|(n, _, _)| wanted(n)) {
        let Some((_, f2, t2)) = sides[1].samples.iter().find(|(m, _, _)| m == name) else { continue };
        let twice = |path: &str, side: usize, r: usize| format!("sample {name} of {path} holds two events of one key ({}): name key columns that tell its events apart", describe(side, r));
        let rows_a = ordered(&keyed[0], *from, *to).map_err(|(r, _)| twice(a, 0, r))?;
        let rows_b = ordered(&keyed[1], *f2, *t2).map_err(|(r, _)| twice(b, 1, r))?;
        let (mut pairs, mut only, (mut i, mut j)) = (Vec::new(), [0, 0], (0, 0));
        while i < rows_a.len() || j < rows_b.len() {
            let order = match (rows_a.get(i), rows_b.get(j)) {
                (Some(&r), Some(&s)) => keyed[0].cmp(r as usize, &keyed[1], s as usize),
                (Some(_), None) => Ordering::Less,
                _ => Ordering::Greater,
            };
            match order {
                Ordering::Less => (only[0], i) = (only[0] + 1, i + 1),
                Ordering::Greater => (only[1], j) = (only[1] + 1, j + 1),
                Ordering::Equal => {
                    pairs.push((rows_a[i], rows_b[j]));
                    (i, j) = (i + 1, j + 1);
                }
            }
        }
        pairs.sort_unstable();
        joined.push(Joined { name: name.clone(), pairs, only, first: None, differ: 0 });
    }
    // Every column both hold, one at a time: a derived one worked out alone, a chunk of its inputs at a time.
    let listed = |side: &Side| -> (Vec<String>, Vec<String>) {
        let h = side.reader.header();
        let stored = h["data"]["cols"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str().map(str::to_owned)).collect();
        (stored, derived::listed(h).into_iter().map(|(name, _)| name).collect())
    };
    let ((stored_a, derived_a), (stored_b, derived_b)) = (listed(&sides[0]), listed(&sides[1]));
    let (in_a, in_b): (Vec<&String>, Vec<&String>) = (stored_a.iter().chain(&derived_a).collect(), stored_b.iter().chain(&derived_b).collect());
    for (path, mine, theirs) in [(a, &in_a, &in_b), (b, &in_b, &in_a)] {
        let alone: Vec<&str> = mine.iter().filter(|c| !theirs.contains(c)).map(|c| c.as_str()).collect();
        if !alone.is_empty() {
            said.push(format!("columns only {path} holds, not compared: {}", alone.join(", ")));
        }
    }
    let mut differs: Vec<Vec<bool>> = joined.iter().map(|j| vec![false; j.pairs.len()]).collect();
    for name in in_a.iter().filter(|c| in_b.contains(c)) {
        let read = |side: usize, path: &str| sides[side].reader.column(name).map_err(|e| rowver::plain(path, &e)).and_then(|c| Col::new(c.expect("a listed column reads"), path));
        compared(name, &read(0, a)?, &read(1, b)?, &mut joined, &mut differs, &mut said);
    }
    let by = if names.is_empty() { "their file (its ROOT UUID, and which of the sample's files with it) and entry".to_owned() } else { names.join(", ") };
    println!("Comparing the events of {a} (A) and {b} (B) by {by}:");
    let mut same = said.is_empty();
    for (join, flags) in joined.iter_mut().zip(&differs) {
        join.differ = flags.iter().filter(|f| **f).count();
        let n = |n: usize| count(n as u64);
        println!("sample {}: {} in both, {} only in A, {} only in B", join.name, some(join.pairs.len(), "event", "events"), n(join.only[0]), n(join.only[1]));
        if let Some((k, column, x, y)) = &join.first {
            let event = describe(0, join.pairs[*k].0 as usize);
            let differ = if join.differ == 1 { "1 event differs".to_owned() } else { format!("{} events differ", count(join.differ as u64)) };
            println!("  first difference: {event}, column {column}: {x} in A, {y} in B; {differ}");
        }
        same &= join.only == [0, 0] && join.differ == 0;
    }
    for line in &said {
        println!("{line}");
    }
    if same {
        println!("The two hold the same events with the same values.");
    }
    Ok(same)
}

/// Compare column `name` of A (`x`) and B (`y`) over the events both hold, flagging each that differs
/// and keeping each sample's first; a column of another kind in each is said, and not compared.
fn compared(name: &str, x: &Col, y: &Col, joined: &mut [Joined], differs: &mut [Vec<bool>], said: &mut Vec<String>) {
    let kind = |c: &Col| if c.held.offsets.is_some() { format!("a list of {}", c.held.kind) } else { c.held.kind.clone() };
    if kind(x) != kind(y) {
        said.push(format!("column {name} is {} in A and {} in B: not compared", kind(x), kind(y)));
        return;
    }
    for (join, flags) in joined.iter_mut().zip(differs) {
        for (k, &(r, s)) in join.pairs.iter().enumerate() {
            let (r, s) = (r as usize, s as usize);
            if x.value(r) == y.value(s) {
                continue;
            }
            flags[k] = true;
            if join.first.as_ref().is_none_or(|(at, ..)| k < *at) {
                let (mut one, mut other) = (x.shown(r), y.shown(s));
                if one == other {
                    (one, other) = (format!("{one} (bits {})", x.bits(r)), format!("{other} (bits {})", y.bits(s)));
                }
                join.first = Some((k, name.to_owned(), one, other));
            }
        }
    }
}
