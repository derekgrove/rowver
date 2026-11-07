//! `rvr samples`: samples.toml in, samples.lock out, and what a press reads of each sample.
//! samples.toml and samples.lock are rowver's (`rowver::study`); see docs/SPEC.md §6.

pub(crate) mod multipart;
pub(crate) mod remote;
pub(crate) mod resolve;

use rowver::rvr::grouped as count;
use resolve::Context;
use rowver::choice::Events;
use rowver::study::lock::{self, Branches, Lock};
use rowver::study::samples;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// How many of a thing, singular or plural as the count asks: `1 file`, `3 files`.
pub fn some(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", count(n as u64), if n == 1 { one } else { many })
}

/// "all 1,234 events", or "50,000 of 1,234,567 events (random)".
fn reading(sample: &lock::Sample) -> String {
    if sample.events_read == sample.events_total {
        format!("all {} events", count(sample.events_total))
    } else {
        let random = matches!(sample.events, Events::Random { .. });
        format!("{} of {} events{}", count(sample.events_read), count(sample.events_total), if random { " (random)" } else { "" })
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b => format!("{:.1} MB", b as f64 / 1e6),
    }
}

/// Values made once a run, each by the first to ask for it while the others wait (a folder's listing).
pub(crate) struct Once<K, V>(std::sync::Mutex<std::collections::HashMap<K, std::sync::Arc<std::sync::OnceLock<V>>>>);

impl<K, V> Default for Once<K, V> {
    fn default() -> Self {
        Once(Default::default())
    }
}

impl<K: Eq + std::hash::Hash, V: Clone> Once<K, V> {
    /// The value for `key`, made by `make` when no one has made it yet.
    pub fn get(&self, key: K, make: impl FnOnce() -> V) -> V {
        let slot = self.0.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_default().clone();
        slot.get_or_init(make).clone()
    }
}

/// The grid proxy to present, and what to say of it.
pub(crate) struct Proxy {
    pub path: Option<String>,
    /// That it ends soon, when it does: said before remote work.
    pub short: Option<String>,
    /// A folder of extra certificate authorities, when one is named.
    pub ca: Option<String>,
}

/// Find the grid proxy to present.
pub(crate) fn proxy() -> Proxy {
    let credentials = voms_proxy::Credentials::found(None, None, &voms_proxy::Places::here());
    let status = voms_proxy::status(&credentials);
    // Extra certificate authorities, when a folder of them is named; grid-trust already trusts the
    // public roots and CERN's grid authorities, which FNAL and CERN EOS are signed by.
    let ca = std::env::var("X509_CERT_DIR")
        .ok()
        .or(credentials.authorities.clone());
    let short = crate::proxy::left_of(&status)
        .filter(|left| status.usable() && (0..crate::proxy::SHORT).contains(left));
    let short = short.map(|left| {
        format!(
            "your grid proxy (or its VOMS attributes) ends in {}; run rvr proxy init for a new one before a long press.",
            crate::proxy::left(left)
        )
    });
    let ended = (status.usable() && status.renew).then(|| {
        "your grid proxy's VOMS attributes have ended, and storage that asks for them will refuse \
         it; run rvr proxy init for a new one."
            .to_owned()
    });
    let short = ended.or(short);
    if status.usable() {
        return Proxy {
            path: status.path,
            short,
            ca,
        };
    }
    Proxy { path: None, short, ca }
}

/// The samples samples.toml at `path` makes, or every problem with it; at a terminal, what it
/// leaves out (its header lines and tables) is offered first, and written in on a yes.
fn read_samples(path: &Path, offering: bool) -> Result<(String, Vec<samples::Sample>), String> {
    let text = std::fs::read_to_string(path).map_err(|e| rowver::plain_io(path, &e))?;
    match samples::read(&text) {
        Ok(made) => Ok((text, made)),
        Err(problems) if offering && problems.said.is_empty() && offer(path, &text, &problems.missing)? => {
            let text = std::fs::read_to_string(path).map_err(|e| rowver::plain_io(path, &e))?;
            let made = samples::read(&text).map_err(|p| format!("{}: {p}", path.display()))?;
            Ok((text, made))
        }
        Err(problems) => Err(format!("{}: {problems}", path.display())),
    }
}

