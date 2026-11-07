//! `samples.lock`, as docs/FORMAT.md §6.3 gives it: every file a study reads, pinned by its size,
//! adler32, ROOT UUID and entries; each sample's branch list; what a press reads of each file; read
//! on rails, checked, written, and checked against the `samples.toml` it was resolved from.

use super::samples;
use crate::choice::Events;
use super::{hex, utc, Problems};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};


/// The most entries a sample's files may hold in all, so a random choice is made again in bounded time.
pub const MOST_ENTRIES: u64 = 1 << 40;

/// The entries a sample's files hold in all, or, past `MOST_ENTRIES`, what is wrong and its fix.
pub fn entries_in_all(entries: &[u64], whose: &str) -> std::result::Result<u64, String> {
    match entries.iter().try_fold(0_u64, |sum, e| sum.checked_add(*e)) {
        Some(total) if total <= MOST_ENTRIES => Ok(total),
        _ if entries.iter().any(|&e| e > MOST_ENTRIES) => Err(format!("one of {whose} files holds more than 2^40 entries, more than a sample of a .rvr file may: write that file again as smaller ones and split them among samples")),
        _ => Err(format!("{whose} files hold more than 2^40 entries in all, more than a sample of a .rvr file may: split them among samples of fewer files")),
    }
}

/// Every file a study reads, pinned.
#[derive(Debug, Clone, PartialEq)]
pub struct Lock {
    /// When it was written, `YYYY-MM-DDTHH:MM:SSZ`.
    pub resolved: String,
    /// The SHA-256 of the samples.toml text it was resolved from.
    pub samples_toml_sha256: String,
    /// Every sample, in samples.toml's order.
    pub samples: Vec<Sample>,
    /// Each distinct branch list, once.
    pub branches: Vec<Branches>,
}

/// One sample, as samples.toml made it, with its files.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// What every tool calls it.
    pub name: String,
    /// What plots show.
    pub label: String,
    /// Its `files`, placeholders filled.
    pub asked: Vec<String>,
    /// The tree it reads in every file.
    pub tree: String,
    /// The SHA-256 of its tree's branch list (its first file's; every file of it has it).
    pub branches: String,
    /// What it reads of its entries.
    pub events: Events,
    /// The entries of its files, in all.
    pub events_total: u64,
    /// The entries its event choice reads.
    pub events_read: u64,
    /// Its metadata, `{file}` filled.
    pub metadata: Map<String, Value>,
    /// Every file it reads, in order.
    pub files: Vec<File>,
}

/// One file, pinned, with what a press reads of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Where it is: a local path (relative to samples.toml's folder, under `~/`, or absolute), or a URL.
    pub at: String,
    /// Its size.
    pub bytes: u64,
    /// 8 lowercase hex digits, when its storage gave one.
    pub adler32: Option<String>,
    /// Its ROOT UUID, 8-4-4-4-12 lowercase hex.
    pub uuid: String,
    /// The entries of its tree.
    pub entries: u64,
    /// For the first N: the entries read, when fewer than `entries`.
    pub stop: Option<u64>,
    /// For a random choice: how many chosen entries lie in it.
    pub take: Option<u64>,
}

/// A tree's branch list, by its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branches {
    /// The SHA-256 of the list's canonical JSON.
    pub sha256: String,
    /// Its branches, in byte order of name.
    pub list: Vec<Branch>,
}

/// One branch: its name as a recipe writes it, the kind rvr reads it as (`none`, with why), and its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Its name, as a recipe writes it.
    pub name: String,
    /// A `.rvr` kind (`f4`, `i8`, `b1` …), or `none`.
    pub kind: String,
    /// One value per event, a list, or N values.
    pub shape: Shape,
    /// Why rvr cannot read it, for kind `none`.
    pub why: Option<String>,
}

/// How many values a branch holds per event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One value per event.
    Flat,
    /// A list per event.
    Jagged,
    /// N values per event (`pv[3]/F`).
    Fixed(u64),
    /// A list per event of elements of N values each (`xy[n][3]/F`).
    JaggedFixed(u64),
}

impl Shape {
    /// The shape as samples.lock writes it: "flat", "jagged", N, or "jagged N".
    fn value(self) -> Value {
        match self {
            Shape::Flat => json!("flat"),
            Shape::Jagged => json!("jagged"),
            Shape::Fixed(n) => json!(n),
            Shape::JaggedFixed(n) => json!(format!("jagged {n}")),
        }
    }

