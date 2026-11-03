//! `rvr samples resolve`, one sample at a time: every file it asks for, found and pinned (its size,
//! its adler32 when its storage gives one, its ROOT UUID and its tree's entries), and the sample's
//! branch list, from its first file; no event read. The lock itself is rowver's
//! (`rowver::study::lock`).

use super::remote::{HttpsFile, Storage};
use hep_root::columns::{inventory, Kind, Shape};
use rowver::study::lock::{self, Branch, Branches, File};
use rowver::study::meant;
use rowver::study::samples::Sample;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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

/// Where a local path of samples.toml or the lock is: under the home folder (`~/`), absolute, or
/// relative to `base`, the folder holding samples.toml.
pub fn local(text: &str, base: &Path) -> PathBuf {
    let home = |rest: &str| std::env::var("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|_| PathBuf::from(text));
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => home(rest),
        None if text == "~" => home(""),
        None => PathBuf::from(text),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

/// How a file differs from what the lock recorded of it, in words, if it does. A size is compared
/// when the storage gives one, an adler32 when both the lock and the storage give one, as a number.
pub fn differs(file: &File, bytes: Option<u64>, adler32: Option<&str>) -> Option<String> {
    let number = |hex: Option<&str>| hex.and_then(|h| u32::from_str_radix(h.trim(), 16).ok());
    let size = bytes.is_some_and(|b| b != file.bytes);
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

/// A hep-root number kind as a `.rvr` kind's name.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Bool => "b1",
        Kind::I8 => "i1",
        Kind::U8 => "u1",
        Kind::I16 => "i2",
        Kind::U16 => "u2",
        Kind::I32 => "i4",
        Kind::U32 => "u4",
        Kind::I64 => "i8",
        Kind::U64 => "u8",
        Kind::F32 => "f4",
        Kind::F64 => "f8",
    }
}

/// A tree's branch list, as the lock writes it.
pub fn branch_list(tree: &hep_root::Tree) -> Branches {
    Branches::new(branches_named(tree, |_| true))
}

/// The branches of a tree whose name `wanted` takes, in the tree's order: each one's name, the
/// kind rvr reads it as (or `none`, with why), and its shape. A name the tree gives twice is
/// listed once, the first time.
pub fn branches_named(tree: &hep_root::Tree, wanted: impl Fn(&str) -> bool) -> Vec<Branch> {
    let mut seen = std::collections::BTreeSet::new();
    // hep-root's inventory gives every branch of the tree, depth first as `Tree::walk`: one entry each.
    inventory(tree)
        .into_iter()
        .zip(tree.walk())
        .filter(|(info, _)| wanted(&info.path) && seen.insert(info.path.clone()))
        .map(|(info, (_, branch))| {
            let kind = match &info.shape {
                Shape::Unreadable { reason } => Err(reason.clone()),
                Shape::Vector(kind) => Ok(*kind),
                _ => Kind::of(branch).map_err(|e| e.to_string()),
            };
            let shape = match info.shape {
                Shape::Scalar => lock::Shape::Flat,
                Shape::Fixed(n) => lock::Shape::Fixed(n as u64),
                Shape::JaggedFixed { len, .. } => lock::Shape::JaggedFixed(len as u64),
                _ => lock::Shape::Jagged,
            };
            match kind {
                Ok(kind) => Branch { name: info.path, kind: kind_name(kind).into(), shape, why: None },
                Err(why) => Branch { name: info.path, kind: "none".into(), shape: lock::Shape::Flat, why: Some(why) },
            }
        })
        .collect()
}

/// How `have` falls short of `want`, in words, by name, kind and shape (not why a branch is
/// unreadable, which words can change): the first few of `want` it lacks, or has as another kind
/// or shape; empty when it has them all as they are.
pub fn differ<'a>(want: impl Iterator<Item = &'a Branch>, have: impl Iterator<Item = &'a Branch>) -> String {
    let index = |list: &mut dyn Iterator<Item = &'a Branch>| list.map(|b| (b.name.as_str(), (b.kind.as_str(), b.shape))).collect::<std::collections::BTreeMap<_, _>>();
    let (want, have) = (index(&mut { want }), index(&mut { have }));
    let mut said = Vec::new();
    for (name, b) in &want {
        match have.get(name) {
            None => said.push(format!("{name} missing")),
            Some(h) if h != b => said.push(format!("{name} {} {} here, {} {} in the sample", h.0, h.1, b.0, b.1)),
            Some(_) => {}
        }
    }
    let more = said.len().saturating_sub(5);
    said.truncate(5);
    if more > 0 {
        said.push(format!("and {more} more"));
    }
    said.join(", ")
}

