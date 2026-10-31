//! `samples.lock`: what each sample of samples.toml resolved to, file by file, with the identity
//! of every file (bytes and adler32 always; events, ROOT UUID and entries where they are known
//! without reading the events). Written by `resolve`, reviewed like code, never edited.

use super::plan::plan;
use super::copies::holding;
use super::remote::{digest_once, host_port, HttpsFile, Storage};
use super::spec::{Events, Plan, Source};
use cms_dbs::{DbsCatalog, DbsInstance};
use grid_catalog::{Catalog, CatalogFile, Did, GridHttps, Replicas, Resolver};
use rucio_client::{Location, Rucio, Server};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize)]
pub struct Lock {
    pub format: String,
    pub version: u32,
    /// When it was resolved, UTC.
    pub resolved: String,
    /// The SHA-256 of the samples.toml it was resolved from.
    pub samples_toml_sha256: String,
    pub sample: Vec<Locked>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Locked {
    /// What every tool calls it.
    pub name: String,
    /// What plots and the viewer show.
    pub label: String,
    /// The name of the [[sample]] entry it came from, before eras and trees were added.
    pub from: String,
    /// What the block asked for, its placeholders filled: paths and patterns, or a dataset.
    pub asked: Vec<String>,
    pub tree: String,
    /// What samples.toml asked for: a number of events, or "all".
    pub events: Value,
    /// "random" when the events are chosen at random (with `seed`); absent for the first ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Entries in all its files, and how many of them a press reads.
    pub events_total: u64,
    pub events_read: u64,
    pub metadata: Map<String, Value>,
    /// For a dataset: the storages that answered fastest when it was resolved, fastest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<Site>,
    pub file: Vec<File>,
}

/// A storage a dataset sample is read from first, and when Rucio cannot be reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    /// The storage element, as T2_US_Purdue.
    pub rse: String,
    /// The address, `https://host:port/path`, its files' logical names follow.
    pub prefix: String,
}

impl Site {
    /// The site of a copy of the file `lfn`, when its address is a prefix and that name.
    pub fn of(copy: &Location, lfn: &str) -> Option<Site> {
        let prefix = copy.url.strip_suffix(lfn).filter(|prefix| prefix.starts_with("https://"))?;
        Some(Site { rse: copy.rse.clone(), prefix: prefix.to_owned() })
    }

    /// The copy of the file `lfn` this site holds, only at a CMS storage door.
    pub fn copy(&self, lfn: &str) -> Option<Location> {
        // The address itself is held to the doors Rucio's answers are held to, so that a lock from
        // elsewhere sends the proxy to no other host, whatever its prefix and names say.
        let url = format!("{}{lfn}", self.prefix);
        let door = host_port(&url).filter(|_| lfn.starts_with('/'))?;
        let (host, port) = door.rsplit_once(':')?;
        let known = cms_doors().contains(&(host.to_owned(), port.parse().ok()?));
        known.then(|| Location { url, door: door.clone(), rse: self.rse.clone(), priority: 0 })
    }
}

/// CMS's storage doors, as `(host, port)`.
fn cms_doors() -> &'static HashSet<(String, u16)> {
    static DOORS: OnceLock<HashSet<(String, u16)>> = OnceLock::new();
    DOORS.get_or_init(|| rucio_client::copies::doors("cms"))
}

/// How many storages a dataset sample records.
const SITES: usize = 3;

/// How long resolve waits for storages to answer: those that answer later are not recorded.
const PROBED: Duration = Duration::from_secs(3);

/// How many of a sample's files with copies on disk are asked of Rucio for one it can be read from.
const ASKED: usize = 3;