    /// The shape a text of samples.lock names, written exactly so.
    fn named(text: &str) -> Option<Shape> {
        match text {
            "flat" => Some(Shape::Flat),
            "jagged" => Some(Shape::Jagged),
            _ => {
                let n: u64 = text.strip_prefix("jagged ")?.parse().ok()?;
                (n >= 2 && text == format!("jagged {n}")).then_some(Shape::JaggedFixed(n))
            }
        }
    }
}

impl std::fmt::Display for Shape {
    /// "flat", "jagged", "N per event" or "jagged, N per element".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Shape::Flat => f.write_str("flat"),
            Shape::Jagged => f.write_str("jagged"),
            Shape::Fixed(n) => write!(f, "{n} per event"),
            Shape::JaggedFixed(n) => write!(f, "jagged, {n} per element"),
        }
    }
}

/// What a press reads of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Read {
    /// Entries read.
    pub read: u64,
    /// For the first N: entries 0..stop, when fewer than the file has.
    pub stop: Option<u64>,
    /// For a random choice: how many of the chosen entries are in the file.
    pub take: Option<u64>,
}

/// What a press reads of each file, given each file's entries.
pub(crate) fn reads(entries: &[u64], events: Events) -> Vec<Read> {
    let total: u64 = entries.iter().sum();
    match events {
        Events::All => entries.iter().map(|&e| Read { read: e, stop: None, take: None }).collect(),
        Events::First(n) => {
            let mut left = n;
            entries
                .iter()
                .map(|&e| {
                    let read = e.min(left);
                    left -= read;
                    Read { read, stop: (read < e).then_some(read), take: None }
                })
                .collect()
        }
        Events::Random { n, seed } => crate::choice::counts(entries, n.min(total), seed)
            .into_iter()
            .map(|take| Read { read: take, stop: None, take: Some(take) })
            .collect(),
    }
}

impl Sample {
    /// Why a file's `take` is not its share (`counts`) of the sample's random choice, if one is not.
    pub fn takes(&self, counts: &[u64]) -> Result<(), String> {
        for (file, count) in self.files.iter().zip(counts) {
            if file.take != Some(*count) {
                let take = file.take.map_or("no take".to_owned(), |t| format!("take {t}"));
                return Err(format!("sample {}'s file {} has {take}, and its random choice takes {count} of it (docs/FORMAT.md §6.3): the lock was edited, or written by something other than rvr samples resolve; resolve again", self.name, file.at));
            }
        }
        Ok(())
    }

    /// The sample as a press with `events` reads it: each file's stop or take, its events_read and events_total.
    pub fn planned(&self, events: Events) -> Sample {
        let entries: Vec<u64> = self.files.iter().map(|f| f.entries).collect();
        let reads = reads(&entries, events);
        let mut sample = self.clone();
        sample.events = events;
        for (file, read) in sample.files.iter_mut().zip(&reads) {
            (file.stop, file.take) = (read.stop, read.take);
        }
        sample.events_total = entries.iter().sum();
        sample.events_read = reads.iter().map(|r| r.read).sum();
        sample
    }
}

impl Branches {
    /// A branch list in its order (by name, byte order), with its SHA-256.
    pub fn new(mut list: Vec<Branch>) -> Branches {
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Branches { sha256: list_sha256(&list), list }
    }
}

/// The SHA-256 of the canonical JSON of a branch list, as an array of objects.
fn list_sha256(list: &[Branch]) -> String {
    let objects: Vec<Value> = list
        .iter()
        .map(|b| {
            let mut o = json!({"name": b.name, "kind": b.kind, "shape": b.shape.value()});
            if let Some(why) = &b.why {
                o["why"] = json!(why);
            }
            o
        })
        .collect();
    let text = crate::json::canonical(&Value::Array(objects)).expect("a branch list is plain JSON");
    crate::write::text_checksum(&text)
}