/// At a terminal, offer to write into samples.toml (`source`, at `path`) what it leaves out: its
/// header lines first, its tables in their places. Whether it did.
fn offer(path: &Path, source: &str, missing: &[&'static str]) -> Result<bool, String> {
    if missing.is_empty() || !crate::at_terminal() {
        return Ok(false);
    }
    // Placed before asking, so that a yes is not refused after.
    let text = match mended(source, missing) {
        Ok(text) => text,
        Err(why) => {
            eprintln!("{why}");
            return Ok(false);
        }
    };
    let names: Vec<&str> = samples::TABLES.iter().filter_map(|t| t.strip_prefix('[')?.strip_suffix(']')).collect();
    let named = missing.join(", ");
    let question = format!(
        "samples.toml leaves out {named}: every samples.toml starts with {} and {}, and writes {}, each even when empty.\nWrite {} in, in {}, then go on resolving? [y/N] ",
        samples::FORMAT,
        samples::VERSION,
        crate::tables::listed(&names, "and"),
        if missing.len() == 1 { "it" } else { "them" },
        if missing.len() == 1 { "its place" } else { "their places" }
    );
    crate::tables::ask_and_write(path, source, &text, &question, &format!("write {named} into it"), "resolve")?;
    eprintln!("Wrote {named} into samples.toml.");
    Ok(true)
}

/// samples.toml (`source`) with what it leaves out written in: the header lines first (after a
/// byte-order mark), the tables in their places; refused unless it reads as before with only
/// these added.
fn mended(source: &str, missing: &[&str]) -> Result<String, String> {
    let root: toml::Table = source.parse().map_err(|e| format!("samples.toml is not TOML: {e}"))?;
    let tables: Vec<&str> = missing.iter().filter_map(|m| m.strip_prefix('[')?.strip_suffix(']')).collect();
    let heads: Vec<&str> = missing.iter().copied().filter(|m| !m.starts_with('[')).collect();
    // The tables in their order, then the samples.
    let names = samples::TABLES.iter().filter_map(|t| t.strip_prefix('[')?.strip_suffix(']'));
    let order: Vec<&str> = names.chain(["sample"]).collect();
    let body = crate::tables::written(source, &root, &tables, &order, "samples.toml")?;
    let (mark, body) = body.strip_prefix('\u{feff}').map_or(("", body.as_str()), |rest| ("\u{feff}", rest));
    let text = match heads.is_empty() {
        true => format!("{mark}{body}"),
        false => format!("{mark}{}\n\n{body}", heads.join("\n")),
    };
    let mut expected = root.clone();
    expected.extend(heads.join("\n").parse::<toml::Table>().unwrap_or_default());
    expected.extend(tables.iter().map(|t| (t.to_string(), toml::Value::Table(Default::default()))));
    match text.parse::<toml::Table>().ok() == Some(expected) {
        true => Ok(text),
        false => Err(format!("{} could not be written into samples.toml, so it is as it was.", missing.join(", "))),
    }
}

fn expand(path: &Path, json: bool) -> Result<(), String> {
    let (_, made) = read_samples(path, false)?;
    if json {
        let list: Vec<_> = made
            .iter()
            .map(|s| serde_json::json!({"name": s.name, "label": s.label, "tree": s.tree, "files": s.files, "events": s.events.record(), "metadata": s.metadata}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap());
        return Ok(());
    }
    let entries: BTreeSet<usize> = made.iter().map(|s| s.entry).collect();
    println!(
        "{} describes {} from {}:",
        path.display(),
        some(made.len(), "sample", "samples"),
        some(entries.len(), "[[sample]] entry", "[[sample]] entries")
    );
    let choice = |events: Events| match events {
        Events::All => "all".to_string(),
        Events::First(n) => format!("first {n}"),
        Events::Random { n, seed } => format!("random {n}, seed {seed}"),
    };
    println!("  {:34} {:40} {:18} {:22} reads", "name", "label", "tree", "events");
    for s in &made {
        println!("  {:34} {:40} {:18} {:22} {}", s.name, s.label, s.tree, choice(s.events), s.files.join(" + "));
    }
    Ok(())
}

/// The sample `named` (the first when none is) of samples.toml at `path`, its first file found as
/// resolve finds it and opened, nothing else asked (no adler32, no digest): the sample, its first
/// file and that file's branch list. Nothing is written.
pub(crate) fn first_file(path: &Path, named: Option<&str>) -> Result<(samples::Sample, String, Branches), String> {
    let (_, made) = read_samples(path, false)?;
    let names: Vec<&str> = made.iter().map(|s| s.name.as_str()).collect();
    let sample = made[crate::open::sample_at(&names, named, "samples.toml")?].clone();
    let proxy = proxy();
    let mut ctx = Context::new(path.parent().map(Path::to_path_buf).unwrap_or_default(), proxy.path, proxy.ca);
    ctx.pins = false;
    let first = std::sync::OnceLock::new();
    let resolved = resolve::resolve(&ctx, &sample, &|locked, list| {
        let _ = first.set((locked.files[0].at.clone(), list.clone()));
        ctx.stop();
    });
    // A sample resolves only with a branch list, which `first` is given before resolve returns.
    match first.into_inner() {
        Some((at, list)) => Ok((sample, at, list)),
        None => Err(format!("{}: {}", sample.name, resolved.err().unwrap_or_default())),
    }
}

/// The recipe.toml beside samples.toml at `path`, as text and as its validator reads it; at a
/// terminal, the tables it leaves out are offered first. resolve writes no lock a recipe cannot be
/// pressed from.
fn recipe_beside(path: &Path) -> Result<(String, rvr_recipe::Recipe), String> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let text = crate::texts::study_file(dir, "recipe.toml")?;
    match crate::texts::read_recipe(&text) {
        Ok(recipe) => Ok((text, recipe)),
        Err(_) if crate::tables::offer(dir, &text, "resolve", "go on resolving")? => {
            let text = crate::texts::study_file(dir, "recipe.toml")?;
            crate::texts::read_recipe(&text).map(|recipe| (text, recipe))
        }
        Err(said) => Err(said),
    }
}

/// A sample as its first file makes it, and its branch list.
type First = (lock::Sample, Branches);

/// The recipe checked against each sample as its first file makes it, once every sample has one
/// (or failed first), so that a recipe that does not fit stops resolve before every other file is asked.
struct Early<'a> {
    recipe: &'a rvr_recipe::Recipe,
    /// Each sample's report: none yet, or its first (`None` when it failed before one).
    firsts: std::sync::Mutex<Vec<Option<Option<First>>>>,
    refused: std::sync::OnceLock<String>,
}

