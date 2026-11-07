//! `samples.toml`, as docs/SPEC.md §3 gives it, read on rails into the samples it makes. Nothing is
//! looked up: the files a sample asks for are resolved later, into `samples.lock`.

use super::{unknown, Problems};
use crate::choice::Events;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The keys of the top level, of `[defaults]` and of a `[[sample]]`.
const TOP: [&str; 6] = ["format", "version", "places", "defaults", "eras", "sample"];
const DEFAULTS: [&str; 6] = ["tree", "eras", "events", "choose", "seed", "metadata"];
const SAMPLE: [&str; 10] = [
    "name", "label", "files", "tree", "trees", "eras", "events", "choose", "seed", "metadata",
];

/// Whether rvr gives every sample this metadata key (a press's counts, and `file`), which samples.toml may not.
pub fn given(key: &str) -> bool {
    rvr_recipe::ADDED.contains(&key) || key == "file"
}

/// What mends a samples.toml that leaves out its header or a table: lines to write in.
pub const FORMAT: &str = "format = \"rvr-samples\"";
/// The version line a samples.toml begins with, after its format.
pub const VERSION: &str = "version = 1";
/// The tables every samples.toml has, empty or not, in their order.
pub const TABLES: [&str; 3] = ["[places]", "[defaults]", "[eras]"];

/// One sample, as samples.toml makes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// What every tool calls it: ASCII letters, digits, `_` and `-`.
    pub name: String,
    /// What plots show.
    pub label: String,
    /// Which `[[sample]]` it was made from, counting from 1.
    pub entry: usize,
    /// Its `files`, placeholders filled: paths, URLs and patterns.
    pub files: Vec<String>,
    /// The tree it reads in every file.
    pub tree: String,
    /// What it reads of its entries.
    pub events: Events,
    /// Its metadata, `{file}` still to be filled once its one file is known.
    pub metadata: Map<String, Value>,
}