impl Lock {
    /// The lock `text` holds, read on rails and checked, or every problem with it.
    pub fn read(text: &str) -> Result<Lock, Problems> {
        let doc: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| Problems::one(format!("samples.lock is not TOML: {e}")))?;
        let mut p = Problems::default();
        let (resolved, samples_toml_sha256, sample_tables, branch_tables) = {
            let top = ["format", "version", "resolved", "samples_toml_sha256", "sample", "branches"];
            let mut at = Fields::new(&doc, "samples.lock", &top, &[], &mut p);
            if at.text("format").is_some_and(|f| f != "rvr-samples-lock") {
                at.p.say("samples.lock's format is not rvr-samples-lock");
            }
            if at.whole("version").is_some_and(|v| v != 1) {
                at.p.say("samples.lock's version is not 1, the one this reader knows");
            }
            let resolved = at.text("resolved").unwrap_or_default();
            let sha256 = at.text("samples_toml_sha256").unwrap_or_default();
            (resolved, sha256, at.tables("sample"), at.tables("branches"))
        };
        let mut samples = Vec::new();
        for (index, table) in sample_tables.iter().enumerate() {
            let place = format!("samples.lock sample {}", index + 1);
            samples.extend(sample(table, &place, &mut p));
        }
        let mut branches = Vec::new();
        for (index, table) in branch_tables.iter().enumerate() {
            branches.extend(branch_list(table, &format!("samples.lock branches {}", index + 1), &mut p));
        }
        let lock = Lock { resolved, samples_toml_sha256, samples, branches };
        if p.said.is_empty() {
            lock.check(&mut p);
        }
        p.or(lock)
    }

    /// FORMAT.md §6.3's rules beyond each field's form.
    fn check(&self, p: &mut Problems) {
        if !utc(&self.resolved) {
            p.say(format!("samples.lock's resolved {:?} is not YYYY-MM-DDTHH:MM:SSZ", self.resolved));
        }
        if !hex(&self.samples_toml_sha256, 64) {
            p.say("samples.lock's samples_toml_sha256 is not 64 lowercase hex digits");
        }
        if self.samples.is_empty() {
            p.say("samples.lock has no sample");
        }
        let lists: BTreeMap<&str, &Branches> = self.branches.iter().map(|b| (b.sha256.as_str(), b)).collect();
        if lists.len() != self.branches.len() {
            p.say("samples.lock writes one branch list twice");
        }
        let mut used = BTreeSet::new();
        let mut names = BTreeSet::new();
        for s in &self.samples {
            let place = format!("samples.lock sample {:?}", s.name);
            if !names.insert(s.name.as_str()) {
                p.say(format!("{place} is named twice"));
            }
            if s.files.is_empty() {
                p.say(format!("{place} has no file: every sample has one or more"));
            }
            let mut seen = BTreeSet::new();
            for f in &s.files {
                let local = !f.at.contains("://");
                let at = match local {
                    true => crate::study::lexical(std::path::Path::new(&f.at)).to_string_lossy().into_owned(),
                    false => f.at.clone(),
                };
                if !seen.insert(at) {
                    p.say(format!("{place} names {} twice", f.at));
                }
                if local && f.adler32.is_none() {
                    p.say(format!("{place}: {} has no adler32: a lock gives one for every file on this computer", f.at));
                }
                if f.bytes == 0 {
                    p.say(format!("{place}: {} has bytes 0: a lock knows every file's size", f.at));
                }
                if f.adler32.as_deref().is_some_and(|a| !hex(a, 8)) {
                    p.say(format!("{place}: {}'s adler32 is not 8 lowercase hex digits", f.at));
                }
                if !uuid(&f.uuid) {
                    p.say(format!("{place}: {}'s uuid is not a ROOT UUID (8-4-4-4-12 lowercase hex)", f.at));
                }
            }
            order(s, &place, p);
            match lists.contains_key(s.branches.as_str()) {
                true => drop(used.insert(s.branches.as_str())),
                false => p.say(format!("{place}: its branches names no branch list of the lock")),
            }
            let entries: Vec<u64> = s.files.iter().map(|f| f.entries).collect();
            let total = match entries_in_all(&entries, "its") {
                Ok(total) => total,
                Err(why) => {
                    p.say(format!("{place}: {why}, then resolve again"));
                    continue;
                }
            };
            // A random choice is made again where it is made anyway (full validation, the Plan):
            // its time grows with the entries. Here, that it takes what it chose in all.
            let planned = match s.events {
                Events::Random { n, .. } => {
                    let taken = s.files.iter().try_fold(0u64, |sum, f| sum.checked_add(f.take.unwrap_or(0)));
                    let fits = s.files.iter().all(|f| f.stop.is_none() && f.take.is_some_and(|t| t <= f.entries));
                    let taken = taken.filter(|_| fits).unwrap_or(u64::MAX);
                    if taken != n.min(total) {
                        p.say(format!("{place}: its files' take is not what its random choice reads"));
                    }
                    taken
                }
                events => {
                    let planned = reads(&entries, events);
                    for (f, r) in s.files.iter().zip(&planned) {
                        if (f.stop, f.take) != (r.stop, r.take) {
                            p.say(format!("{place}: {}'s stop and take are not what its event choice reads", f.at));
                        }
                    }
                    planned.iter().map(|r| r.read).sum::<u64>()
                }
            };
            if s.events_total != total {
                p.say(format!("{place}: events_total is not the sum of its files' entries"));
            }
            if s.events_read != planned {
                p.say(format!("{place}: events_read is not what its event choice reads"));
            }
        }
        if used.len() != lists.len() {
            p.say("samples.lock has a branch list no sample has");
        }
        for b in &self.branches {
            let place = format!("samples.lock branch list {}", b.sha256.chars().take(12).collect::<String>());
            if b.list.windows(2).any(|w| w[0].name >= w[1].name) {
                p.say(format!("{place} is not in byte order of name, each name once"));
            }
            if !hex(&b.sha256, 64) || list_sha256(&b.list) != b.sha256 {
                p.say(format!("{place}: its sha256 is not that of its list"));
            }
        }
    }

    /// The lock as text, as rvr samples resolve writes it.
    pub fn write(&self) -> Result<String, String> {
        let s = |text: &str| toml::Value::String(text.to_owned()).to_string();
        let mut out = String::from("# samples.lock: written by rvr samples resolve; it pins every file this study reads.\n# Review its changes as you review code, and never edit it: resolve again instead.\n\n");
        out += &format!("format = \"rvr-samples-lock\"\nversion = 1\nresolved = {}\nsamples_toml_sha256 = {}\n", s(&self.resolved), s(&self.samples_toml_sha256));
        for sample in &self.samples {
            let asked: Vec<String> = sample.asked.iter().map(|a| s(a)).collect();
            out += &format!("\n[[sample]]\nname = {}\nlabel = {}\nasked = [{}]\ntree = {}\nbranches = {}\n", s(&sample.name), s(&sample.label), asked.join(", "), s(&sample.tree), s(&sample.branches));
            out += &match sample.events {
                Events::All => "events = \"all\"\n".into(),
                Events::First(n) => format!("events = {n}\n"),
                Events::Random { n, seed } => format!("events = {n}\nchoose = \"random\"\nseed = {}\n", toml_whole(seed, "seed")?),
            };
            out += &format!("events_total = {}\nevents_read = {}\n", sample.events_total, sample.events_read);
            out += &format!("metadata = {}\n", toml_value(&Value::Object(sample.metadata.clone()), &format!("sample {}'s metadata", sample.name))?);
            for f in &sample.files {
                out += &format!("\n[[sample.file]]\nat = {}\nbytes = {}\n", s(&f.at), f.bytes);
                if let Some(adler32) = &f.adler32 {
                    out += &format!("adler32 = {}\n", s(adler32));
                }
                out += &format!("uuid = {}\nentries = {}\n", s(&f.uuid), f.entries);
                if let Some(stop) = f.stop {
                    out += &format!("stop = {stop}\n");
                }
                if let Some(take) = f.take {
                    out += &format!("take = {take}\n");
                }
            }
        }
        for b in &self.branches {
            out += &format!("\n[[branches]]\nsha256 = {}\nlist = [\n", s(&b.sha256));
            for branch in &b.list {
                // As JSON, which writes these plain texts and numbers as TOML does.
                let shape = branch.shape.value().to_string();
                let why = branch.why.as_ref().map(|why| format!(", why = {}", s(why))).unwrap_or_default();
                out += &format!("  {{ name = {}, kind = {}, shape = {shape}{why} }},\n", s(&branch.name), s(&branch.kind));
            }
            out += "]\n";
        }
        Ok(out)
    }

    /// The branch list of a sample of the lock.
    pub fn branches_of(&self, sample: &Sample) -> Option<&Branches> {
        self.branches.iter().find(|b| b.sha256 == sample.branches)
    }
}