/// The storages a dataset sample is read from first: the first of its files that Rucio says can be
/// read over HTTPS asked at every storage that holds it, all at once, and the first three storages
/// to answer that they hold it, as a press asks. None when the storages or Rucio cannot be asked:
/// a press then asks Rucio alone.
fn best_sites(ctx: &Context, files: &[File]) -> Vec<Site> {
    let (Some(storage), Some((_, rucio))) = (&ctx.storage, &ctx.catalog) else {
        return Vec::new();
    };
    let on_disk = files.iter().filter(|f| f.disk.as_ref().is_some_and(|disk| !disk.is_empty()));
    let Some((file, lfn, said)) = on_disk.take(ASKED).find_map(|f| {
        let lfn = f.lfn.clone()?;
        let said = rucio.copies(&[Did::cms(lfn.clone())]).ok()?.remove(&lfn).filter(|said| !said.copies.is_empty())?;
        Some((f, lfn, said))
    }) else {
        return Vec::new();
    };
    let (tell, heard) = mpsc::channel();
    for copy in said.copies.into_iter().filter_map(|copy| Site::of(&copy, &lfn).map(|site| (copy, site))) {
        let (agent, watch, tell, file) = (storage.agent(), storage.watch.clone(), tell.clone(), (*file).clone());
        // Each asks once, and the ones answering after the first three are not waited for.
        std::thread::spawn(move || {
            let right = holding(&file, digest_once(&agent, &copy.0.url, &watch)).is_ok();
            let _ = tell.send(right.then_some(copy.1));
        });
    }
    drop(tell);
    let (mut sites, until) = (Vec::new(), Instant::now() + PROBED);
    while sites.len() < SITES {
        match heard.recv_timeout(until.saturating_duration_since(Instant::now())) {
            // One storage, given twice (its davs and https addresses), takes one place.
            Ok(Some(site)) if sites.iter().all(|kept: &Site| kept.rse != site.rse) => sites.push(site),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    sites
}

/// A lock's value as TOML writes it, each number a TOML number: rowver keeps serde_json's numbers
/// exact (for .rvr headers), which TOML would write as tables. `name` is its key, to say which
/// value TOML cannot hold.
pub fn toml_value(value: &Value, name: &str) -> Result<toml::Value, String> {
    let cannot = || format!("samples.lock cannot hold {name}: TOML has no such value (nan, inf, or a number past its range). Give it a finite number in samples.toml.");
    Ok(match value {
        Value::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(whole), _) => toml::Value::Integer(whole),
            (None, Some(float)) if number.is_f64() && float.is_finite() => toml::Value::Float(float),
            _ => return Err(cannot()),
        },
        Value::String(text) => toml::Value::String(text.clone()),
        Value::Bool(truth) => toml::Value::Boolean(*truth),
        Value::Array(items) => toml::Value::Array(items.iter().map(|item| toml_value(item, name)).collect::<Result<_, _>>()?),
        Value::Object(fields) => {
            let fields = fields.iter().map(|(key, value)| Ok((key.clone(), toml_value(value, key)?)));
            toml::Value::Table(fields.collect::<Result<_, String>>()?)
        }
        Value::Null => return Err(cannot()),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct File {
    /// Where to open it: a local path or a root:// URL.
    pub at: String,
    pub bytes: u64,
    /// adler32 of the whole file, as 8 hex digits: from the storage or catalog, or computed here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adler32: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// For a catalog file: its logical name, events, and the sites holding it on disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<Vec<String>>,
    /// Read from the file's header (through its storage's door for a remote file): the ROOT UUID
    /// and the tree's entries. A catalog file's entries are its events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<u64>,
    /// What a press reads: entries 0..stop (when not all), or `take` entries chosen at random.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<u64>,
}

/// Whether `name` matches `pattern`, where `*` is any run of characters and `?` one character.
pub fn matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn pattern(text: &str) -> bool {
    text.contains(['*', '?'])
}

/// adler32 of a file, read once.
pub fn adler32_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut sum = 1;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(format!("{sum:08x}"));
        }
        sum = adler32(sum, &buf[..n]);
    }
}