/// The UUID of a ROOT file and the entries of one of its trees, and with `listed` the tree's
/// branch list, from its header and the tree's record alone (the start of the record, which holds
/// the entries, when no list is asked: the whole record of a skim can be megabytes); the trees it
/// has when the one asked for is not there.
fn root_identity<S: hep_root::ByteSource>(
    source: S,
    shown: &str,
    tree: &str,
    listed: bool,
) -> Result<(String, u64, Option<Branches>), String> {
    let file = hep_root::RootFile::open(source)
        .map_err(|e| format!("{shown} is not a ROOT file this reader can open: {e}"))?;
    let uuid = file.header().uuid_string();
    let found = match listed {
        true => file.tree_at(tree).map(|t| (t.entries, Some(branch_list(&t)))),
        false => file.tree_entries(tree).map(|entries| (entries, None)),
    };
    match found {
        Ok((entries, list)) => Ok((uuid, entries.max(0) as u64, list)),
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

/// What resolving needs: the folder samples.toml is in, and storage access.
pub struct Context {
    pub base: PathBuf,
    pub storage: Option<Storage>,
    /// Why storage is unavailable, said once to the user if a sample needs it.
    pub offline: String,
    /// Each local folder's file names (or why it cannot be read), listed once a run.
    folders: std::sync::Mutex<std::collections::HashMap<PathBuf, Listing>>,
}

/// One folder's listing, made by the first thread to ask for it while the others wait.
type Listing = Arc<OnceLock<Result<Arc<[String]>, String>>>;

/// The folder and name of a path or URL of `files` (`.` for a bare name, `/` for one at the root).
pub fn split(asked: &str) -> (&str, &str) {
    match asked.rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some(split) => split,
        None => (".", asked),
    }
}

/// A file's path or URL from its folder and name, as `split` parts them.
pub fn join(folder: &str, name: &str) -> String {
    match folder {
        "/" => format!("/{name}"),
        _ => format!("{folder}/{name}"),
    }
}

impl Context {
    pub fn new(
        base: PathBuf,
        proxy: Option<String>,
        proxy_said: String,
        ca: Option<String>,
    ) -> Self {
        let storage = Storage::new(proxy.as_deref(), ca.as_deref());
        let offline = match &storage {
            Err(e) => e.clone(),
            Ok(_) => proxy_said,
        };
        Context {
            base,
            storage: storage.ok(),
            offline,
            folders: Default::default(),
        }
    }

    /// The regular files of a local folder, and links to them, sorted by name; listed once a run.
    pub fn listed(&self, dir: &Path) -> Result<Arc<[String]>, String> {
        let slot = self.folders.lock().unwrap_or_else(|e| e.into_inner()).entry(dir.to_path_buf()).or_default().clone();
        slot.get_or_init(|| {
            let mut names = Vec::new();
            // An entry that cannot be read is left out, as one that is not a file is.
            for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
                // A link is followed to see whether it is a file; anything else is told by the folder.
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_file() || (kind.is_symlink() && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_file())) {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            names.sort();
            Ok(names.into())
        })
        .clone()
    }

    /// Which file a path or URL of the lock is, however it is written: a local file as its path
    /// with links followed, a storage file as the door and path it is read at.
    pub fn which(&self, at: &str) -> PathBuf {
        match at.contains("://") {
            true => match super::remote::door(at) {
                Ok((door, path)) => PathBuf::from(format!("{door}{}", rowver::study::plain(Path::new(&path)).display())),
                Err(_) => PathBuf::from(at),
            },
            false => {
                let path = local(at, &self.base);
                path.canonicalize().unwrap_or_else(|_| rowver::study::plain(&path))
            }
        }
    }

    fn storage(&self) -> Result<&Storage, String> {
        self.storage.as_ref().ok_or_else(|| self.offline.clone())
    }
}

/// The files one path, URL or pattern of `files` matches, each pinned; the first with its branch
/// list when `listed`.
fn files_of(ctx: &Context, asked: &str, tree: &str, listed: bool) -> Result<Vec<(File, Option<Branches>)>, String> {
    let (folder, name) = split(asked);
    let join = |n: &str| join(folder, n);
    if pattern(folder) {
        return Err(format!("{asked}: a pattern may only be in the file name, not in a folder. Write one pattern per folder."));
    }
    let wanted = |found: &str| if pattern(name) { matches(name, found) } else { found == name };
    let pinned = |at: String, bytes: u64, adler32: Option<String>, identity: (String, u64, Option<Branches>)| {
        let (uuid, entries, list) = identity;
        (File { at, bytes, adler32, uuid, entries, stop: None, take: None }, list)
    };
    if asked.contains("://") {
        let storage = ctx.storage()?;
        let entries = storage.list(folder)?;
        let found: Vec<_> = entries.iter().filter(|e| wanted(&e.name)).collect();
        if found.is_empty() {
            let near = meant(name, entries.iter().map(|e| e.name.as_str()));
            return Err(format!("{asked} matched no file ({} files in that folder).{near}", entries.len()));
        }
        found
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let at = join(&e.name);
                let (said, adler32) = storage.digest(&at).map_err(|failed| failed.why)?;
                // The listing's size, else the HEAD's: a lock pins every file's size.
                let bytes = match e.bytes {
                    0 => said.filter(|b| *b > 0).ok_or_else(|| format!("{at}: its storage gives no size for it, so the lock cannot pin it"))?,
                    size => size,
                };
                let identity = root_identity(HttpsFile::new(storage, &at, bytes)?, &at, tree, listed && i == 0)?;
                Ok(pinned(at, bytes, adler32, identity))
            })
            .collect()
    } else {
        let dir = local(folder, &ctx.base);
        let names = ctx.listed(&dir).map_err(|e| format!("{asked}: the folder {folder} cannot be read: {e}"))?;
        let found: Vec<&String> = names.iter().filter(|n| wanted(n)).collect();
        if found.is_empty() {
            let near = meant(name, names.iter().map(String::as_str));
            let shown = if folder == "." { "the study folder" } else { folder };
            return Err(format!("{asked} matched no file in {shown}.{near}"));
        }
        found
            .into_iter()
            .enumerate()
            .map(|(i, n)| {
                let path = dir.join(n);
                let meta = std::fs::metadata(&path).map_err(|e| format!("{n}: {e}"))?;
                // The folder as samples.toml writes it, so the lock reads from any folder.
                let at = join(n);
                let opened = hep_root::LocalFile::open(&path).map_err(|e| format!("{at}: {e}"))?;
                let identity = root_identity(opened, &at, tree, listed && i == 0)?;
                let adler32 = adler32_file(&path).map_err(|e| format!("{at}: {e}"))?;
                Ok(pinned(at, meta.len(), Some(adler32), identity))
            })
            .collect()
    }
}