/// Check a sample's files are the matches of its `asked` entries (docs/SPEC.md §3.8): entry after
/// entry, each entry's in byte order of name, each `at` its entry's folder and the name (a bare
/// name as `./NAME`), and every entry matching one file or more.
fn order(s: &Sample, place: &str, p: &mut Problems) {
    let (mut entry, mut last, mut used) = (0, None::<&str>, vec![false; s.asked.len()]);
    for f in &s.files {
        let (folder, name) = super::split(&f.at);
        let written = super::join(folder, name);
        let found = (entry..s.asked.len()).find(|&i| {
            let (at, pattern) = super::split(&s.asked[i]);
            at == folder && super::matches(pattern, name)
        });
        match found {
            Some(i) if written == f.at && (i > entry || last.is_none_or(|last| last < name)) => {
                (entry, last, used[i]) = (i, Some(name), true);
            }
            _ => {
                p.say(format!("{place}: {} is not where the lock lists it: its files are the matches of asked, entry after entry, each entry's in byte order of name, written as the entry writes its folder (a bare name as ./NAME)", f.at));
                return;
            }
        }
    }
    if let Some(i) = used.iter().position(|u| !u) {
        p.say(format!("{place}: its asked {} matches none of its files, and every entry of asked matches one or more", s.asked[i]));
    }
}

