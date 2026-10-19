//! `rvr samples`: samples.toml in, samples.lock out, and what a press reads of each sample.
//! See the crate's README for the files and every command.

mod multipart;
mod plan;
pub(crate) mod remote;
pub(crate) mod resolve;
pub(crate) mod spec;

use resolve::{Context, Lock, Locked};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A count with thousands separated: 1,234,567.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "all 1,234 events", or "50,000 of 1,234,567 events (random)".
fn reading(sample: &Locked) -> String {
    if sample.events_read == sample.events_total {
        format!("all {} events", count(sample.events_total))
    } else {
        format!(
            "{} of {} events{}",
            count(sample.events_read),
            count(sample.events_total),
            if sample.choose.is_some() {
                " (random)"
            } else {
                ""
            }
        )
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b => format!("{:.1} MB", b as f64 / 1e6),
    }
}

/// The grid proxy to present, and a sentence for when there is none that works.
pub(crate) fn proxy() -> (Option<String>, String, Option<String>) {
    let credentials = grid_proxy::Credentials::found(None, None, &grid_proxy::Places::here());
    let status = grid_proxy::status(&credentials);
    // Extra certificate authorities, when a folder of them is named; grid-trust already trusts the
    // public roots and CERN's grid authorities, which FNAL and CERN EOS are signed by.
    let ca = std::env::var("X509_CERT_DIR")
        .ok()
        .or(credentials.authorities.clone());
    if status.usable() {
        (status.path.clone(), String::new(), ca)
    } else {
        (None, format!("No usable grid proxy: {} Remote folders and datasets need one; local files do not.", status.message), ca)
    }
}

fn expand(path: &Path, json: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let plans = spec::expand(&text)?;
    if json {
        let list: Vec<_> = plans.iter().map(|p| serde_json::json!({"name": p.name, "label": p.label, "tree": p.tree, "metadata": p.metadata,
            "source": match &p.source { spec::Source::Files(f) => serde_json::json!({"files": f}), spec::Source::Dataset(d) => serde_json::json!({"dataset": d}) }})).collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap());
        return Ok(());
    }
    let entries: BTreeSet<usize> = plans.iter().map(|p| p.entry).collect();
    println!(
        "{} describes {} samples from {} [[sample]] entries:",
        path.display(),
        plans.len(),
        entries.len()
    );
    println!("  {:34} {:40} {:18} reads", "name", "label", "tree");
    for p in &plans {
        let source = match &p.source {
            spec::Source::Files(f) => f.join(" + "),
            spec::Source::Dataset(d) => format!("dataset {d}"),
        };
        println!("  {:34} {:40} {:18} {}", p.name, p.label, p.tree, source);
    }
    Ok(())
}

fn resolve_all(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let plans = spec::expand(&text)?;
    let (proxy, said, ca) = proxy();
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let ctx = Context::new(base, proxy, said, ca);
    let started = std::time::Instant::now();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<Locked, String>>>> =
        plans.iter().map(|_| Default::default()).collect();
    std::thread::scope(|s| {
        for _ in 0..12 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(plan) = plans.get(i) else { break };
                *results[i].lock().unwrap() = Some(resolve::resolve(&ctx, plan));
            });
        }
    });
    let mut locked = Vec::new();
    let mut failed = 0;
    for (plan, result) in plans.iter().zip(results) {
        match result.into_inner().unwrap().unwrap() {
            Ok(l) => {
                let bytes: u64 = l.file.iter().map(|f| f.bytes).sum();
                let files = if l.file.len() == 1 {
                    "1 file ".to_string()
                } else {
                    format!("{} files", l.file.len())
                };
                println!(
                    "  ok    {:40} {files:>9} {:>10}   {}",
                    l.name,
                    size(bytes),
                    reading(&l)
                );
                locked.push(l);
            }
            Err(why) => {
                println!("  FAIL  {}: {why}", plan.name);
                failed += 1;
            }
        }
    }
    let distinct: BTreeMap<&str, u64> = locked
        .iter()
        .flat_map(|l| l.file.iter().map(|f| (f.at.as_str(), f.bytes)))
        .collect();
    println!(
        "\n{} samples, {} distinct files, {} in {:.1} s.",
        locked.len(),
        distinct.len(),
        size(distinct.values().sum()),
        started.elapsed().as_secs_f64()
    );
    if let Some(storage) = &ctx.storage {
        let folders: BTreeSet<&str> = locked
            .iter()
            .flat_map(|l| l.file.iter())
            .filter(|f| f.at.contains("://") && f.lfn.is_none())
            .filter_map(|f| f.at.rsplit_once('/').map(|(d, _)| d))
            .collect();
        let mut unread = Vec::new();
        for folder in folders {
            for entry in storage.list(folder).unwrap_or_default() {
                let at = format!("{folder}/{}", entry.name);
                if !distinct.contains_key(at.as_str()) && entry.name.ends_with(".root") {
                    unread.push(at);
                }
            }
        }
        if !unread.is_empty() {
            println!("The folders read also hold {} .root files no sample reads (not an error; check none was meant):", unread.len());
            for at in unread.iter().take(3) {
                println!("  {at}");
            }
            if unread.len() > 3 {
                println!("  … and {} more", unread.len() - 3);
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} of {} samples could not be resolved; samples.lock was not written. Fix the lines marked FAIL and run resolve again.", plans.len()));
    }
    let lock = Lock {
        format: "rvr-samples-lock".into(),
        version: 1,
        resolved: grid_proxy::utc(grid_proxy::now()),
        samples_toml_sha256: sha256(text.as_bytes()),
        sample: locked,
    };
    let out = path.with_file_name("samples.lock");
    let body = toml::to_string(&lock).map_err(|e| e.to_string())?;
    let header = "# samples.lock: what samples.toml resolved to, file by file. Written by `rvr samples resolve`;\n# do not edit it. Review a change to it like a change to code: a changed line is a changed input.\n\n";
    std::fs::write(&out, format!("{header}{body}"))
        .map_err(|e| format!("{} cannot be written: {e}", out.display()))?;
    println!(
        "Wrote {} ({}).",
        out.display(),
        size(header.len() as u64 + body.len() as u64)
    );
    Ok(())
}