/// adler32 `sum` carried on over `data`, in sixteen lanes that LLVM vectorizes; never inlined,
/// so its code does not change with its callers.
#[inline(never)]
fn adler32(sum: u32, data: &[u8]) -> u32 {
    const MOD: u64 = 65521;
    let (mut a, mut b) = (u64::from(sum & 0xffff), u64::from(sum >> 16));
    // 256 words of 16 bytes a block, so a lane of `ones` (at most 255 * 256) fits a u16.
    for block in data.chunks(4096) {
        let (mut ones, mut twos) = ([0u16; 16], [0u32; 16]);
        let words = block.chunks_exact(16);
        let rest = words.remainder();
        for word in words {
            for j in 0..16 {
                ones[j] += u16::from(word[j]);
                twos[j] += u32::from(ones[j]);
            }
        }
        // Byte j of word t of n is added to b 16 * (n - t) - j times.
        b += (block.len() - rest.len()) as u64 * a;
        for j in 0..16 {
            a += u64::from(ones[j]);
            b += 16 * u64::from(twos[j]) - j as u64 * u64::from(ones[j]);
        }
        for &byte in rest {
            a += u64::from(byte);
            b += a;
        }
        (a, b) = (a % MOD, b % MOD);
    }
    ((b << 16) | a) as u32
}

fn local(text: &str, base: &Path) -> PathBuf {
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|_| PathBuf::from(text)),
        None => PathBuf::from(text),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

fn utc(time: std::time::SystemTime) -> String {
    let seconds = time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    voms_proxy::utc(seconds)
}

/// How a file differs from what the lock recorded of it, in words, if it does. A size or an
/// adler32 is compared only when both the lock and the storage give one; an adler32 as a number.
pub fn differs(file: &File, bytes: Option<u64>, adler32: Option<&str>) -> Option<String> {
    let number = |hex: Option<&str>| hex.and_then(|h| u32::from_str_radix(h.trim(), 16).ok());
    let size = bytes.is_some_and(|b| b != file.bytes && file.bytes > 0);
    let sum =
        matches!((number(file.adler32.as_deref()), number(adler32)), (Some(a), Some(b)) if a != b);
    let shown = |b: Option<u64>| b.map_or("? bytes".to_string(), |b| format!("{b} bytes"));
    (size || sum).then(|| {
        format!(
            "{}: it was {}, adler32 {}; it is now {}, adler32 {}",
            file.at,
            shown(Some(file.bytes)),
            file.adler32.as_deref().unwrap_or("?"),
            shown(bytes),
            adler32.unwrap_or("?")
        )
    })
}

/// The UUID of a ROOT file and the entries of one of its trees, from its header and the tree's
/// record alone; the trees it has when the one asked for is not there.
fn root_identity<S: hep_root::ByteSource>(
    source: S,
    shown: &str,
    tree: &str,
) -> Result<(String, u64), String> {
    let file = hep_root::RootFile::open(source)
        .map_err(|e| format!("{shown} is not a ROOT file this reader can open: {e}"))?;
    let uuid = file.header().uuid_string();
    match file.tree_entries(tree) {
        Ok(entries) => Ok((uuid, entries.max(0) as u64)),
        Err(hep_root::Error::KeyNotFound(_)) => {
            let trees: Vec<String> = file
                .trees_listing()
                .map(|l| l.trees.into_iter().map(|t| t.path).collect())
                .unwrap_or_default();
            Err(format!(
                "{shown} has no tree {tree:?}. Its trees: {}.",
                if trees.is_empty() {
                    "none".into()
                } else {
                    trees.join(", ")
                }
            ))
        }
        Err(e) => Err(format!("{shown}: its tree {tree:?} could not be read: {e}")),
    }
}

/// What resolving needs: the folder samples.toml is in, storage access, and the catalog.
pub struct Context {
    pub base: PathBuf,
    pub storage: Option<Storage>,
    pub catalog: Option<(DbsCatalog, Rucio)>,
    /// Why storage or the catalog is unavailable, said once to the user if a sample needs them.
    pub offline: String,
}

impl Context {
    pub fn new(
        base: PathBuf,
        proxy: Option<String>,
        proxy_said: String,
        ca: Option<String>,
    ) -> Self {
        let storage = Storage::new(proxy.as_deref(), ca.as_deref());
        let catalog = proxy.as_ref().map(|p| {
            let transport: Arc<dyn grid_catalog::Transport> =
                Arc::new(GridHttps::new(p.into(), ca.clone().map(PathBuf::from)));
            (
                DbsCatalog::new(DbsInstance::Global, transport.clone()),
                Rucio::new(Server::cms(), transport),
            )
        });
        let offline = match &storage {
            Err(e) => e.clone(),
            Ok(_) => proxy_said,
        };
        Context {
            base,
            storage: storage.ok(),
            catalog,
            offline,
        }
    }

