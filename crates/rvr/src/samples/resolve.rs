//! `rvr samples resolve`, one sample at a time: every file it asks for, found and pinned (its size,
//! its adler32 when its storage gives one, its ROOT UUID and its tree's entries), and the sample's
//! branch list, from its first file; no event read. The lock itself is rowver's
//! (`rowver::study::lock`).

use super::remote::{HttpsFile, Storage, STOPPED};
use hep_root::columns::{inventory, Kind, Shape};
use rowver::study::lock::{self, Branch, Branches, File};
use rvr_recipe::meant;
pub use rowver::study::{join, matches, split};
use rowver::study::samples::Sample;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Where a local path of samples.toml or the lock is: under `~/`, absolute, or relative to `base`.
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

/// How a file differs from what the lock recorded, in words: its size, and its adler32 when both have one.
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

/// The branches of a tree `wanted` takes, in order and once each: name, kind (or `none`, with why) and shape.
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

/// How `have` falls short of `want` by name, kind and shape: the first few it lacks or has otherwise; empty if none.
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
    let file = hep_root::RootFile::open(source).map_err(|e| match e {
        // Shorter than its header says: cut short, as a copy that did not finish leaves a file.
        hep_root::Error::SourceChanged(why) if why.starts_with("header says the file ends at") => {
            format!("{shown} is cut short ({why}), as a copy that did not finish leaves a file: put a whole copy in its place, then run resolve again")
        }
        // Its storage failing it is not the file's fault, as open.rs says too; a file shorter than
        // its keys say, or holding what cannot be so, is (a remote one's storage fails as Remote).
        hep_root::Error::Io(e) if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::InvalidData) => format!("{shown} is not a ROOT file this reader can open: {e}"),
        e @ (hep_root::Error::Remote(_) | hep_root::Error::SourceChanged(_) | hep_root::Error::Io(_)) => format!("{shown} could not be read: {e}"),
        e => format!("{shown} is not a ROOT file this reader can open: {e}"),
    })?;
    let uuid = file.header().uuid_string();
    let found = match listed {
        true => file.tree_at(tree).map(|t| (t.entries, Some(branch_list(&t)))),
        false => file.tree_entries(tree).map(|entries| (entries, None)),
    };
    match found {
        Ok((entries, list)) => Ok((uuid, entries.max(0) as u64, list)),
        Err(hep_root::Error::KeyNotFound(_)) => {
            let listing = match file.trees_listing() {
                Ok(listing) => listing,
                Err(e) => return Err(format!("{shown} has no tree {tree:?}, and its trees could not be listed ({e}): check that it is a whole ROOT file (a copy that did not finish leaves one cut short), then resolve again")),
            };
            if listing.rntuples.iter().any(|path| path == tree.trim_matches('/')) {
                return Err(format!("{shown} holds {tree:?} as an RNTuple, which rvr cannot read (it reads TTrees): write its events as a TTree"));
            }
            let trees: Vec<String> = listing.trees.into_iter().map(|t| t.path).collect();
            let listed = if trees.is_empty() { "none".into() } else { trees.join(", ") };
            // The listing goes LISTING_DEPTH folders deep and opens at most LISTING_DIRECTORIES (SPEC §6.2.7): what it left is said.
            let (depth, most) = (hep_root::file::LISTING_DEPTH, hep_root::file::LISTING_DIRECTORIES);
            let left = match listing.unopened {
                0 => String::new(),
                n => {
                    let was = if n == 1 { "was" } else { "were" };
                    format!(" ({}, past the {depth} folders deep or {most} folders the listing opens, {was} not looked in)", super::some(n, "folder", "folders"))
                }
            };
            Err(format!("{shown} has no tree {tree:?}. Its trees: {listed}{left}."))
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
    folders: super::Once<PathBuf, Result<Arc<[String]>, String>>,
    /// Set when the run is to end: no further file is asked.
    stop: std::sync::atomic::AtomicBool,
    /// Whether each file is pinned (its adler32, or its storage's digest); not when files are only
    /// found and opened, as `rvr branches` before a lock does.
    pub pins: bool,
}


impl Context {
    pub fn new(
        base: PathBuf,
        proxy: Option<String>,
        ca: Option<String>,
    ) -> Self {
        let storage = Storage::new(proxy.as_deref(), ca.as_deref());
        let offline = storage.as_ref().err().cloned().unwrap_or_default();
        Context {
            base,
            storage: storage.ok(),
            offline,
            folders: Default::default(),
            stop: Default::default(),
            pins: true,
        }
    }

    /// End the run: no further file is asked, and a storage stops waiting.
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(storage) = &self.storage {
            storage.watch.stop.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Whether the run goes on, or the refusal that ends it.
    fn going(&self) -> Result<(), String> {
        match self.stop.load(std::sync::atomic::Ordering::Acquire) {
            true => Err(STOPPED.into()),
            false => Ok(()),
        }
    }

    /// The regular files of a local folder, and links to them, sorted by name; listed once a run.
    pub fn listed(&self, dir: &Path) -> Result<Arc<[String]>, String> {
        self.folders.get(dir.to_path_buf(), || {
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
    }

    /// Which file a path or URL of the lock is: a local path with links followed, or a storage file's door and path.
    pub fn which(&self, at: &str) -> PathBuf {
        match at.contains("://") {
            true => match super::remote::door(at) {
                Ok((door, path)) => PathBuf::from(format!("{door}{}", rowver::study::lexical(Path::new(&path)).display())),
                Err(_) => PathBuf::from(at),
            },
            false => {
                let path = local(at, &self.base);
                path.canonicalize().unwrap_or_else(|_| rowver::study::lexical(&path))
            }
        }
    }

    fn storage(&self) -> Result<&Storage, String> {
        self.storage.as_ref().ok_or_else(|| self.offline.clone())
    }
}

/// Each file one path, URL or pattern of `files` matches, pinned and given to `each` in turn; the
/// first with its branch list when `listed`. A run stopped ends before the next file.
fn files_of(ctx: &Context, asked: &str, tree: &str, listed: bool, each: &mut dyn FnMut(File, Option<Branches>) -> Result<(), String>) -> Result<(), String> {
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
            // A storage may list a name percent-encoded, as in a URL, and a lock pins it as listed:
            // one that would match with its escapes decoded, either side, is told how to be written.
            let plain = |n: &str| crate::samples::remote::decoded(n).unwrap_or_else(|| n.to_owned());
            // A pattern is decoded too, unless that makes a wildcard of a %2A or %3F.
            let wild = |n: &str| n.matches(['*', '?']).count();
            let decoded = Some(plain(name)).filter(|n| wild(n) == wild(name));
            let alike = |listed: &str| match pattern(name) {
                true => matches(name, &plain(listed)) || decoded.as_ref().is_some_and(|n| matches(n, &plain(listed))),
                false => plain(name) == plain(listed),
            };
            if let Some(e) = entries.iter().find(|e| alike(&e.name)) {
                return Err(match pattern(name) {
                    true => format!("{asked} matched no file: a name is matched as its storage spells it, with or without %-escapes as in a URL, and it spells one {}: write the pattern so", e.name),
                    false => format!("{asked} matched no file: a name is matched as its storage spells it, with or without %-escapes as in a URL, and it spells this one {}: write it so", e.name),
                });
            }
            let near = meant(name, entries.iter().map(|e| e.name.as_str()));
            return Err(format!("{asked} matched no file ({} files in that folder).{near}", entries.len()));
        }
        for (i, e) in found.iter().enumerate() {
                ctx.going()?;
                let at = join(&e.name);
                // The listing's size is enough to open a file only found; one without needs the HEAD.
                let (said, adler32) = match ctx.pins || e.bytes == 0 {
                    true => storage.digest(&at).map_err(|failed| failed.why)?,
                    false => (None, None),
                };
                // The HEAD's size when it was asked, which a press and rvr samples check compare, else
                // the listing's: a lock pins every file's size.
                let bytes = match (said, e.bytes) {
                    (Some(0), 0) => return Err(format!("{at} is empty (0 bytes), so it is no ROOT file: remove it, or name the files so that this one is not among them")),
                    (Some(0), listed) => return Err(format!("{at}: its storage answers that it holds 0 bytes, but lists it with {listed}, so the lock cannot pin its size: try again later, or ask the storage's admins")),
                    (Some(size), _) | (None, size @ 1..) => size,
                    (None, 0) => return Err(format!("{at}: its storage gives no size for it, so the lock cannot pin it")),
                };
                let identity = root_identity(HttpsFile::new(storage, &at, bytes)?, &at, tree, listed && i == 0)?;
                let (file, list) = pinned(at, bytes, adler32, identity);
                each(file, list)?;
        }
        Ok(())
    } else {
        let dir = local(folder, &ctx.base);
        let names = ctx.listed(&dir).map_err(|e| format!("{asked}: the folder {folder} cannot be read: {e}"))?;
        let found: Vec<&String> = names.iter().filter(|n| wanted(n)).collect();
        if found.is_empty() {
            let near = meant(name, names.iter().map(String::as_str));
            let shown = if folder == "." { "the study folder" } else { folder };
            return Err(format!("{asked} matched no file in {shown}.{near}"));
        }
        for (i, n) in found.into_iter().enumerate() {
                ctx.going()?;
                let path = dir.join(n);
                let meta = std::fs::metadata(&path).map_err(|e| format!("{n}: {e}"))?;
                // The folder as samples.toml writes it, so the lock reads from any folder.
                let at = join(n);
                let opened = hep_root::LocalFile::open(&path).map_err(|e| format!("{at}: {e}"))?;
                let identity = root_identity(opened, &at, tree, listed && i == 0)?;
                let adler32 = match ctx.pins {
                    true => Some(adler32_file(&path).map_err(|e| format!("{at}: {e}"))?),
                    false => None,
                };
                let (file, list) = pinned(at, meta.len(), adler32, identity);
                each(file, list)?;
        }
        Ok(())
    }
}

/// A sample as the lock records it, of these files and their sample's branch list, its choice
/// not yet worked out over them (`planned` does that).
fn locked(sample: &Sample, files: Vec<File>, list: &Branches) -> Result<lock::Sample, String> {
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
    Ok(locked)
}

/// Every file of `sample`, pinned with what its choice reads, and its branch list; `first` is given
/// the sample as its first file alone makes it (its choice not worked out), as soon as that file's
/// branch list is read. A run stopped (`ctx.stop`)
/// ends before the next file.
pub fn resolve(ctx: &Context, sample: &Sample, first: &dyn Fn(lock::Sample, &Branches)) -> Result<(lock::Sample, Branches), String> {
    let mut files: Vec<File> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut list: Option<Branches> = None;
    for text in &sample.files {
        files_of(ctx, text, &sample.tree, list.is_none(), &mut |file, found| {
            if !seen.insert(ctx.which(&file.at)) {
                return Err(format!("{} is reached twice, by two of files or by a pattern and a name: name each file once", file.at));
            }
            if let Some(found) = found.filter(|_| list.is_none()) {
                first(locked(sample, vec![file.clone()], &found)?, &found);
                list = Some(found);
            }
            files.push(file);
            Ok(())
        })?;
    }
    let list = list.ok_or("it has no files")?;
    lock::entries_in_all(&files.iter().map(|f| f.entries).collect::<Vec<_>>(), "its")?;
    if list.list.iter().any(|b| b.name.is_empty()) {
        return Err(format!("its tree {} has a branch with no name, which samples.lock cannot record: write the files again with every branch named", sample.tree));
    }
    Ok((locked(sample, files, &list)?.planned(sample.events), list))
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC 7.2.3
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

    // SPEC 3.8.1
    #[test]
    fn a_local_path_is_under_home_absolute_or_the_study_s() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(local("~", Path::new("s")), PathBuf::from(&home));
        assert_eq!(local("~/d/x.root", Path::new("s")), PathBuf::from(&home).join("d/x.root"));
        assert_eq!((local("/d/x.root", Path::new("s")), local("d/x.root", Path::new("s"))), (PathBuf::from("/d/x.root"), PathBuf::from("s/d/x.root")));
        assert_eq!((split("/x.root"), split("x.root"), split("~/x.root")), (("/", "x.root"), (".", "x.root"), ("~", "x.root")));
    }

    // SPEC 6.2.2, 6.2.3
    #[test]
    fn adler32_of_known_text() {
        let path = std::env::temp_dir().join(format!("rvr-samples-adler-{}", std::process::id()));
        std::fs::write(&path, b"Wikipedia").unwrap();
        assert_eq!(adler32_file(&path).unwrap(), "11e60398");
        std::fs::remove_file(path).unwrap();
    }

    // SPEC 6.2.2
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