/// Every sample `text` makes, in order, or every problem with it.
pub fn read(text: &str) -> Result<Vec<Sample>, Problems> {
    let doc: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| Problems::one(format!("samples.toml is not TOML: {e}")))?;
    let mut p = Problems::default();
    for key in doc.keys().filter(|key| !TOP.contains(&key.as_str())) {
        p.say(unknown("samples.toml", key, &TOP));
    }
    match doc.get("format") {
        None => p.missing.push(FORMAT),
        Some(toml::Value::String(f)) if f == "rvr-samples" => {}
        Some(_) => p.say("samples.toml's format is \"rvr-samples\": write format = \"rvr-samples\""),
    }
    match doc.get("version") {
        None => p.missing.push(VERSION),
        Some(toml::Value::Integer(1)) => {}
        Some(_) => p.say("samples.toml's version is 1: write version = 1"),
    }
    let mut table = |key: &str, line: &'static str| match doc.get(key) {
        None => {
            p.missing.push(line);
            toml::Table::new()
        }
        Some(toml::Value::Table(t)) => t.clone(),
        Some(_) => {
            p.say(format!("{line} must be a table"));
            toml::Table::new()
        }
    };
    let (places, defaults, eras) = (table("places", TABLES[0]), table("defaults", TABLES[1]), table("eras", TABLES[2]));
    let mut vars = Map::new();
    for (name, value) in &places {
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            p.say(format!("[places] {name:?}: a place's name is one or more ASCII letters, digits and _, as {{NAME}} uses it"));
        } else if name == "era" || name == "file" {
            p.say(format!("[places] {name}: {{{name}}} is rvr's (an era's name, a sample's one file): give the place another name"));
        }
        match value {
            toml::Value::String(place) => {
                vars.insert(name.clone(), Value::String(place.strip_suffix('/').unwrap_or(place).to_owned()));
            }
            _ => p.say(format!("[places] {name} must be text: a folder, a path or a URL")),
        }
    }
    for key in defaults.keys().filter(|key| !DEFAULTS.contains(&key.as_str())) {
        match key.as_str() {
            "name" | "label" | "files" | "trees" => p.say(format!("[defaults] cannot hold {key}, which belongs to one sample: give it in each [[sample]]")),
            _ => p.say(unknown("[defaults]", key, &DEFAULTS) + &metadata_hint(key)),
        }
    }
    // A default of the wrong form is said once, for [defaults], and no sample takes it.
    let mut usable = defaults.clone();
    let wrong = defaults_forms(&defaults, &eras, &mut p);
    for key in &wrong {
        usable.remove(*key);
    }
    // Each era's values, its text filled from [places] and {era}: said once, for the era.
    let mut era_values: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (era, values) in &eras {
        if !is_name(era) {
            p.say(format!("the era {era:?} must be a name (ASCII letters, digits, _ and -), since it becomes part of sample names: try {:?}", as_name(era)));
        }
        let place = format!("[eras.{era}]");
        let toml::Value::Table(values) = values else {
            p.say(format!("{place} must be a table of the era's metadata"));
            continue;
        };
        metadata(values, &place, &mut p);
        let mut known = vars.clone();
        known.insert("era".into(), Value::String(era.clone()));
        let mut filled_in = Map::new();
        for (key, value) in values {
            if vars.contains_key(key) {
                p.say(format!("{place} {key}: a place has this name too, so {{{key}}} would mean two things: name one of them otherwise"));
            }
            // One that cannot be filled is said here, and kept as written so its uses say nothing more.
            let value = filled(value, &known, &place, &mut p).unwrap_or_else(|| json(value));
            filled_in.insert(key.clone(), value);
        }
        era_values.insert(era.clone(), filled_in);
    }
    metadata_of(&defaults, "[defaults]", &mut p);
    let entries = match doc.get("sample") {
        Some(toml::Value::Array(list)) if !list.is_empty() => list.clone(),
        Some(toml::Value::Array(_)) | None => {
            p.say("samples.toml describes no sample: add one, [[sample]] with name = \"…\", files = \"…\" and tree = \"…\"");
            Vec::new()
        }
        Some(_) => {
            p.say("samples are written [[sample]], with two brackets, one per sample");
            Vec::new()
        }
    };
    let mut samples = Vec::new();
    for (index, body) in entries.iter().enumerate() {
        match body {
            toml::Value::Table(body) => expand(body, index + 1, (&usable, &wrong), &era_values, &vars, &mut samples, &mut p),
            _ => p.say(format!("sample {} must be a table", index + 1)),
        }
    }
    // A default seed is used by a random sample that gives none of its own.
    let own_seed = |s: &Sample| entries[s.entry - 1].as_table().is_some_and(|body| body.contains_key("seed"));
    if p.said.is_empty() && defaults.contains_key("seed") && !samples.is_empty() && !samples.iter().any(|s| matches!(s.events, Events::Random { .. }) && !own_seed(s)) {
        p.say("[defaults] gives seed, but no sample uses it (none chooses at random without its own seed): take it out");
    }
    let (mut names, mut labels) = (BTreeSet::new(), BTreeSet::new());
    for sample in &samples {
        if !names.insert(sample.name.as_str()) {
            p.say(format!("two samples are named {:?}: give one of them another name, or other eras or trees", sample.name));
        }
        if !labels.insert(sample.label.as_str()) {
            p.say(format!("two samples are labelled {:?}, so plots could not tell them apart: give one of them another label", sample.label));
        }
    }
    p.or(samples)
}