impl Early<'_> {
    /// Sample `i` as its first file makes it, or `None` when it failed before; once a sample.
    fn tell(&self, ctx: &Context, i: usize, first: Option<First>) {
        let mut held = self.firsts.lock().unwrap_or_else(|e| e.into_inner());
        if held[i].is_some() {
            return;
        }
        held[i] = Some(first);
        if held.iter().any(Option::is_none) {
            return;
        }
        let (samples, branches): (Vec<lock::Sample>, Vec<Branches>) = held.iter().flatten().flatten().cloned().unzip();
        let asked: Vec<Events> = samples.iter().map(|s| s.events).collect();
        let lock = Lock { resolved: String::new(), samples_toml_sha256: String::new(), samples, branches };
        if let Err(said) = crate::agree::recipe(self.recipe, &lock, &asked) {
            let _ = self.refused.set(said.join("\n"));
            ctx.stop();
        }
    }
}

/// What resolve says when the recipe does not fit the samples (`said`, one problem a line).
fn unfit(said: &str) -> String {
    for line in said.lines() {
        println!("  FAIL  {line}");
    }
    "the recipe does not fit the samples as the lines marked FAIL say, so samples.lock was not written (a press would refuse them): fix them, then run resolve again.".into()
}

fn resolve_all(path: &Path) -> Result<(), String> {
    let (text, made) = read_samples(path, true)?;
    let (recipe_text, recipe) = recipe_beside(path)?;
    let proxy = proxy();
    let remote = made.iter().any(|s| s.files.iter().any(|f| f.contains("://")));
    if let Some(short) = proxy.short.as_ref().filter(|_| remote) {
        eprintln!("rvr: {short}");
    }
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let ctx = Context::new(base, proxy.path, proxy.ca);
    let started = std::time::Instant::now();
    let next = std::sync::atomic::AtomicUsize::new(0);
    type Resolved = Result<(lock::Sample, Branches), String>;
    let results: Vec<std::sync::Mutex<Option<Resolved>>> = made.iter().map(|_| Default::default()).collect();
    let early = Early { recipe: &recipe, firsts: std::sync::Mutex::new(vec![None; made.len()]), refused: Default::default() };
    std::thread::scope(|s| {
        for _ in 0..12 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(sample) = made.get(i) else { break };
                let resolved = resolve::resolve(&ctx, sample, &|first, list| early.tell(&ctx, i, Some((first, list.clone()))));
                early.tell(&ctx, i, None);
                *results[i].lock().unwrap() = Some(resolved);
            });
        }
    });
    let refused = early.refused.get().cloned();
    let mut locked = Vec::new();
    let mut lists: Vec<Branches> = Vec::new();
    let mut failed = 0;
    for (sample, result) in made.iter().zip(results) {
        match result.into_inner().unwrap().unwrap() {
            Ok((l, list)) => {
                let bytes: u64 = l.files.iter().map(|f| f.bytes).sum();
                let files = if l.files.len() == 1 { "1 file ".to_string() } else { format!("{} files", l.files.len()) };
                println!("  ok    {:40} {files:>9} {:>10}   {}", l.name, size(bytes), reading(&l));
                if !lists.iter().any(|b| b.sha256 == list.sha256) {
                    lists.push(list);
                }
                locked.push(l);
            }
            // A sample stopped because the recipe does not fit says nothing of its own.
            Err(why) if refused.is_some() && why.ends_with(remote::STOPPED) => {}
            Err(why) => {
                println!("  FAIL  {}: {why}", sample.name);
                failed += 1;
            }
        }
    }
    let distinct: BTreeMap<&str, u64> = locked.iter().flat_map(|l| l.files.iter().map(|f| (f.at.as_str(), f.bytes))).collect();
    println!(
        "\n{}, {}, {} in {:.1} s.",
        some(locked.len(), "sample", "samples"),
        some(distinct.len(), "distinct file", "distinct files"),
        size(distinct.values().sum()),
        started.elapsed().as_secs_f64()
    );
    if let Some(said) = refused {
        return Err(unfit(&said));
    }
    unread(&ctx, &distinct);
    if failed > 0 {
        return Err(format!("{failed} of {} samples could not be resolved; samples.lock was not written. Fix the lines marked FAIL and run resolve again.", made.len()));
    }
    let lock = Lock {
        resolved: voms_proxy::utc(voms_proxy::now()),
        samples_toml_sha256: rowver::write::text_checksum(&text),
        samples: locked,
        branches: lists,
    };
    let body = lock.write()?;
    // What resolve writes is what every reader checks: a lock that does not read back is a bug.
    let read = Lock::read(&body).map_err(|p| format!("the lock resolve made does not check out (a bug in rvr: please report it): {p}"))?;
    // The agreement check a press makes, but for its options and the file it builds on.
    let texts = crate::texts::Texts { recipe: recipe_text, samples: text, lock: body.clone() };
    let choose = crate::plan::Choose::Options(Choice::default());
    if let Err(said) = crate::plan::agreed(&texts, true, Some(recipe), Some(read), Vec::new(), &choose) {
        return Err(unfit(&said));
    }
    let out = path.with_file_name("samples.lock");
    crate::tables::put(&out, &body).map_err(|e| format!("{} cannot be written ({e}), so it is the one it was: make it and its folder writable, then resolve again.", out.display()))?;
    println!("Wrote {} ({}).", out.display(), size(body.len() as u64));
    Ok(())
}