/// Check that `lock` was resolved from `text`, a samples.toml: its checksum, and the samples it makes.
pub fn agree(text: &str, lock: &Lock) -> Result<(), Problems> {
    let mut p = Problems::default();
    if crate::write::text_checksum(text) != lock.samples_toml_sha256 {
        p.say("samples.lock was not resolved from this samples.toml (its samples_toml_sha256 differs): run rvr samples resolve and review the new lock");
        return Err(p);
    }
    let made = samples::read(text)?;
    if made.len() != lock.samples.len() {
        p.say(format!("samples.toml makes {} samples and samples.lock holds {}", made.len(), lock.samples.len()));
    }
    for (want, have) in made.iter().zip(&lock.samples) {
        let place = format!("samples.lock sample {:?}", have.name);
        let files: Vec<&str> = have.files.iter().map(|f| f.at.as_str()).collect();
        let metadata = samples::file_filled(&want.metadata, &files);
        let same = want.name == have.name
            && want.label == have.label
            && want.tree == have.tree
            && want.events == have.events
            && want.files == have.asked
            && metadata.as_ref() == Some(&have.metadata);
        if !same {
            p.say(format!("{place} is not the sample samples.toml makes (its name, label, tree, event choice, files asked or metadata)"));
        }
    }
    p.or(())
}

/// Whether `text` is a ROOT UUID as a file records it: 8-4-4-4-12 lowercase hex.
pub(crate) fn uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5 && parts.iter().zip([8, 4, 4, 4, 12]).all(|(part, n)| hex(part, n))
}

/// A JSON value as TOML writes it, each number a TOML number (rowver keeps serde_json's numbers
/// exact, which TOML would write as tables); `name` says which value TOML cannot hold.
pub(crate) fn toml_value(value: &Value, name: &str) -> Result<toml::Value, String> {
    let cannot = || format!("samples.lock cannot hold {name}: TOML has no such value (a null, or a number past its range). Give it a text, a whole number of 64 bits or a finite float in samples.toml");
    Ok(match value {
        Value::Null => return Err(cannot()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) if crate::json::whole(n) => toml::Value::Integer(i),
            (_, Some(f)) if f.is_finite() && !crate::json::whole(n) => toml::Value::Float(f),
            _ => return Err(cannot()),
        },
        Value::Array(list) => toml::Value::Array(list.iter().map(|v| toml_value(v, name)).collect::<Result<_, _>>()?),
        Value::Object(map) => toml::Value::Table(map.iter().map(|(k, v)| Ok((k.clone(), toml_value(v, name)?))).collect::<Result<_, String>>()?),
    })
}

fn toml_whole(n: u64, name: &str) -> Result<String, String> {
    i64::try_from(n).map(|n| n.to_string()).map_err(|_| format!("samples.lock cannot hold {name} {n}: TOML's whole numbers end at 2^63 − 1"))
}

/// One `[[sample]]` of a lock.
fn sample(table: &toml::Table, place: &str, p: &mut Problems) -> Option<Sample> {
    let random = table.get("choose").is_some();
    let mut required = vec!["name", "label", "asked", "tree", "branches", "events", "events_total", "events_read", "metadata", "file"];
    if random {
        required.extend(["choose", "seed"]);
    }
    let mut at = Fields::new(table, place, &required, &[], p);
    let name = at.text("name");
    let label = at.text("label");
    let asked = at.texts("asked");
    let tree = at.text("tree");
    let branches = at.text("branches");
    let n = match at.get("events") {
        Some(toml::Value::String(all)) if all == "all" => None,
        Some(toml::Value::Integer(n)) if *n >= 1 => Some(*n as u64),
        Some(_) => {
            at.p.say(format!("{place}: events is \"all\" or a whole number from 1"));
            None
        }
        None => None,
    };
    let events = match (random, n) {
        (false, None) => Events::All,
        (false, Some(n)) => Events::First(n),
        (true, Some(n)) if at.text("choose").as_deref() == Some("random") => Events::Random { n, seed: at.whole("seed").unwrap_or(0) },
        (true, _) => {
            at.p.say(format!("{place}: choose is \"random\", with a number of events and a seed"));
            Events::All
        }
    };
    let events_total = at.whole("events_total");
    let events_read = at.whole("events_read");
    let metadata = match at.get("metadata") {
        Some(toml::Value::Table(t)) => {
            samples::metadata(t, &format!("{place} metadata"), at.p);
            serde_json::to_value(t).ok().and_then(|v| v.as_object().cloned())
        }
        Some(_) => {
            at.p.say(format!("{place}: metadata must be a table"));
            None
        }
        None => None,
    };
    let files: Vec<File> = at.tables("file").iter().enumerate().filter_map(|(i, t)| file(t, &format!("{place} file {}", i + 1), random, at.p)).collect();
    Some(Sample {
        name: name?,
        label: label?,
        asked: asked?,
        tree: tree?,
        branches: branches?,
        events,
        events_total: events_total?,
        events_read: events_read?,
        metadata: metadata?,
        files,
    })
}