/// The samples one `[[sample]]` makes: one per era and per tree; `defaults` without the keys of
/// the wrong form, `wrong`, which were said for [defaults].
fn expand(
    body: &toml::Table,
    entry: usize,
    (defaults, wrong): (&toml::Table, &[&str]),
    eras: &BTreeMap<String, Map<String, Value>>,
    places: &Map<String, Value>,
    out: &mut Vec<Sample>,
    p: &mut Problems,
) {
    let said = p.said.len();
    let base = match body.get("name") {
        Some(toml::Value::String(name)) => name.clone(),
        Some(_) => {
            p.say(format!("sample {entry}: name must be text, like name = \"ST_s-channel\""));
            return;
        }
        None => {
            p.say(format!("sample {entry} has no name: give it one, like name = \"ST_s-channel\" (and label = \"ST s-channel\" for plots)"));
            return;
        }
    };
    let place = format!("sample {base:?}");
    for key in body.keys().filter(|key| !SAMPLE.contains(&key.as_str())) {
        p.say(unknown(&place, key, &SAMPLE) + &metadata_hint(key));
    }
    if !is_name(&base) {
        p.say(format!("{place}: a name holds only ASCII letters, digits, _ and -, since tools use it in commands and file names: use name = {:?}, and put the text in label", as_name(&base)));
    }
    let label = match body.get("label") {
        None => base.clone(),
        Some(toml::Value::String(label)) if !label.is_empty() => label.clone(),
        Some(_) => {
            p.say(format!("{place}: label must be nonempty text"));
            base.clone()
        }
    };
    let setting = |key: &str| body.get(key).or_else(|| defaults.get(key));
    let files: Vec<String> = match body.get("files") {
        Some(toml::Value::String(f)) if !f.is_empty() => vec![f.clone()],
        Some(toml::Value::Array(list)) if !list.is_empty() && list.iter().all(|f| f.as_str().is_some_and(|f| !f.is_empty())) => {
            list.iter().filter_map(|f| f.as_str().map(str::to_owned)).collect()
        }
        Some(_) => {
            p.say(format!("{place}: files is a path or URL, or a nonempty list of them"));
            Vec::new()
        }
        None => {
            p.say(format!("{place} has no files: say where its events are, files = \"…\" (paths, URLs or patterns)"));
            Vec::new()
        }
    };
    let trees: Vec<(Option<String>, String)> = match (body.get("tree"), body.get("trees")) {
        (Some(_), Some(_)) => {
            p.say(format!("{place}: give tree or trees, not both"));
            Vec::new()
        }
        (None, Some(toml::Value::Table(points))) if !points.is_empty() => points
            .iter()
            .filter_map(|(point, tree)| match tree.as_str() {
                Some(tree) if !tree.is_empty() => Some((Some(point.clone()), tree.to_owned())),
                _ => {
                    p.say(format!("{place}: trees maps a label to a tree's name, like trees = {{ \"300/250\" = \"SMS_30000_25000\" }}"));
                    None
                }
            })
            .collect(),
        (None, Some(_)) => {
            p.say(format!("{place}: trees maps a label to a tree's name, like trees = {{ \"300/250\" = \"SMS_30000_25000\" }}"));
            Vec::new()
        }
        (tree, None) => match tree.or_else(|| defaults.get("tree")) {
            Some(toml::Value::String(tree)) if !tree.is_empty() => vec![(None, tree.clone())],
            Some(_) => {
                p.say(format!("{place}: tree is the name of a tree, like tree = \"Events\""));
                Vec::new()
            }
            None if wrong.contains(&"tree") => Vec::new(),
            None => {
                p.say(format!("{place} has no tree: say which tree to read, tree = \"Events\", here or in [defaults]"));
                Vec::new()
            }
        },
    };
    for (point, _) in &trees {
        if point.as_deref().is_some_and(|point| as_name(point).is_empty()) {
            p.say(format!("{place}: the trees label {:?} makes no name: give it letters or digits", point.as_deref().unwrap_or("")));
        }
    }
    let era_list: Vec<Option<String>> = match setting("eras") {
        None => vec![None],
        Some(toml::Value::Array(list)) if !list.is_empty() => list
            .iter()
            .filter_map(|era| match era.as_str() {
                Some(era) if eras.contains_key(era) => Some(Some(era.to_owned())),
                Some(era) => {
                    let known: Vec<&str> = eras.keys().map(String::as_str).collect();
                    p.say(format!("{place}: the era {era:?} is not in [eras], which has {}", if known.is_empty() { "none".into() } else { known.join(", ") }));
                    None
                }
                None => {
                    p.say(format!("{place}: eras is a list of era names, like eras = [\"UL17\", \"UL18\"]"));
                    None
                }
            })
            .collect(),
        Some(_) => {
            p.say(format!("{place}: eras is a nonempty list of era names, like eras = [\"UL17\", \"UL18\"]"));
            Vec::new()
        }
    };
    let events = events(setting("events"), setting("choose"), setting("seed"), body.contains_key("seed"), &place, p);
    metadata_of(body, &place, p);
    if p.said.len() > said {
        return;
    }
    let empty = toml::Table::new();
    let mine = |table: &toml::Table| match table.get("metadata") {
        Some(toml::Value::Table(metadata)) => metadata.clone(),
        _ => empty.clone(),
    };
    for era in &era_list {
        // An era's values, filled once, are what files and metadata read alike.
        let mut vars = places.clone();
        let mut metadata = Map::new();
        if let Some(era) = era {
            let values = eras.get(era).cloned().unwrap_or_default();
            vars.extend(values.clone());
            metadata.extend(values);
            vars.insert("era".into(), Value::String(era.clone()));
        }
        let files: Vec<String> = files.iter().filter_map(|f| fill(f, &vars, false, &place, p)).collect();
        for file in &files {
            reachable(file, &place, p);
        }
        // Merged first, so that a default text the sample replaces is never filled for it.
        let mut merged = mine(defaults);
        merged.extend(mine(body));
        for (key, value) in merged {
            if let Some(value) = filled(&value, &vars, &place, p) {
                metadata.insert(key, value);
            }
        }
        for (point, tree) in &trees {
            let (mut name, mut shown) = (base.clone(), label.clone());
            if let Some(point) = point {
                name = format!("{name}_{}", as_name(point));
                shown = format!("{shown} {point}");
            }
            if let Some(era) = era {
                name = format!("{name}_{era}");
                shown = format!("{shown} ({era})");
            }
            out.push(Sample { name, label: shown, entry, files: files.clone(), tree: tree.clone(), events, metadata: metadata.clone() });
        }
    }
}