    fn storage(&self) -> Result<&Storage, String> {
        self.storage.as_ref().ok_or_else(|| self.offline.clone())
    }
}

/// The files of one path, URL or pattern.
fn files_of(ctx: &Context, asked: &str, tree: &str) -> Result<Vec<File>, String> {
    let remote = asked.contains("://");
    let (folder, name) = asked.rsplit_once('/').unwrap_or((".", asked));
    if pattern(folder) {
        return Err(format!("{asked}: a pattern may only be in the file name, not in a folder. Write one pattern per folder."));
    }
    if remote {
        let storage = ctx.storage()?;
        let listed = storage.list(folder)?;
        let found: Vec<_> = listed
            .iter()
            .filter(|e| {
                if pattern(name) {
                    matches(name, &e.name)
                } else {
                    e.name == name
                }
            })
            .collect();
        if found.is_empty() {
            let near = nearest(name, listed.iter().map(|e| e.name.as_str()));
            return Err(format!(
                "{asked} matched no file ({} files in that folder).{}",
                listed.len(),
                if near.is_empty() {
                    String::new()
                } else {
                    format!(" Did you mean: {}?", near.join(", "))
                }
            ));
        }
        found
            .iter()
            .map(|e| {
                let at = format!("{folder}/{}", e.name);
                let (_, adler32) = storage.digest(&at)?;
                let (uuid, entries) =
                    root_identity(HttpsFile::new(storage, &at, e.bytes)?, &at, tree)?;
                Ok(File {
                    at,
                    bytes: e.bytes,
                    adler32,
                    modified: e.modified.clone(),
                    lfn: None,
                    events: None,
                    disk: None,
                    uuid: Some(uuid),
                    entries: Some(entries),
                    stop: None,
                    take: None,
                })
            })
            .collect()
    } else {
        let dir = local(folder, &ctx.base);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{asked}: the folder {} cannot be read: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let found: Vec<&String> = names
            .iter()
            .filter(|n| {
                if pattern(name) {
                    matches(name, n)
                } else {
                    *n == name
                }
            })
            .collect();
        if found.is_empty() {
            let near = nearest(name, names.iter().map(String::as_str));
            return Err(format!(
                "{asked} matched no file in {}.{}",
                dir.display(),
                if near.is_empty() {
                    String::new()
                } else {
                    format!(" Did you mean: {}?", near.join(", "))
                }
            ));
        }
        found
            .into_iter()
            .map(|n| {
                let path = dir.join(n);
                let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                let (uuid, entries) = root_identity(
                    hep_root::LocalFile::open(&path).map_err(|e| e.to_string())?,
                    &path.display().to_string(),
                    tree,
                )?;
                Ok(File {
                    at: path.display().to_string(),
                    bytes: meta.len(),
                    adler32: Some(adler32_file(&path).map_err(|e| e.to_string())?),
                    modified: meta.modified().ok().map(utc),
                    lfn: None,
                    events: None,
                    disk: None,
                    uuid: Some(uuid),
                    entries: Some(entries),
                    stop: None,
                    take: None,
                })
            })
            .collect()
    }
}

/// How many single-character edits turn `a` into `b`.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(x != *y))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The few `names` nearest to what was asked, when they are near enough to be what was meant:
/// within a tenth of its length in edits (at least three).
pub fn nearest<'a>(asked: &str, names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let limit = (asked.chars().count() / 10).max(3);
    let mut ranked: Vec<(usize, &str)> = names
        .map(|n| (distance(asked, n), n))
        .filter(|(d, _)| *d <= limit)
        .collect();
    ranked.sort();
    ranked.into_iter().take(3).map(|(_, n)| n).collect()
}