/// One `[[sample.file]]` of a lock.
fn file(table: &toml::Table, place: &str, random: bool, p: &mut Problems) -> Option<File> {
    let optional: &[&str] = if random { &["adler32", "take"] } else { &["adler32", "stop"] };
    let mut at = Fields::new(table, place, &["at", "bytes", "uuid", "entries"], optional, p);
    if random && !table.contains_key("take") {
        at.p.say(format!("{place}: a file of a random choice has take"));
    }
    // Every field is read before any is missed, so that each problem is said.
    let (path, bytes, uuid, entries) = (at.text("at"), at.whole("bytes"), at.text("uuid"), at.whole("entries"));
    let adler32 = table.contains_key("adler32").then(|| at.text("adler32")).flatten();
    let stop = table.contains_key("stop").then(|| at.whole("stop")).flatten();
    let take = table.contains_key("take").then(|| at.whole("take")).flatten();
    Some(File { at: path?, bytes: bytes?, adler32, uuid: uuid?, entries: entries?, stop, take })
}

/// One `[[branches]]` of a lock.
fn branch_list(table: &toml::Table, place: &str, p: &mut Problems) -> Option<Branches> {
    let mut at = Fields::new(table, place, &["sha256", "list"], &[], p);
    let sha256 = at.text("sha256")?;
    let Some(toml::Value::Array(items)) = at.get("list") else {
        at.p.say(format!("{place}: list is an array of branches"));
        return None;
    };
    let mut list = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let place = format!("{place} branch {}", i + 1);
        let Some(item) = item.as_table() else {
            at.p.say(format!("{place} must be a table"));
            continue;
        };
        let none = item.get("kind").and_then(toml::Value::as_str) == Some("none");
        let mut b = Fields::new(item, &place, &["name", "kind", "shape"], if none { &["why"] } else { &[] }, at.p);
        let kind = b.text("kind");
        if kind.as_deref().is_some_and(|k| k != "none" && rvr_recipe::Kind::named(k).is_err()) {
            b.p.say(format!("{place}: kind is a .rvr kind (f4, f8, i1 … u8, b1), or none"));
        }
        if none && !item.contains_key("why") {
            b.p.say(format!("{place}: a branch rvr cannot read says why"));
        }
        let shape = match b.get("shape") {
            Some(toml::Value::String(text)) => Shape::named(text),
            Some(toml::Value::Integer(n)) if *n >= 1 => Some(Shape::Fixed(*n as u64)),
            _ => None,
        };
        if shape.is_none() {
            b.p.say(format!("{place}: shape is \"flat\", \"jagged\", \"jagged N\" (N values per element, from 2) or a whole number from 1"));
        }
        if none && shape.is_some_and(|shape| shape != Shape::Flat) {
            b.p.say(format!("{place}: a branch rvr cannot read has shape \"flat\""));
        }
        let why = none.then(|| b.text("why")).flatten();
        if let (Some(name), Some(kind), Some(shape)) = (b.text("name"), kind, shape) {
            list.push(Branch { name, kind, shape, why });
        }
    }
    Some(Branches { sha256, list })
}

/// A table read on rails: exactly its fields, each taken with its form checked.
struct Fields<'a, 'p> {
    table: &'a toml::Table,
    place: String,
    p: &'p mut Problems,
}

impl<'a, 'p> Fields<'a, 'p> {
    fn new(table: &'a toml::Table, place: &str, required: &[&str], optional: &[&str], p: &'p mut Problems) -> Self {
        let known: Vec<&str> = required.iter().chain(optional).copied().collect();
        for key in table.keys().filter(|k| !known.contains(&k.as_str())) {
            p.say(super::unknown(place, key, &known));
        }
        for key in required.iter().filter(|k| !table.contains_key(**k)) {
            p.say(format!("{place} has no {key}"));
        }
        Fields { table, place: place.to_owned(), p }
    }