/// Check that each value of `[defaults]` has its form, even when every sample gives its own: the
/// keys that do not.
fn defaults_forms(defaults: &toml::Table, eras: &toml::Table, p: &mut Problems) -> Vec<&'static str> {
    use toml::Value::{Array, Integer, String as Text};
    let wrong = |key: &str| match defaults.get(key) {
        None => false,
        Some(value) => !match key {
            "tree" => matches!(value, Text(t) if !t.is_empty()),
            "eras" => matches!(value, Array(list) if !list.is_empty() && list.iter().all(|e| e.as_str().is_some_and(|e| eras.contains_key(e)))),
            "events" => matches!(value, Text(t) if t == "all") || matches!(value, Integer(n) if *n >= 1),
            "choose" => matches!(value, Text(t) if t == "first" || t == "random"),
            _ => matches!(value, Integer(n) if *n >= 0),
        },
    };
    let forms = [
        ("tree", "the name of a tree, like tree = \"Events\""),
        ("eras", "a nonempty list of names in [eras], like eras = [\"UL17\", \"UL18\"]"),
        ("events", "a number of entries to read, from 1, like events = 50_000, or \"all\""),
        ("choose", "\"first\" or \"random\""),
        ("seed", "a whole number from 0, like seed = 7"),
    ];
    let wrong: Vec<(&str, &str)> = forms.into_iter().filter(|(key, _)| wrong(key)).collect();
    for (key, form) in &wrong {
        p.say(format!("[defaults]: {key} is {form}"));
    }
    wrong.into_iter().map(|(key, _)| key).collect()
}

/// For a key no sample or default has: where metadata goes.
fn metadata_hint(key: &str) -> String {
    format!(" Metadata goes in its own table: metadata = {{ {key} = … }}.")
}

/// A sample's event choice, from its `events`, `choose` and `seed` (or `[defaults]`'s).
fn events(events: Option<&toml::Value>, choose: Option<&toml::Value>, seed: Option<&toml::Value>, seeded: bool, place: &str, p: &mut Problems) -> Events {
    let n = match events {
        None => None,
        Some(toml::Value::String(all)) if all == "all" => None,
        Some(toml::Value::Integer(n)) if *n >= 1 => Some(*n as u64),
        Some(_) => {
            p.say(format!("{place}: events is a number of entries to read, from 1, like events = 50_000, or \"all\""));
            None
        }
    };
    let random = match choose {
        None => false,
        Some(toml::Value::String(c)) if c == "first" => false,
        Some(toml::Value::String(c)) if c == "random" => true,
        Some(_) => {
            p.say(format!("{place}: choose is \"first\" (the first entries, in the files' order) or \"random\""));
            return Events::All;
        }
    };
    let seed = match seed {
        None => 1,
        Some(toml::Value::Integer(s)) if *s >= 0 => *s as u64,
        Some(_) => {
            p.say(format!("{place}: seed is a whole number from 0, like seed = 7"));
            1
        }
    };
    match (random, n) {
        (false, _) if seeded => {
            p.say(format!("{place}: seed is given, but its events are not chosen at random: add choose = \"random\", or take seed out"));
            Events::All
        }
        (false, None) => Events::All,
        (false, Some(n)) => Events::First(n),
        (true, Some(n)) => Events::Random { n, seed },
        (true, None) => {
            p.say(format!("{place}: choose = \"random\" needs a number of entries to choose, like events = 50_000"));
            Events::All
        }
    }
}

/// Say so when a `files` entry is neither a local path nor a URL of a form rvr reads.
fn reachable(file: &str, place: &str, p: &mut Problems) {
    if file.contains("://") && !["root://", "davs://", "https://"].iter().any(|s| file.starts_with(s)) {
        p.say(format!("{place}: {file:?} is not a form rvr reads: a local path, or a URL starting root://, davs:// or https://"));
    }
    let query = (file.starts_with("https://") || file.starts_with("davs://")) && file.find('?').is_some_and(|at| file[at..].contains('='));
    if query {
        p.say(format!("{place}: {file} has a query: rvr names a storage file by its path: drop the query; storage access goes by your grid proxy"));
        return;
    }
    if let Some(rest) = file.strip_prefix("root://").filter(|rest| !rest.find('/').is_some_and(|at| rest[at..].starts_with("//"))) {
        let written = match rest.split_once('/') {
            Some((host, path)) => format!("root://{host}//{path}"),
            None => format!("root://{rest}//path/file.root"),
        };
        p.say(format!("{place}: {file:?} has one slash before its path, and a grid storage file is written root://host[:port]//path, with two slashes before the path: write it {written:?}"));
    }
    let (folder, name) = super::split(file);
    if ["", ".", "..", "~"].contains(&name) {
        let pattern = match name.is_empty() {
            true => format!("{file}*.root"),
            false => format!("{file}/*.root"),
        };
        p.say(format!("{place}: {file:?} names a folder: name its files, or a pattern of them, such as {pattern:?}"));
    }
    if folder.contains(['*', '?']) {
        p.say(format!("{place}: {file}: its folder, {folder}, holds * or ?, which rvr reads only in a file's name (a pattern), never in a folder: rename the folder, or link it under a name without them, and write that name"));
    }
}