/// The files of a catalog dataset: names, events, sizes and adler32 from DBS, disk sites from Rucio.
fn dataset_files(ctx: &Context, dataset: &str) -> Result<Vec<File>, String> {
    let (dbs, rucio) = ctx.catalog.as_ref().ok_or_else(|| ctx.offline.clone())?;
    let did = Did::cms(dataset);
    let files: Vec<CatalogFile> = dbs.files(&did).map_err(|e| e.to_string())?;
    if files.is_empty() {
        let parts: Vec<&str> = dataset.split('/').collect();
        let hint = match parts.as_slice() {
            ["", primary, _, tier] => {
                let found: Vec<String> = dbs
                    .search(&format!("/{primary}/*/{tier}"))
                    .map(|f| f.into_iter().map(|d| d.dataset).collect())
                    .unwrap_or_default();
                nearest(dataset, found.iter().map(String::as_str))
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            }
            _ => Vec::new(),
        };
        return Err(format!(
            "the dataset {dataset} has no valid files in DBS.{}",
            if hint.is_empty() {
                String::new()
            } else {
                format!(" Did you mean: {}?", hint.join(", "))
            }
        ));
    }
    let urls = cms_dbs::CmsResolver::new("root://cms-xrd-global.cern.ch/")
        .and_then(|r| r.urls(&files))
        .map_err(|e| e.to_string())?;
    let replicas = rucio
        .replicas(&files.iter().map(|f| f.did.clone()).collect::<Vec<_>>())
        .map_err(|e| e.to_string())?;
    Ok(files
        .iter()
        .zip(urls)
        .zip(replicas)
        .map(|((f, url), replica)| File {
            at: url.unwrap_or_else(|| f.did.name.clone()),
            bytes: f.bytes.unwrap_or(0),
            adler32: f.checksum.clone(),
            modified: None,
            lfn: Some(f.did.name.clone()),
            events: f.events,
            disk: Some(replica.disk),
            uuid: None,
            entries: f.events,
            stop: None,
            take: None,
        })
        .collect())
}

/// One sample resolved: every file it names, and its metadata with {file} filled.
pub fn resolve(ctx: &Context, plan: &Plan) -> Result<Locked, String> {
    let (asked, files) = match &plan.source {
        Source::Files(list) => {
            let mut files = Vec::new();
            for text in list {
                files.extend(files_of(ctx, text, &plan.tree)?);
            }
            (list.clone(), files)
        }
        // DBS counts a file's events in its Events tree, and the lock's entries are DBS's.
        Source::Dataset(_) if plan.tree != "Events" => {
            return Err(format!("it reads the tree {:?} of a dataset, but rvr knows a dataset file's entries only from DBS, which counts its Events tree, so a dataset sample reads tree = \"Events\". Set it so; the Runs and LuminosityBlocks trees of a dataset cannot be pressed yet.", plan.tree))
        }
        Source::Dataset(d) => (vec![d.clone()], dataset_files(ctx, d)?),
    };
    let sites = match &plan.source {
        Source::Dataset(_) => best_sites(ctx, &files),
        Source::Files(_) => Vec::new(),
    };
    let mut metadata = plan.metadata.clone();
    for (key, value) in metadata.iter_mut() {
        if let Value::String(s) = value {
            if s.contains("{file}") {
                let [one] = files.as_slice() else {
                    return Err(format!("{}: {key} uses {{file}}, which names a sample's one file, but it has {} files", plan.name, files.len()));
                };
                *s = s.replace("{file}", one.at.rsplit('/').next().unwrap_or(&one.at));
            }
        }
    }
    let mut locked = Locked {
        name: plan.name.clone(),
        label: plan.label.clone(),
        from: plan.from.clone(),
        asked,
        tree: plan.tree.clone(),
        events: Value::Null,
        choose: None,
        seed: None,
        events_total: 0,
        events_read: 0,
        metadata,
        sites,
        file: files,
    };
    planned(&mut locked, plan.events)?;
    Ok(locked)
}

