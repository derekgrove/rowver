//! `samples.lock`: what each sample of samples.toml resolved to, file by file, with the identity
//! of every file (bytes and adler32 always; events, ROOT UUID and entries where they are known
//! without reading the events). Written by `resolve`, reviewed like code, never edited.

use super::plan::plan;
use super::remote::{HttpsFile, Storage};
use super::spec::{Events, Plan, Source};
use cms_dbs::{DbsCatalog, DbsInstance};
use grid_catalog::{Catalog, CatalogFile, Did, GridHttps, Replicas, Resolver};
use rucio_client::{Rucio, Server};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    pub file: Vec<File>,
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
    let (mut a, mut b) = (1u32, 0u32);
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for chunk in buf[..n].chunks(5552) {
            for &byte in chunk {
                a += byte as u32;
                b += a;
            }
            a %= 65521;
            b %= 65521;
        }
    }
    Ok(format!("{:08x}", (b << 16) | a))
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
}