/// Check the `metadata` table of `table`, when it has one.
fn metadata_of(table: &toml::Table, place: &str, p: &mut Problems) {
    match table.get("metadata") {
        None => {}
        Some(toml::Value::Table(values)) => metadata(values, &format!("{place} metadata"), p),
        Some(_) => p.say(format!("{place}: metadata is a table, like metadata = {{ kind = \"signal\", xsec = 2.5 }}")),
    }
}

/// Check metadata: each key a name a recipe reads as `sample.KEY`, not one rvr gives; each value
/// text, a whole number, a finite float, a truth value, or a list or table of these.
pub(super) fn metadata(values: &toml::Table, place: &str, p: &mut Problems) {
    for (key, value) in values {
        if !rvr_recipe::plain(key) || key == "sample" {
            p.say(format!("{place}: {key:?} is not a name a recipe can read as sample.{key}: use a letter or _, then letters, digits and _"));
        } else if given(key) {
            p.say(format!("{place}: {key} is given by rvr to every sample: name yours otherwise"));
        }
        let place = format!("{place} {key}");
        if value_ok(value, &place, p) > NESTING {
            p.say(format!("{place}: its lists and tables nest more than {NESTING} deep, more than a .rvr header holds: nest them less in samples.toml, then resolve again"));
        }
    }
}

/// The lists and tables a metadata value may nest: a sample's metadata lies 5 deep in a header of
/// at most `DEEPEST` levels (docs/FORMAT.md §1).
const NESTING: usize = crate::json::DEEPEST - 5;

/// Say what in `value` is not metadata; how many lists and tables it nests, itself among them.
fn value_ok(value: &toml::Value, place: &str, p: &mut Problems) -> usize {
    match value {
        toml::Value::Datetime(_) => p.say(format!("{place}: a date or time is not metadata: write it as text, in quotes")),
        toml::Value::Float(f) if !f.is_finite() => p.say(format!("{place}: {f} is not a finite number")),
        toml::Value::Array(list) => return 1 + list.iter().map(|v| value_ok(v, place, p)).max().unwrap_or(0),
        toml::Value::Table(table) => {
            if table.contains_key(crate::json::NUMBER) {
                p.say(format!("{place}: {:?} is a key no .rvr header holds, as the JSON reader takes it for a number: name it otherwise", crate::json::NUMBER));
            }
            return 1 + table.values().map(|v| value_ok(v, place, p)).max().unwrap_or(0);
        }
        _ => {}
    }
    0
}

/// Whether text can be a sample's or era's name: ASCII letters, digits, `_` and `-`, at least one.
pub(crate) fn is_name(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Text made into a name, as CMS dataset names write numbers: `.` becomes `p`, every other run of
/// characters a name cannot hold becomes one `_`, and `_` is trimmed from both ends. "300/292.5"
/// is "300_292p5"; "ST s-channel (13.6 TeV)" is "ST_s-channel_13p6_TeV".
pub(crate) fn as_name(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
        } else if c == '.' {
            out.push('p');
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// `text` with its `{NAME}` placeholders filled from `vars` (and `{file}` kept, in metadata, for
/// resolve to fill), or `None` with the problem said.
fn fill(text: &str, vars: &Map<String, Value>, metadata: bool, place: &str, p: &mut Problems) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let open = rest.find('{');
        let close = rest.find('}');
        match (open, close) {
            (None, None) => break,
            (Some(_), None) => {
                p.say(format!("{place}: a '{{' is never closed in {text:?}"));
                return None;
            }
            (Some(open), Some(close)) if open < close => {
                let close = open + rest[open..].find('}').unwrap_or(0);
                out.push_str(&rest[..open]);
                let key = &rest[open + 1..close];
                match vars.get(key) {
                    Some(Value::String(s)) => out.push_str(s),
                    Some(Value::Number(n)) if n.is_i64() || n.is_u64() => out.push_str(&n.to_string()),
                    Some(_) => {
                        p.say(format!("{place}: {{{key}}} is not text or a whole number, so it cannot be put into {text:?}"));
                        return None;
                    }
                    None if metadata && key == "file" => out.push_str("{file}"),
                    None => {
                        let known: Vec<String> = vars.keys().map(|k| format!("{{{k}}}")).collect();
                        p.say(format!("{place}: {{{key}}} is not defined here; defined: {}", if known.is_empty() { "nothing".into() } else { known.join(", ") }));
                        return None;
                    }
                }
                rest = &rest[close + 1..];
            }
            _ => {
                p.say(format!("{place}: a '}}' with no '{{' before it in {text:?}"));
                return None;
            }
        }
    }
    out.push_str(rest);
    Some(out)
}