/// Say which `.root` files the folders read hold that no sample reads, as a check that none was
/// forgotten (not a failure).
fn unread(ctx: &Context, read: &BTreeMap<&str, u64>) {
    let folders: BTreeSet<&str> = read.keys().map(|at| resolve::split(at).0).collect();
    // Each file once, however its folder is written.
    let mut seen: BTreeSet<std::path::PathBuf> = read.keys().map(|at| ctx.which(at)).collect();
    let mut unread = Vec::new();
    for folder in folders {
        let names: Vec<String> = match folder.contains("://") {
            true => ctx.storage.as_ref().and_then(|s| s.list(folder).ok()).unwrap_or_default().into_iter().map(|e| e.name).collect(),
            false => ctx.listed(&resolve::local(folder, &ctx.base)).map(|names| names.to_vec()).unwrap_or_default(),
        };
        for name in names.into_iter().filter(|n| n.ends_with(".root")) {
            let at = resolve::join(folder, &name);
            if seen.insert(ctx.which(&at)) {
                unread.push(at);
            }
        }
    }
    if !unread.is_empty() {
        unread.sort();
        println!("The folders read also hold {} no sample reads (not an error; check none was meant):", some(unread.len(), ".root file", ".root files"));
        for at in unread.iter().take(3) {
            println!("  {at}");
        }
        if unread.len() > 3 {
            println!("  … and {} more", unread.len() - 3);
        }
    }
}