/// Write into a locked sample what a press reads of each file for these `events`.
pub fn planned(sample: &mut Locked, events: Events) -> Result<(), String> {
    let entries: Vec<u64> = sample
        .file
        .iter()
        .map(|f| {
            f.entries.ok_or_else(|| {
                format!(
                    "{}: {} has no known entries, so what to read cannot be planned",
                    sample.name, f.at
                )
            })
        })
        .collect::<Result<_, _>>()?;
    let reads = plan(&entries, events);
    (sample.events, sample.choose, sample.seed) = match events {
        Events::All => ("all".into(), None, None),
        Events::First(n) => (n.into(), None, None),
        Events::Random { n, seed } => (n.into(), Some("random".into()), Some(seed)),
    };
    for (file, read) in sample.file.iter_mut().zip(&reads) {
        (file.stop, file.take) = (read.stop, read.take);
    }
    sample.events_total = entries.iter().sum();
    sample.events_read = reads.iter().map(|r| r.read).sum();
    Ok(())
}

/// A locked sample's `events` as samples.toml asked for them.
pub fn asked(sample: &Locked) -> Events {
    match (sample.events.as_u64(), sample.choose.as_deref()) {
        (Some(n), Some("random")) => Events::Random {
            n,
            seed: sample.seed.unwrap_or(1),
        },
        (Some(n), _) => Events::First(n),
        _ => Events::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(matches("Run2012*_TauPlusX.root", "Run2012B_TauPlusX.root"));
        assert!(matches("*.root", "a.root"));
        assert!(!matches(
            "Run2012?_Tau*.root",
            "Run2012BC_DoubleMuParked_Muons.root"
        ));
        assert!(matches(
            "SMS-Signal*_Summer22_130X.root",
            "SMS-Signal-2L_TuneCP5_13p6TeV_madgraphMLM-pythia8_Summer22_130X.root"
        ));
    }

    #[test]
    fn near_names_are_suggested_and_far_ones_not() {
        let names = [
            "ST_s-channel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20UL18_106X.root",
            "TTJets.root",
        ];
        assert_eq!(
            nearest(
                "ST_s-chanel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20UL18_106X.root",
                names.into_iter()
            ),
            [names[0]]
        );
        assert!(nearest("WJets.root", names.into_iter()).len() <= 1);
        assert!(nearest("completely_different_name.root", names.into_iter()).is_empty());
    }

    #[test]
    fn adler32_of_known_text() {
        let path = std::env::temp_dir().join(format!("rvr-samples-adler-{}", std::process::id()));
        std::fs::write(&path, b"Wikipedia").unwrap();
        assert_eq!(adler32_file(&path).unwrap(), "11e60398");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn adler32_lanes_agree_with_the_plain_sum() {
        let plain = |sum: u32, data: &[u8]| {
            let (mut a, mut b) = (sum & 0xffff, sum >> 16);
            for &byte in data {
                a = (a + u32::from(byte)) % 65521;
                b = (b + a) % 65521;
            }
            (b << 16) | a
        };
        let mixed: Vec<u8> = (0..30_011u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        for data in [vec![0xff; 30_011], mixed] {
            for len in [0, 1, 15, 16, 17, 4095, 4096, 4097, 5552, 5553, 11_111, 30_011] {
                assert_eq!(adler32(1, &data[..len]), plain(1, &data[..len]), "{len} bytes");
            }
            // Carried on across calls of uneven lengths, from sums as high as they go.
            let (head, tail) = data.split_at(7_777);
            assert_eq!(adler32(adler32(1, head), tail), plain(1, &data));
            assert_eq!(adler32(0xfff0_fff0, &data), plain(0xfff0_fff0, &data));
        }
    }

    #[test]
    fn a_number_is_written_as_toml_holds_it_or_refused_by_name() {
        assert_eq!(toml_value(&serde_json::json!(3), "era").unwrap(), toml::Value::Integer(3));
        assert_eq!(toml_value(&serde_json::json!(2.5), "xsec").unwrap(), toml::Value::Float(2.5));
        assert_eq!(toml_value(&serde_json::json!(3.0), "scale").unwrap(), toml::Value::Float(3.0));
        // A whole number past TOML's range is refused, not written as a float that reads back otherwise.
        assert!(toml_value(&serde_json::json!(u64::MAX), "seed").unwrap_err().starts_with("samples.lock cannot hold seed:"));
        assert!(toml_value(&serde_json::json!({"a": null}), "metadata").unwrap_err().starts_with("samples.lock cannot hold a:"));
    }
}