fn check(path: &Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let lock: Lock = toml::from_str(&text)
        .map_err(|e| format!("{} is not a samples.lock: {e}", path.display()))?;
    let mut same = true;
    let samples = path.with_file_name("samples.toml");
    if let Ok(now) = std::fs::read(&samples) {
        if sha256(&now) != lock.samples_toml_sha256 {
            println!("samples.toml has changed since the lock was written ({}): run resolve to see what it now asks for.", lock.resolved);
            same = false;
        }
    }
    let (proxy, said, ca) = proxy();
    let ctx = Context::new(
        path.parent().map(Path::to_path_buf).unwrap_or_default(),
        proxy,
        said,
        ca,
    );
    let files: BTreeMap<&str, &resolve::File> = lock
        .sample
        .iter()
        .flat_map(|s| s.file.iter().map(|f| (f.at.as_str(), f)))
        .collect();
    let mut changed = Vec::new();
    for (at, f) in &files {
        let now = if at.contains("://") && f.lfn.is_none() {
            ctx.storage
                .as_ref()
                .ok_or(ctx.offline.clone())
                .and_then(|s| s.digest(at))
        } else if f.lfn.is_some() {
            continue;
        } else {
            // A relative path is relative to the lock's folder.
            let local = ctx.base.join(at);
            std::fs::metadata(&local)
                .map_err(|_| format!("{at} no longer exists"))
                .and_then(|m| {
                    Ok((
                        Some(m.len()),
                        Some(resolve::adler32_file(&local).map_err(|e| e.to_string())?),
                    ))
                })
        };
        match now {
            Ok((bytes, adler)) => changed.extend(resolve::differs(f, bytes, adler.as_deref())),
            Err(why) => changed.push(why),
        }
    }
    let catalog = files.values().filter(|f| f.lfn.is_some()).count();
    if changed.is_empty() {
        println!(
            "All {} storage and local files are as the lock recorded them ({}).{}",
            files.len() - catalog,
            lock.resolved,
            if catalog > 0 {
                format!(" {catalog} catalog files are checked when pressed.")
            } else {
                String::new()
            }
        );
    } else {
        same = false;
        println!(
            "{} {} from the lock:",
            changed.len(),
            if changed.len() == 1 {
                "file differs"
            } else {
                "files differ"
            }
        );
        for c in &changed {
            println!("  {c}");
        }
        println!("A press from this lock would refuse them. If the change is wanted, run resolve again and review the new lock.");
    }
    Ok(same)
}

/// What `--events N|all`, `--random` and `--seed S` ask of every sample, for plan and press.
#[derive(Clone, Copy, Debug, Default)]
pub struct Choice<'a> {
    pub events: Option<&'a str>,
    pub random: bool,
    pub seed: Option<u64>,
}