/// The lock at `path`, read on rails.
pub(crate) fn read_lock(path: &Path) -> Result<Lock, String> {
    let text = std::fs::read_to_string(path).map_err(|e| rowver::plain_io(path, &e))?;
    crate::texts::lock_of(&text, &path.display().to_string())
}

/// How each file a lock names differs from what it recorded, one line each: a local file (its
/// path relative to `base`) by its size and adler32, a storage file by its digest; and the files
/// that could not be asked, with why. Both empty when every file is as recorded.
pub(crate) fn differing(lock: &Lock, base: &Path) -> (Vec<String>, Vec<String>) {
    let proxy = proxy();
    let ctx = Context::new(base.to_path_buf(), proxy.path, proxy.ca);
    let files: BTreeMap<&str, &lock::File> = lock.samples.iter().flat_map(|s| s.files.iter().map(|f| (f.at.as_str(), f))).collect();
    let (mut changed, mut unasked) = (Vec::new(), Vec::new());
    for (at, f) in &files {
        let now = if at.contains("://") {
            match ctx.storage.as_ref().map(|s| s.digest(at)) {
                None => Err(ctx.offline.clone()),
                Some(Err(failed)) if failed.fault == crate::holes::Fault::Gone => {
                    changed.push(failed.why);
                    continue;
                }
                Some(now) => now.map_err(|failed| failed.why),
            }
        } else {
            let local = resolve::local(at, &ctx.base);
            let folder = local.parent().is_none_or(Path::exists);
            match std::fs::metadata(&local) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && folder => {
                    changed.push(format!("{at} no longer exists"));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("{at}: its folder is not there either (a disk not mounted?)")),
                Err(e) => Err(format!("{at} could not be read: {e}")),
                Ok(m) => resolve::adler32_file(&local).map(|a| (Some(m.len()), Some(a))).map_err(|e| format!("{at} could not be read: {e}")),
            }
        };
        match now {
            Ok((bytes, adler)) => changed.extend(resolve::differs(f, bytes, adler.as_deref())),
            Err(why) => unasked.push(why),
        }
    }
    (changed, unasked)
}