/// A metadata value with every text in it filled, as `fill` fills one, or `None` with the problem said.
fn filled(value: &toml::Value, vars: &Map<String, Value>, place: &str, p: &mut Problems) -> Option<Value> {
    match value {
        toml::Value::String(text) => fill(text, vars, true, place, p).map(Value::String),
        toml::Value::Array(list) => list.iter().map(|v| filled(v, vars, place, p)).collect::<Option<_>>().map(Value::Array),
        toml::Value::Table(table) => table.iter().map(|(k, v)| Some((k.clone(), filled(v, vars, place, p)?))).collect::<Option<_>>().map(Value::Object),
        other => Some(json(other)),
    }
}

/// A sample's metadata with each text's `{file}` the name of its one file of `files`; `None` when there is not one.
pub fn file_filled(metadata: &Map<String, Value>, files: &[&str]) -> Option<Map<String, Value>> {
    fn fill(value: &mut Value, name: Option<&str>) -> bool {
        match value {
            Value::String(text) if text.contains("{file}") => match name {
                Some(name) => {
                    *text = text.replace("{file}", name);
                    true
                }
                None => false,
            },
            Value::Array(list) => list.iter_mut().all(|v| fill(v, name)),
            Value::Object(map) => map.values_mut().all(|v| fill(v, name)),
            _ => true,
        }
    }
    let name = match files {
        [one] => Some(one.rsplit('/').next().unwrap_or(one)),
        _ => None,
    };
    let mut out = metadata.clone();
    out.values_mut().all(|v| fill(v, name)).then_some(out)
}