impl Choice<'_> {
    /// Read `--seed`'s value.
    pub fn seed(text: &str) -> Result<u64, String> {
        text.parse()
            .map_err(|_| format!("--seed is a whole number, not {text:?}"))
    }

    /// The options in words.
    pub fn said(&self) -> String {
        match (self.events, self.random) {
            (None, false) => "as samples.toml asks".to_string(),
            (Some("all"), _) => "--events all: every sample whole".to_string(),
            (Some(n), r) => format!("--events {n}{}", if r { " --random" } else { "" }),
            (None, true) => "--random: each sample's own number, chosen at random".to_string(),
        }
    }

    /// What a sample reads: what samples.toml asked of it, changed as the options say. A
    /// `--random` that has no number of events to choose is refused, not ignored.
    pub fn of(&self, sample: &Locked) -> Result<spec::Events, String> {
        use spec::Events::{All, First, Random};
        let own = resolve::asked(sample);
        let seed = |s| self.seed.unwrap_or(s);
        Ok(match (self.events, self.random) {
            (Some("all"), true) => {
                return Err("--random chooses N events at random: give it with --events N, not --events all".into())
            }
            (None, true) if own == All => {
                return Err(format!("{}: --random needs a number of events to choose, and samples.toml reads all of this sample: give --events N", sample.name))
            }
            (None, _) => match own {
                First(n) if self.random => Random { n, seed: seed(1) },
                Random { n, seed: s } => Random { n, seed: seed(s) },
                own => own,
            },
            (Some("all"), false) => All,
            (Some(n), random) => {
                let n: u64 = n
                    .replace('_', "")
                    .parse()
                    .map_err(|_| format!("--events is a number of events or all, not {n:?}"))?;
                match random {
                    true => Random { n, seed: seed(1) },
                    false => First(n),
                }
            }
        })
    }

    /// Refuse options that would change nothing: a `--seed` when no sample is chosen at random.
    pub fn check(&self, samples: &[Locked]) -> Result<(), String> {
        let random = |s| matches!(self.of(s), Ok(spec::Events::Random { .. }));
        if self.seed.is_some() && !samples.iter().any(random) {
            return Err(
                "--seed is for a random choice, and no sample is chosen at random: add --random"
                    .into(),
            );
        }
        Ok(())
    }
}

/// What a press would read of each sample, from the lock alone, with the overrides a press takes:
/// `--events N` (every sample at most N), `--events all` (every sample whole, whatever
/// samples.toml says), `--random` (chosen at random rather than the first) and `--seed S`.
fn plan_all(path: &Path, flags: &[&str]) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let mut lock: Lock = toml::from_str(&text)
        .map_err(|e| format!("{} is not a samples.lock: {e}", path.display()))?;
    let value = |flag: &str| {
        flags.iter().position(|f| *f == flag).map(|i| {
            flags
                .get(i + 1)
                .copied()
                .ok_or(format!("{flag} needs a value"))
        })
    };
    let choice = Choice {
        events: value("--events").transpose()?,
        random: flags.contains(&"--random"),
        seed: value("--seed").transpose()?.map(Choice::seed).transpose()?,
    };
    let known = ["--events", "--random", "--seed"];
    for (i, f) in flags.iter().enumerate() {
        let is_value = i > 0 && ["--events", "--seed"].contains(&flags[i - 1]);
        if !is_value && !known.contains(f) {
            return Err(format!(
                "plan does not know {f}; it takes --events N, --events all, --random and --seed S"
            ));
        }
    }
    choice.check(&lock.sample)?;
    let started = std::time::Instant::now();
    let said = choice.said();
    for sample in &mut lock.sample {
        let want = choice.of(sample)?;
        resolve::planned(sample, want)?;
    }
    println!(
        "What a press would read ({said}), planned from the lock in {:.1} ms:",
        started.elapsed().as_secs_f64() * 1e3
    );
    for sample in &lock.sample {
        let read: Vec<&resolve::File> = sample
            .file
            .iter()
            .filter(|f| f.stop != Some(0) && f.take != Some(0))
            .collect();
        let files = format!("{} of {} files", read.len(), sample.file.len());
        let cut = sample
            .file
            .iter()
            .find(|f| matches!(f.stop, Some(n) if n > 0))
            .map(|f| {
                format!(
                    "; stops at entry {} of {}",
                    count(f.stop.unwrap()),
                    f.at.rsplit('/').next().unwrap_or(&f.at)
                )
            })
            .unwrap_or_default();
        println!(
            "  {:40} {:>34}   {files}{cut}",
            sample.name,
            reading(sample)
        );
    }
    let (read, total): (u64, u64) = lock
        .sample
        .iter()
        .fold((0, 0), |(r, t), s| (r + s.events_read, t + s.events_total));
    println!("{} of {} events in all.", count(read), count(total));
    Ok(())
}

/// `rvr samples …`: the exit status of the command named.
pub fn main(words: &[&str]) -> u8 {
    let result = match words {
        ["expand", file] => expand(Path::new(file), false).map(|_| true),
        ["expand", file, "--json"] => expand(Path::new(file), true).map(|_| true),
        ["resolve", file] => resolve_all(Path::new(file)).map(|_| true),
        ["check", file] => check(Path::new(file)),
        ["plan", file, flags @ ..] => plan_all(Path::new(file), flags).map(|_| true),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(why) => {
            eprintln!("{why}");
            if why == USAGE {
                2
            } else {
                1
            }
        }
    }
}

pub const USAGE: &str = "usage:
  rvr samples expand  samples.toml [--json]   the samples it describes, looked up nowhere
  rvr samples resolve samples.toml            ask every source; write samples.lock beside it
  rvr samples check   samples.lock            ask again; say what changed since the lock
  rvr samples plan    samples.lock [--events N|all] [--random] [--seed S]
                                              what a press would read of each sample";