fn check(path: &Path) -> Result<bool, String> {
    let lock = read_lock(path)?;
    let mut same = true;
    if let Ok(now) = std::fs::read_to_string(path.with_file_name("samples.toml")) {
        if crate::agree::stale(&now, &lock) {
            println!("samples.toml has changed since the lock was written ({}): run rvr samples resolve to see what it now asks for.", lock.resolved);
            same = false;
        }
    }
    let (changed, unasked) = differing(&lock, path.parent().unwrap_or(Path::new("")));
    let files: BTreeSet<&str> = lock.samples.iter().flat_map(|s| s.files.iter().map(|f| f.at.as_str())).collect();
    if changed.is_empty() && unasked.is_empty() {
        println!("Every one of the {} is as the lock recorded it ({}).", some(files.len(), "file", "files"), lock.resolved);
    }
    if !changed.is_empty() {
        same = false;
        println!("{} from the lock:", if changed.len() == 1 { "1 file differs".to_owned() } else { format!("{} files differ", changed.len()) });
        for c in &changed {
            println!("  {c}");
        }
        println!("A press from this lock makes a changed (or gone) hole of each; if the change is wanted, run rvr samples resolve again and review the new lock.");
    }
    if !unasked.is_empty() {
        same = false;
        println!("{} could not be asked:", some(unasked.len(), "file", "files"));
        for u in &unasked {
            println!("  {u}");
        }
    }
    Ok(same)
}

/// What `--events N|all`, `--random` and `--seed S` ask of every sample, as the options validator read them.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    /// `--events N` (the first N) or `all`.
    pub events: Option<Events>,
    /// `--random`.
    pub random: bool,
    /// `--seed S`.
    pub seed: Option<u64>,
}

impl Choice {
    /// The choice the options give, refused when a value is not of its form or `--events all --random`.
    pub fn read(options: &crate::options::Options) -> Result<Choice, String> {
        let events = options.value("--events").map(crate::options::events).transpose()?;
        let seed = options.value("--seed").map(crate::options::seed).transpose()?;
        let choice = Choice { events, random: options.has("--random"), seed };
        if choice.random && choice.events == Some(Events::All) {
            return Err("--random chooses N events at random: give it with --events N, not --events all".into());
        }
        Ok(choice)
    }

    /// The options in words.
    pub fn said(&self) -> String {
        let seed = self.seed.map_or(String::new(), |seed| format!(" --seed {seed}"));
        match (self.events, self.random) {
            (None, false) if self.seed.is_some() => format!("{}: as samples.toml asks, with that seed for a random choice", seed.trim_start()),
            (None, false) => "as samples.toml asks".to_string(),
            (Some(Events::First(n)), r) => format!("--events {n}{}{seed}", if r { " --random" } else { "" }),
            (Some(_), _) => "--events all: every sample whole".to_string(),
            (None, true) => format!("--random{seed}: each sample's own number, chosen at random"),
        }
    }

    /// What a sample reads: its samples.toml choice as the options change it (a `--random` without a number refused).
    pub fn of(&self, sample: &lock::Sample) -> Result<Events, String> {
        use Events::{All, First, Random};
        let own = sample.events;
        let seed = |s| self.seed.unwrap_or(s);
        Ok(match (self.events, self.random) {
            (None, true) if own == All => {
                return Err(format!("{}: --random needs a number of events to choose, and samples.toml reads all of this sample: give --events N", sample.name))
            }
            (None, _) => match own {
                First(n) if self.random => Random { n, seed: seed(1) },
                Random { n, seed: s } => Random { n, seed: seed(s) },
                own => own,
            },
            (Some(First(n)), true) => Random { n, seed: seed(1) },
            (Some(events), _) => events,
        })
    }