    fn get(&self, key: &str) -> Option<&'a toml::Value> {
        self.table.get(key)
    }

    fn text(&mut self, key: &str) -> Option<String> {
        match self.table.get(key)? {
            toml::Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => {
                self.p.say(format!("{}: {key} must be nonempty text", self.place));
                None
            }
        }
    }

    fn texts(&mut self, key: &str) -> Option<Vec<String>> {
        match self.table.get(key)? {
            toml::Value::Array(list) if !list.is_empty() && list.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty())) => {
                Some(list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            }
            _ => {
                self.p.say(format!("{}: {key} must be a nonempty list of nonempty text", self.place));
                None
            }
        }
    }

    fn whole(&mut self, key: &str) -> Option<u64> {
        match self.table.get(key)? {
            toml::Value::Integer(n) if *n >= 0 => Some(*n as u64),
            _ => {
                self.p.say(format!("{}: {key} must be a whole number from 0", self.place));
                None
            }
        }
    }

    fn tables(&mut self, key: &str) -> Vec<toml::Table> {
        match self.table.get(key) {
            None => Vec::new(),
            Some(toml::Value::Array(list)) if list.iter().all(toml::Value::is_table) => {
                list.iter().filter_map(|v| v.as_table().cloned()).collect()
            }
            Some(_) => {
                self.p.say(format!("{}: {key} must be written [[{key}]], one table each", self.place));
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(samples_toml: &str) -> Lock {
        let list = Branches::new(vec![
            Branch { name: "nJet".into(), kind: "i4".into(), shape: Shape::Flat, why: None },
            Branch { name: "Jet_pt".into(), kind: "f4".into(), shape: Shape::Jagged, why: None },
            Branch { name: "obj".into(), kind: "none".into(), shape: Shape::Flat, why: Some("a whole object".into()) },
            Branch { name: "pv".into(), kind: "f4".into(), shape: Shape::Fixed(3), why: None },
            Branch { name: "xy".into(), kind: "f4".into(), shape: Shape::JaggedFixed(3), why: None },
        ]);
        let file = |at: &str, entries: u64, stop| File {
            at: at.into(),
            bytes: 100,
            adler32: Some("0a1b2c3d".into()),
            uuid: "31e851e4-b994-11f1-ba5c-1a45bb13c76a".into(),
            entries,
            stop,
            take: None,
        };
        Lock {
            resolved: "2026-11-02T12:00:00Z".into(),
            samples_toml_sha256: crate::write::text_checksum(samples_toml),
            samples: vec![Sample {
                name: "a".into(),
                label: "A".into(),
                asked: vec!["./*.root".into()],
                tree: "T".into(),
                branches: list.sha256.clone(),
                events: Events::First(7),
                events_total: 10,
                events_read: 7,
                metadata: json!({"kind": "signal", "xsec": 2.5, "era": 3}).as_object().unwrap().clone(),
                files: vec![file("./a.root", 5, None), file("./b.root", 5, Some(2))],
            }],
            branches: vec![list],
        }
    }

    const TOML: &str = "format = \"rvr-samples\"\nversion = 1\n[places]\n[defaults]\n[eras]\n\
        [[sample]]\nname = \"a\"\nlabel = \"A\"\nfiles = \"./*.root\"\ntree = \"T\"\nevents = 7\n\
        metadata = { kind = \"signal\", xsec = 2.5, era = 3 }\n";

    // SPEC 2.3; FORMAT 6.3
    #[test]
    fn a_lock_written_reads_back_the_same_and_agrees_with_its_samples_toml() {
        let lock = lock(TOML);
        let text = lock.write().unwrap();
        assert!(text.contains("  { name = \"Jet_pt\", kind = \"f4\", shape = \"jagged\" },\n"), "{text}");
        assert!(text.contains("shape = 3 },\n") && text.contains("shape = \"jagged 3\" },\n"), "{text}");
        assert_eq!(Lock::read(&text).unwrap(), lock);
        assert_eq!(agree(TOML, &lock), Ok(()));
        assert!(agree(&TOML.replace("label = \"A\"", "label = \"B\""), &lock).is_err());
    }

    // FORMAT 6.1, 6.3, 12
    #[test]
    fn each_file_of_a_random_choice_takes_its_share() {
        let mut lock = lock(TOML);
        let s = &mut lock.samples[0];
        s.events = Events::Random { n: 3, seed: 7 };
        s.events_read = 3;
        let counts = crate::choice::counts(&[5, 5], 3, 7);
        for (f, count) in s.files.iter_mut().zip(&counts) {
            (f.stop, f.take) = (None, Some(*count));
        }
        assert_eq!(s.takes(&counts), Ok(()));
        // Swapped, the takes add up as before (Lock::read's check), and each is not its share.
        let swapped = [counts[1], counts[0]];
        let said = s.takes(&swapped).unwrap_err();
        assert!(said.contains("sample a's file ./a.root has take") && said.contains("resolve again"), "{said}");
        let text = lock.write().unwrap();
        assert!(Lock::read(&text.replacen(&format!("take = {}\n", counts[0]), "TAKE\n", 1).replacen(&format!("take = {}\n", counts[1]), &format!("take = {}\n", counts[0]), 1).replacen("TAKE\n", &format!("take = {}\n", counts[1]), 1)).is_ok(), "Lock::read checks only the sum");
    }

    // SPEC 1.4, 1.5; FORMAT 6.3
    #[test]
    fn a_lock_off_its_rails_is_refused_with_every_problem() {
        let text = lock(TOML).write().unwrap();
        let said = |text: String| Lock::read(&text).unwrap_err().to_string();
        assert!(said(text.replace("entries = 5\n", "entries = 5\nmodified = \"x\"\n")).contains("no key \"modified\""));
        assert!(said(text.replace("entries = 5\n", "entires = 5\n")).contains("has no key \"entires\"; it takes at, bytes, uuid, entries, adler32, stop. Did you mean: entries?"));
        assert!(said(text.replace("stop = 2\n", "")).contains("not what its event choice reads"));
        assert!(said(text.replace("events_read = 7", "events_read = 8")).contains("events_read"));
        assert!(said(text.replace("bytes = 100", "bytes = 0")).contains("bytes 0"));
        assert!(said(text.replace("at = \"./b.root\"", "at = \"a.root\"")).contains("names a.root twice"));
        assert!(said(text.replace("adler32 = \"0a1b2c3d\"\n", "")).contains("has no adler32"));
        assert!(said(text.replace("era = 3 }", "era = 3, events_read = 1 }")).contains("given by rvr"));
        assert!(said(text.replace("era = 3 }", "era = 3, when = 2026-11-02 }")).contains("date or time"));
        assert!(said(text.replace("31e851e4", "31E851E4")).contains("uuid"));
        assert!(said(text.replace("kind = \"f4\"", "kind = \"f16\"")).contains("kind is a .rvr kind"));
        assert!(said(text.replace("kind = \"none\", shape = \"flat\"", "kind = \"none\", shape = \"jagged\"")).contains("a branch rvr cannot read has shape \"flat\""));
        for shape in ["\"jagged 03\"", "\"jagged 1\"", "\"jagged3\"", "0"] {
            assert!(said(text.replace("\"jagged 3\"", shape)).contains("shape is \"flat\""), "{shape}");
        }
        assert!(said(text.replace("events = 7\n", "events = 7\nchoose = \"random\"\n")).contains("has no seed"));
        let sha = lock(TOML).branches[0].sha256.clone();
        assert!(said(text.replace(&format!("sha256 = \"{sha}\""), "sha256 = \"a€€€€€€€\"")).contains("its sha256 is not that of its list"));
        assert!(said(text.replace("version = 1", "version = 2")).contains("version is not 1"));
        let both = said(text.replacen("at = \"./a.root\"", "at = \"\"", 1).replacen("bytes = 100", "bytes = -1", 1));
        assert!(both.contains("at must be nonempty text") && both.contains("bytes must be a whole number"), "{both}");
        let twice = said(text.replace("bytes = 100", "bytes = 0").replace("31e851e4", "31E851E4"));
        assert!(twice.contains("bytes 0") && twice.contains("uuid"), "{twice}");
        assert!(said(text.replace("nJet", "Aaa")).contains("byte order"));
        let swapped = text.replace("at = \"./a.root\"", "at = \"./c.root\"").replace("at = \"./b.root\"", "at = \"./a.root\"").replace("at = \"./c.root\"", "at = \"./b.root\"");
        assert!(said(swapped).contains("./a.root is not where the lock lists it"));
        assert!(said(text.replace("at = \"./b.root\"", "at = \"b.root\"")).contains("b.root is not where the lock lists it"));
        assert!(said(text.replace("at = \"./b.root\"", "at = \"./b.dat\"")).contains("./b.dat is not where"));
    }

    // SPEC 7.1.1; FORMAT 6.1, 6.3
    #[test]
    fn what_a_press_reads_of_each_file() {
        let first = reads(&[100, 50, 80], Events::First(120));
        assert_eq!(first.iter().map(|r| (r.read, r.stop)).collect::<Vec<_>>(), [(100, None), (20, Some(20)), (0, Some(0))]);
        let a = reads(&[867_540, 8_332, 714_243], Events::Random { n: 50_000, seed: 7 });
        assert_eq!(a, reads(&[867_540, 8_332, 714_243], Events::Random { n: 50_000, seed: 7 }));
        assert_eq!(a.iter().map(|r| r.read).sum::<u64>(), 50_000);
    }
}