fn json(value: &toml::Value) -> Value {
    serde_json::to_value(value).expect("TOML values are JSON values")
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    const HEAD: &str = "format = \"rvr-samples\"\nversion = 1\n[places]\n[defaults]\n[eras]\n";

    fn problems(text: &str) -> String {
        read(text).unwrap_err().to_string()
    }

    // SPEC 3.2.2, 3.2.3, 3.4.2, 3.5.5, 3.5.6, 3.5.7, 3.6.1, 3.6.3, 3.6.4, 3.7.1, 3.7.2, 3.7.3
    #[test]
    fn eras_and_trees_make_samples_named_and_labelled_by_rule() {
        let text = format!(
            "{}{}",
            HEAD.replace("[places]\n", "[places]\nlpc = \"root://cmseos.fnal.gov//store/x/\"\n").replace("[eras]\n", ""),
            "[eras.UL17]\nera = 2\n[eras.UL18]\nera = 3\n\
             [[sample]]\nname = \"Signal\"\nfiles = \"{lpc}/{era}/s_{era}.root\"\neras = [\"UL17\", \"UL18\"]\n\
             trees = { \"300/292.5\" = \"SMS_300\", \"400/300\" = \"SMS_400\" }\nmetadata = { kind = \"signal\", tag = \"{era}-{file}\" }\n",
        );
        let samples = read(&text).unwrap();
        let names: Vec<&str> = samples.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Signal_300_292p5_UL17", "Signal_400_300_UL17", "Signal_300_292p5_UL18", "Signal_400_300_UL18"]);
        assert_eq!(samples[0].label, "Signal 300/292.5 (UL17)");
        assert_eq!(samples[0].files, ["root://cmseos.fnal.gov//store/x/UL17/s_UL17.root"]);
        assert_eq!(samples[3].metadata["era"], 3);
        assert_eq!(samples[3].metadata["tag"], "UL18-{file}");
        let nested = read(&format!("{HEAD}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nmetadata = {{ tags = [\"{{nowhere}}\"] }}")).unwrap_err();
        assert!(nested.to_string().contains("{nowhere} is not defined"), "{nested}");
        let one = json!({"note": {"f": ["{file}", 2]}, "k": "{file}!"});
        let filled = file_filled(one.as_object().unwrap(), &["root://h//d/x.root"]).unwrap();
        assert_eq!(Value::Object(filled), json!({"note": {"f": ["x.root", 2]}, "k": "x.root!"}));
        assert!(file_filled(one.as_object().unwrap(), &["a.root", "b.root"]).is_none());
        assert_eq!(samples[0].events, Events::All);
        let placed = HEAD.replace("[places]\n", "[places]\np = \"/data\"\n").replace("[eras]\n", "[eras.E1]\nfolder = \"{p}/d\"\n");
        let made = read(&format!("{placed}[[sample]]\nname = \"a\"\nfiles = \"{{folder}}/x.root\"\ntree = \"T\"\neras = [\"E1\"]")).unwrap();
        assert_eq!((made[0].files[0].as_str(), &made[0].metadata["folder"]), ("/data/d/x.root", &json!("/data/d")));
        let by_era = read(&format!("{}[[sample]]\nname = \"a\"\nfiles = \"{{folder}}/x.root\"\ntree = \"T\"\neras = [\"E1\"]", placed.replace("{p}/d", "{p}/S{era}"))).unwrap();
        assert_eq!(by_era[0].files[0], "/data/SE1/x.root");
        let clash = problems(&format!("{}[[sample]]\nname = \"a\"\nfiles = \"{{folder}}/x.root\"\ntree = \"T\"\neras = [\"E1\"]", placed.replace("folder = ", "p = \"x\"\nfolder = ")));
        assert!(clash.contains("[eras.E1] p: a place has this name too"), "{clash}");
        let undefined = problems(&format!("{}[[sample]]\nname = \"a\"\nfiles = \"{{folder}}/x.root\"\ntree = \"T\"\neras = [\"E1\"]", placed.replace("{p}/d", "{q}/d")));
        assert_eq!(undefined.lines().count(), 1, "{undefined}");
        // A default text a sample replaces is not filled for it; one it takes is (SPEC 3.3.2).
        let noted = HEAD.replace("[defaults]\n", "[defaults]\nmetadata = { note = \"{campaign}\", n = 1 }\n");
        let own = read(&format!("{noted}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nmetadata = {{ note = \"no era\" }}")).unwrap();
        assert_eq!(Value::Object(own[0].metadata.clone()), json!({"note": "no era", "n": 1}));
        assert!(problems(&format!("{noted}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"")).contains("{campaign} is not defined"));
        // Metadata nests as deep as the header holds, and no deeper (SPEC 3.5.10).
        let nested = |n: usize| format!("{HEAD}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nmetadata = {{ deep = {}1{} }}", "[".repeat(n), "]".repeat(n));
        assert!(read(&nested(59)).is_ok());
        assert!(problems(&nested(60)).contains("metadata deep: its lists and tables nest more than 59 deep, more than a .rvr header holds: nest them less"), "{}", problems(&nested(60)));
        // Said once for the key, however many of its lists go too deep.
        let deep = format!("{}1{}", "[".repeat(60), "]".repeat(60));
        let siblings = problems(&format!("{HEAD}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nmetadata = {{ deep = [{deep}, {deep}, {deep}] }}"));
        assert_eq!(siblings.matches("nest more than 59 deep").count(), 1, "{siblings}");
    }

    // SPEC 1.4, 1.5, 3.1.3, 3.1.4, 3.2.1, 3.3.1, 3.3.2, 3.5.1, 3.5.2, 3.5.4, 3.5.5, 3.5.7, 3.5.8, 3.5.9, 3.5.10, 3.6.4, 3.7.2, 3.7.4, 3.8.1, 3.8.2, 3.8.5
    #[test]
    fn every_problem_is_said_and_what_is_left_out_is_offered() {
        let said = read("[[sample]]\nname = \"a\"\nfiles = \"x.root\"\ntree = \"T\"\nkind = \"mc\"\nchoice = \"random\"\n").unwrap_err();
        assert_eq!(said.missing, [FORMAT, VERSION, TABLES[0], TABLES[1], TABLES[2]]);
        assert_eq!(said.said.len(), 2, "{said}");
        assert!(said.to_string().contains("has no key \"choice\"") && said.to_string().contains("Did you mean: choose?"), "{said}");
        let p = |body: &str| problems(&format!("{HEAD}[[sample]]\n{body}"));
        assert!(p("name = \"a b\"\nfiles = \"x\"\ntree = \"T\"").contains("use name = \"a_b\""));
        assert!(p("name = \"a\"\ntree = \"T\"").contains("has no files"));
        assert!(p("name = \"a\"\nfiles = \"x\"").contains("has no tree"));
        assert!(p("name = \"a\"\nfiles = \"x\"\ntree = \"T\"\ntrees = { a = \"T\" }").contains("not both"));
        assert!(p("name = \"a\"\nfiles = \"x\"\ntree = \"T\"\nseed = 3").contains("not chosen at random"));
        let seeded = HEAD.replace("[defaults]\n", "[defaults]\nevents = 10\nchoose = \"random\"\nseed = 7\n");
        let first = read(&format!("{seeded}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nchoose = \"first\"\n[[sample]]\nname = \"b\"\nfiles = \"y\"\ntree = \"T\"")).unwrap();
        assert_eq!(first.iter().map(|s| s.events).collect::<Vec<_>>(), [Events::First(10), Events::Random { n: 10, seed: 7 }]);
        assert!(problems(&format!("{seeded}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nchoose = \"first\"")).contains("no sample uses it"));
        assert!(problems(&format!("{seeded}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nseed = 3")).contains("no sample uses it"));
        assert!(!problems(&format!("{seeded}[[sample]]\nname = \"a\"\nfiles = \"x\"")).contains("no sample uses it"));
        let typo = p("name = \"a\"\nfiles = \"x\"\ntree = \"T\"\nevents = 10\nchoose = \"randm\"\nseed = 3");
        assert!(typo.contains("choose is") && !typo.contains("seed is given"), "{typo}");
        assert!(p("name = \"a\"\nfiles = \"x\"\ntree = \"T\"\nevents = 0").contains("from 1"));
        assert!(p("name = \"a\"\nfiles = \"x\"\ntree = \"T\"\nmetadata = { when = 2026-11-02 }").contains("date or time"));
        assert!(p("name = \"a\"\nfiles = \"x\"\ntree = \"T\"\nmetadata = { events_read = 1 }").contains("given by rvr"));
        assert!(p("name = \"a\"\nfiles = \"{nowhere}/x\"\ntree = \"T\"").contains("{nowhere} is not defined"));
        assert!(p("name = \"a\"\nfiles = \"x}\"\ntree = \"T\"").contains("no '{' before it"));
        let url = p("name = \"a\"\nfiles = \"xroot://h//x.root\"\ntree = \"T\"");
        assert!(url.contains("not a form rvr reads") && !url.contains("nonempty list"), "{url}");
        assert!(p("name = \"a\"\nfiles = \"d*/x.root\"\ntree = \"T\"").contains("d*/x.root: its folder, d*, holds * or ?, which rvr reads only in a file's name (a pattern), never in a folder: rename the folder"));
        let query = p("name = \"a\"\nfiles = \"https://h.org/d/x.root?authz=a/b\"\ntree = \"T\"");
        assert!(query.contains("has a query: rvr names a storage file by its path: drop the query") && !query.contains("its folder"), "{query}");
        assert!(read(&format!("{HEAD}[[sample]]\nname = \"a\"\nfiles = \"https://h.org/d/x?.root\"\ntree = \"T\"")).is_ok(), "a ? in the file name is a pattern");
        let placed = problems(&format!("{}[[sample]]\nname = \"a\"\nfiles = \"{{x}}/f.root\"\ntree = \"T\"", HEAD.replace("[places]\n", "[places]\nx = \"xroot://h//d\"\n")));
        assert!(placed.contains("\"xroot://h//d/f.root\" is not a form rvr reads"), "{placed}");
        assert!(problems(&HEAD.replace("[places]\n", "[places]\nfile = \"x\"\n")).contains("{file} is rvr's"));
        assert!(problems(&HEAD.replace("[places]\n", "[places]\n\"\" = \"x\"\n")).contains("one or more ASCII"));
        assert!(p("name = \"a\"\nfiles = \"x\"\ntrees = { \"///\" = \"T\" }").contains("makes no name"));
        assert!(problems(&format!("{}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\n[[sample]]\nname = \"a\"\nfiles = \"y\"\ntree = \"T\"", HEAD)).contains("two samples are named"));
        assert!(problems(&HEAD.replace("[defaults]\n", "[defaults]\nlabel = \"x\"\n")).contains("belongs to one sample"));
        let bad = HEAD.replace("[defaults]\n", "[defaults]\ntree = 5\nevents = -3\nchoose = \"x\"\n");
        let said = problems(&format!("{bad}[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\nevents = 2\nchoose = \"first\""));
        assert!(said.contains("[defaults]: tree is") && said.contains("[defaults]: events is") && said.contains("[defaults]: choose is"), "{said}");
        let inherited = problems(&format!("{bad}[[sample]]\nname = \"a\"\nfiles = \"x\"\n[[sample]]\nname = \"b\"\nfiles = \"y\""));
        assert_eq!(inherited.lines().count(), 3, "{inherited}");
    }
}