    /// Refuse options that would change nothing: a `--seed` when no sample is chosen at random.
    pub fn check(&self, samples: &[lock::Sample]) -> Result<(), String> {
        let random = |s| matches!(self.of(s), Ok(Events::Random { .. }));
        if self.seed.is_some() && !samples.iter().any(random) {
            return Err("--seed is for a random choice, and no sample is chosen at random: add --random".into());
        }
        Ok(())
    }
}

/// What a press would read of each sample, from the lock alone, with the overrides a press takes:
/// `--events N` (every sample at most N), `--events all` (every sample whole, whatever
/// samples.toml says), `--random` (chosen at random rather than the first) and `--seed S`.
fn plan_all(path: &Path, choice: &Choice) -> Result<(), String> {
    let lock = read_lock(path)?;
    let started = std::time::Instant::now();
    let (lock, _) = crate::agree::planned(lock, &crate::plan::Choose::Options(choice.clone())).map_err(|said| said.join("\n"))?;
    println!(
        "What a press would read ({}), planned from the lock in {:.1} ms:",
        choice.said(),
        started.elapsed().as_secs_f64() * 1e3
    );
    for sample in &lock.samples {
        let read = sample.files.iter().filter(|f| f.stop != Some(0) && f.take != Some(0)).count();
        let files = format!("{read} of {} files", sample.files.len());
        let cut = sample
            .files
            .iter()
            .find_map(|f| Some((f.stop.filter(|n| *n > 0)?, f)))
            .map(|(stop, f)| format!("; stops at entry {} of {}", count(stop), f.at.rsplit('/').next().unwrap_or(&f.at)))
            .unwrap_or_default();
        println!("  {:40} {:>34}   {files}{cut}", sample.name, reading(sample));
    }
    let (read, total): (u64, u64) = lock.samples.iter().fold((0, 0), |(r, t), s| (r + s.events_read, t + s.events_total));
    println!("{} of {} events in all.", count(read), count(total));
    Ok(())
}

/// `rvr samples …` (`name`, as `samples plan`) with its options read: the exit status.
pub fn main(name: &str, options: &crate::options::Options) -> u8 {
    let file = Path::new(&options.words[0]);
    let result = match name {
        "samples expand" => expand(file, options.has("--json")).map(|_| true),
        "samples resolve" => resolve_all(file).map(|_| true),
        "samples check" => check(file),
        _ => match Choice::read(options) {
            Ok(choice) => plan_all(file, &choice).map(|_| true),
            Err(why) => return crate::wrong(name, &why),
        },
    };
    crate::report(name, result.map(|good| u8::from(!good)))
}

pub const USAGE: &str = "usage:
  rvr samples expand  samples.toml [--json]   the samples it describes, looked up nowhere
  rvr samples resolve samples.toml            ask every source; write samples.lock beside it
  rvr samples check   samples.lock            ask again; say what changed since the lock
  rvr samples plan    samples.lock [--events N|all] [--random] [--seed S]
                                              what a press would read of each sample";

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC 1.4, 3.1.4
    #[test]
    fn what_samples_toml_leaves_out_is_written_in_its_place() {
        let missing = [samples::FORMAT, samples::VERSION, samples::TABLES[0], samples::TABLES[1]];
        let source = "\u{feff}# my study\n[eras.UL18]\nera = 3\n\n[[sample]]\nname = \"a\"\nfiles = \"x\"\ntree = \"T\"\n";
        let text = mended(source, &missing).unwrap();
        // The file's first comment stays first; the tables go before the era's.
        assert!(text.starts_with("\u{feff}format = \"rvr-samples\"\nversion = 1\n\n# my study\n\n[places]\n\n[defaults]\n\n[eras.UL18]"), "{text:?}");
        assert!(samples::read(text.trim_start_matches('\u{feff}')).is_ok(), "{text}");
    }
}