/// One sample resolved: every file it asks for, pinned, with what its own event choice reads of
/// each, its metadata's `{file}` filled; and its branch list, its first file's.
pub fn resolve(ctx: &Context, sample: &Sample) -> Result<(lock::Sample, Branches), String> {
    let mut files: Vec<File> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut list = None;
    for text in &sample.files {
        for (file, found) in files_of(ctx, text, &sample.tree, list.is_none())? {
            if !seen.insert(ctx.which(&file.at)) {
                return Err(format!("{} is reached twice, by two of files or by a pattern and a name: name each file once", file.at));
            }
            list = list.or(found);
            files.push(file);
        }
    }
    let list = list.ok_or("it has no files")?;
    if files.iter().try_fold(0u64, |sum, f| sum.checked_add(f.entries)).is_none_or(|t| t > lock::MOST_ENTRIES) {
        return Err("its files hold more than 2^40 entries in all, more than a .rvr file may: split them among samples of fewer files".into());
    }
    if list.list.iter().any(|b| b.name.is_empty()) {
        return Err(format!("its tree {} has a branch with no name, which samples.lock cannot record: write the files again with every branch named", sample.tree));
    }
    let ats: Vec<&str> = files.iter().map(|f| f.at.as_str()).collect();
    let metadata = rowver::study::samples::file_filled(&sample.metadata, &ats)
        .ok_or_else(|| format!("its metadata uses {{file}}, which names a sample's one file, but it has {} files: give each file a sample of its own, or leave {{file}} out", files.len()))?;
    let locked = lock::Sample {
        name: sample.name.clone(),
        label: sample.label.clone(),
        asked: sample.files.clone(),
        tree: sample.tree.clone(),
        branches: list.sha256.clone(),
        events: sample.events,
        events_total: 0,
        events_read: 0,
        metadata,
        files,
    };
    Ok((locked.planned(sample.events), list))
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
    fn a_path_is_split_and_joined_back_as_written() {
        for asked in ["root://eospublic.cern.ch//x.root", "root://h//eos/a/x.root", "/x.root", "./x.root", "~/d/x.root", "d//x.root"] {
            let (folder, name) = split(asked);
            assert_eq!(join(folder, name), asked);
        }
        assert_eq!(join(split("x.root").0, "x.root"), "./x.root");
    }

    #[test]
    fn two_branch_lists_differ_by_name_kind_and_shape() {
        let branch = |name: &str, kind: &str, shape| Branch { name: name.into(), kind: kind.into(), shape, why: None };
        let want = Branches::new(vec![branch("nJet", "i4", lock::Shape::Flat), branch("Jet_pt", "f4", lock::Shape::Jagged)]);
        let have = Branches::new(vec![branch("nJet", "u4", lock::Shape::Flat), branch("Muon_pt", "f4", lock::Shape::Jagged)]);
        assert_eq!(differ(want.list.iter(), have.list.iter()), "Jet_pt missing, nJet u4 flat here, i4 flat in the sample");
        assert_eq!(differ(want.list.iter(), want.list.iter()), "");
        let unread = |why: &str| Branch { name: "obj".into(), kind: "none".into(), shape: lock::Shape::Flat, why: Some(why.into()) };
        assert_eq!(differ([unread("old words")].iter(), [unread("new words")].iter()), "", "why is not compared");
    }

    #[test]
    fn a_local_path_is_under_home_absolute_or_the_study_s() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(local("~", Path::new("s")), PathBuf::from(&home));
        assert_eq!(local("~/d/x.root", Path::new("s")), PathBuf::from(&home).join("d/x.root"));
        assert_eq!((local("/d/x.root", Path::new("s")), local("d/x.root", Path::new("s"))), (PathBuf::from("/d/x.root"), PathBuf::from("s/d/x.root")));
        assert_eq!((split("/x.root"), split("x.root"), split("~/x.root")), (("/", "x.root"), (".", "x.root"), ("~", "x.root")));
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
}
