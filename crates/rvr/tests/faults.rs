//! The faults of local files and disks, each in a folder of its own: a file deleted, cut short or rewritten, its folder moved away, a file or folder made
//! unreadable, while the press reads another file; a file changed since the lock in each way the
//! lock pins it; an output folder that may not be written; two presses of one target at once; a
//! full disk. Each gives the hole, stop or message SPEC §2.6 and §7 name, and nothing else.

mod common;
#[allow(dead_code)]
#[path = "storage/server.rs"]
mod server;

use common::{rvr_with, Study};
use rowver::study::lock::{Branches, Lock};
use serde_json::Value;
use server::{Answer, Server};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RECIPE: &str = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"nJet\", \"Jet_pt\"]\n\n[totals]\n\n[derived]\n";

/// What every rvr here runs with: the test storage's authority trusted, no grid proxy, and each
/// second of waiting a millisecond.
fn set(dir: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("X509_CERT_DIR", common::fixture("tls/authority").display().to_string()),
        ("X509_USER_PROXY", dir.join("no-proxy").display().to_string()),
        ("RVR_SECOND_MS", "1".into()),
    ]
}

/// rvr in `dir`: its exit status, and what it said.
fn rvr(dir: &Path, args: &[&str]) -> (i32, String) {
    let set = set(dir);
    let set: Vec<(&str, &str)> = set.iter().map(|(name, value)| (*name, value.as_str())).collect();
    rvr_with(dir, &set, args)
}

/// The file a press of the study in `dir` writes.
fn file(dir: &Path) -> String {
    format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())
}

/// The header of the file at `path` (in `dir`).
fn header(dir: &Path, path: &str) -> Value {
    let (code, said) = rvr(dir, &["header", path]);
    assert_eq!(code, 0, "{said}");
    serde_json::from_str(&said).unwrap()
}

/// Every hole a header records: its file, its kind and why.
fn holes(header: &Value) -> Vec<(String, String, String)> {
    let samples = header["made"]["files"].as_object().unwrap().values();
    let files = samples.flat_map(|sample| sample["files"].as_object().unwrap().iter());
    let text = |hole: &Value, key: &str| hole[key].as_str().unwrap().to_owned();
    files.flat_map(|(at, file)| file["holes"].as_array().into_iter().flatten().map(move |hole| (at.clone(), text(hole, "kind"), text(hole, "why")))).collect()
}

/// Whether this runs as root, who reads a file whatever its rights.
fn root() -> bool {
    let uid = Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    uid.ok().as_deref() == Some("0")
}

fn mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A study whose first sample is the ntuple on a test storage, and whose second two copies of it
/// in the folder `b` (`b/flat_a.root`, `b/flat_b.root`), resolved: a press reads the remote one first.
fn halted(name: &str) -> (Study, Server) {
    let server = Server::start();
    server.put("/store/flat.root", std::fs::read(common::fixture("uproot_flat.root")).unwrap());
    let samples = format!(
        "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n[[sample]]\nname = \"far\"\nfiles = {:?}\n\n[[sample]]\nname = \"near\"\nfiles = \"b/flat_*.root\"\n",
        server.url("/store/flat.root")
    );
    let dir = common::study(name, &samples, RECIPE);
    std::fs::create_dir(dir.join("b")).unwrap();
    for copy in ["flat_a.root", "flat_b.root"] {
        std::fs::copy(dir.join("uproot_flat.root"), dir.join("b").join(copy)).unwrap();
    }
    let (code, said) = rvr(&dir, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    (dir, server)
}

/// rvr in `dir` with `args`, the first GET it sends `server` held unanswered while `meanwhile`
/// runs: its exit status, what it said, and what `meanwhile` gave.
fn while_held<T>(dir: &Path, server: &Server, args: &[&str], meanwhile: impl FnOnce() -> T) -> (i32, String, T) {
    let held = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    let shared = held.clone();
    server.script(move |request| {
        if request.method == "GET" && !shared.1.load(Ordering::SeqCst) {
            shared.0.store(true, Ordering::SeqCst);
            let since = Instant::now();
            while !shared.1.load(Ordering::SeqCst) && since.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Answer::Serve
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_rvr"));
    command.args(args).current_dir(dir).env("RVR_CONFIG", dir.join(".rvr-config")).envs(set(dir));
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let since = Instant::now();
    while !held.0.load(Ordering::SeqCst) {
        assert!(since.elapsed() < Duration::from_secs(30), "rvr {args:?} never asked for its first file");
        std::thread::sleep(Duration::from_millis(5));
    }
    let gave = meanwhile();
    held.1.store(true, Ordering::SeqCst);
    let out = child.wait_with_output().unwrap();
    server.script(|_| Answer::Serve);
    let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), said, gave)
}

/// The data checksum of the file at `path`, which a press of the same study at once writes too.
fn sha256(dir: &Path, path: &str) -> Value {
    header(dir, path)["data"]["sha256"].clone()
}

/// The checksum a press of the whole study in `dir` writes, its file put back after.
fn full(dir: &Path) -> Value {
    let path = dir.join(file(dir));
    let kept = std::fs::read(&path).ok();
    let (code, said) = rvr(dir, &["press", "--full"]);
    assert_eq!(code, 0, "{said}");
    let sha = sha256(dir, &file(dir));
    match kept {
        Some(bytes) => std::fs::write(&path, bytes).unwrap(),
        None => std::fs::remove_file(&path).unwrap(),
    }
    sha
}

/// One byte of the file at `path` flipped: its size the same, its adler32 not.
fn flip(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    bytes[30_000] ^= 1;
    std::fs::write(path, bytes).unwrap();
}

// SPEC 2.6, 7.2.3, 7.3.1, 7.3.2, 7.3.3, 7.3.4
#[test]
fn a_file_deleted_cut_short_or_rewritten_while_the_press_reads_another_is_a_hole_of_its_kind() {
    type Fault = fn(&Path);
    let cases: [(&str, Fault, &str, &str); 3] = [
        ("deleted", |path| std::fs::remove_file(path).unwrap(), "gone", "b/flat_b.root no longer exists: put it back and run rvr press --fill"),
        ("cut short", |path| std::fs::File::options().write(true).open(path).unwrap().set_len(20_000).unwrap(), "changed", "b/flat_b.root: it was 39262 bytes, adler32 "),
        ("rewritten", flip, "changed", "; it is now 39262 bytes, adler32 "),
    ];
    for (what, fault, kind, why) in cases {
        let (dir, server) = halted("mid-press");
        let (code, said, ()) = while_held(&dir, &server, &["press", "--workers", "1"], || fault(&dir.join("b/flat_b.root")));
        assert_eq!(code, 3, "{what}: {said}");
        let header = header(&dir, &file(&dir));
        let holes = holes(&header);
        assert_eq!(holes.len(), 1, "{what}: {holes:?}");
        let (at, found, said_why) = &holes[0];
        assert!(at == "b/flat_b.root" && found == kind && said_why.contains(why), "{what}: {holes:?}");
        assert!(said.contains(&format!("    hole ({kind}): ")) && said.contains("with 1 hole:"), "{what}: {said}");
        assert_eq!(header["data"]["rows"], 10, "{what}: the remote file and b/flat_a.root, 5 entries each");
    }
}

// SPEC 2.6, 7.3.3, 7.5.1, 7.5.3, 7.5.4
#[test]
fn a_folder_moved_away_or_a_file_or_folder_made_unreadable_mid_press_stops_it_and_the_next_carries_on() {
    if root() {
        return eprintln!("not checked here: root reads a file whatever its rights");
    }
    type Change = fn(&Path);
    let cases: [(&str, Change, Change, &str, usize); 3] = [
        ("folder moved", |d| std::fs::rename(d.join("b"), d.join("away")).unwrap(), |d| std::fs::rename(d.join("away"), d.join("b")).unwrap(), "b/flat_a.root cannot be read: its folder is not there either (a disk not mounted?)", 1),
        ("file unreadable", |d| mode(&d.join("b/flat_b.root"), 0o000), |d| mode(&d.join("b/flat_b.root"), 0o644), "b/flat_b.root is not yours to read or write", 2),
        ("folder unreadable", |d| mode(&d.join("b"), 0o000), |d| mode(&d.join("b"), 0o755), "b/flat_a.root is not yours to read or write", 1),
    ];
    for (what, fault, mend, why, done) in cases {
        let (dir, server) = halted("stopped");
        let whole = full(&dir);
        let (code, said, ()) = while_held(&dir, &server, &["press", "--workers", "1"], || fault(&dir));
        mend(&dir);
        assert_eq!(code, 1, "{what}: {said}");
        assert!(said.contains(why) && !said.contains("hole ("), "{what}: {said}");
        assert!(said.contains(&format!("It is not a file's fault, so no file was made a hole of it: fix it, then press again, which carries on from the {done} of 3 files done")), "{what}: {said}");
        assert!(!dir.join(file(&dir)).exists(), "{what}");
        let (code, said) = rvr(&dir, &["press", "--workers", "1"]);
        assert!(code == 0 && said.contains(&format!("where it stopped: {done} of 3 files done")), "{what}: {said}");
        assert_eq!(sha256(&dir, &file(&dir)), whole, "{what}: picked up, the file is the one a press at once writes");
    }
}

/// Three bytes of the file at `path`, from `at`, changed by +1, -2 and +1: its size and adler32 the same.
fn same_sum(path: &Path, at: usize) {
    let mut bytes = std::fs::read(path).unwrap();
    let before = server::adler32(&bytes);
    bytes[at] += 1;
    bytes[at + 1] -= 2;
    bytes[at + 2] += 1;
    assert_eq!(server::adler32(&bytes), before);
    std::fs::write(path, bytes).unwrap();
}

/// The study's lock changed by `edit`, as resolve would have written it.
fn relocked(dir: &Path, edit: impl FnOnce(&mut Lock)) {
    let text = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    let mut lock = Lock::read(&text).unwrap_or_else(|p| panic!("{:?}", p.said));
    edit(&mut lock);
    std::fs::write(dir.join("samples.lock"), lock.write().unwrap()).unwrap();
}

// SPEC 7.2.3, 7.3.1, 7.3.2
#[test]
fn a_file_changed_since_the_lock_in_each_way_the_lock_pins_it_is_a_changed_hole() {
    let samples = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nfiles = \"flat_*.root\"\ntree = \"analysis\"\n";
    let cases = [
        ("bytes", "./flat_b.root: it was 39262 bytes, adler32 "),
        ("adler32", "; it is now 39262 bytes, adler32 "),
        ("uuid", "./flat_b.root is not the file samples.lock recorded: its ROOT UUID was 31e851e4-b994-11f1-ba5c-1a45bb13c76a, it is now 32e652e4-b994-11f1-ba5c-1a45bb13c76a."),
        ("entries", "./flat_b.root: its tree analysis has 5 entries, not the 6 samples.lock recorded."),
        ("kind", "of the branches the recipe reads, its tree is not as its sample's first file is (nJet i4 flat here, i8 flat in the sample)."),
    ];
    for (what, why) in cases {
        let dir = common::study("changed-here", samples, RECIPE);
        for copy in ["flat_a.root", "flat_b.root"] {
            std::fs::copy(dir.join("uproot_flat.root"), dir.join(copy)).unwrap();
        }
        assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
        let b = dir.join("flat_b.root");
        match what {
            "bytes" => std::fs::write(&b, [std::fs::read(&b).unwrap(), vec![0]].concat()).unwrap(),
            "adler32" => flip(&b),
            // The UUID's first three bytes (FORMAT §6.3: from byte 47).
            "uuid" => same_sum(&b, 47),
            "entries" => relocked(&dir, |lock| {
                let sample = &mut lock.samples[0];
                sample.files[1].entries += 1;
                sample.events_total += 1;
                sample.events_read += 1;
            }),
            _ => relocked(&dir, |lock| {
                let mut list = lock.branches[0].list.clone();
                list.iter_mut().find(|b| b.name == "nJet").unwrap().kind = "i8".into();
                lock.branches = vec![Branches::new(list)];
                lock.samples[0].branches = lock.branches[0].sha256.clone();
            }),
        }
        let (code, said) = rvr(&dir, &["press"]);
        assert_eq!(code, 3, "{what}: {said}");
        let holes = holes(&header(&dir, &file(&dir)));
        // A branch list is its sample's: every file reads the branch as another kind than it gives.
        let files: &[&str] = if what == "kind" { &["./flat_a.root", "./flat_b.root"] } else { &["./flat_b.root"] };
        let found: Vec<&str> = holes.iter().map(|(at, _, _)| at.as_str()).collect();
        assert_eq!(found, files, "{what}: {holes:?}");
        assert!(holes.iter().all(|(_, kind, said)| kind == "changed" && said.contains(why)), "{what}: {holes:?}");
    }
}

// SPEC 2.6, 7.5.1, 7.5.3, 7.5.4
#[test]
fn an_output_folder_that_may_not_be_written_stops_the_press_before_it_reads_or_as_it_puts_the_file_in() {
    if root() {
        return eprintln!("not checked here: root writes a folder whatever its rights");
    }
    let (dir, server) = halted("shut-out");
    let out = dir.join("out");
    std::fs::create_dir(&out).unwrap();
    mode(&out, 0o555);
    let (code, said) = rvr(&dir, &["press", "--out", "out/x.rvr"]);
    mode(&out, 0o755);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("x.rvr could not be staged in ") && said.contains("x.rvr.partial: Permission denied. Make its folder writable"), "{said}");
    assert!(!said.contains("Pressing") && !out.join("x.rvr.partial").exists(), "nothing is read, and nothing is left: {said}");
    // Shut as the press reads: what it read is kept, and the next press puts the file in.
    let (code, said, ()) = while_held(&dir, &server, &["press", "--workers", "1", "--out", "out/x.rvr"], || mode(&out, 0o555));
    mode(&out, 0o755);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("carries on from the 3 of 3 files done") && !said.contains("hole ("), "{said}");
    assert!(!out.join("x.rvr").exists() && out.join("x.rvr.partial").exists(), "{said}");
    let (code, said) = rvr(&dir, &["press", "--out", "out/x.rvr"]);
    assert!(code == 0 && said.contains("where it stopped: 3 of 3 files done"), "{said}");
    assert_eq!(rvr(&dir, &["validate", "out/x.rvr"]).0, 0);
    assert!(!out.join("x.rvr.partial").exists());
}

const BUSY: &str = "is being changed by another program (it holds the file's lock): wait for it to finish, then try again";

// SPEC 7.5.2, 8.12
#[test]
fn of_two_presses_of_one_target_at_once_the_second_is_refused() {
    let (dir, server) = halted("two-at-once");
    // No file there yet: the second is refused the staging folder the first holds.
    let (code, said, (second, refused)) = while_held(&dir, &server, &["press", "--workers", "1"], || rvr(&dir, &["press"]));
    assert!(second == 1 && refused.contains("another rvr press of ") && refused.contains(" is under way (it holds "), "{refused}");
    assert_eq!(code, 0, "{said}");
    // A file there: the second is refused the file's lock, which the first holds from its first read.
    let (code, said, (second, refused)) = while_held(&dir, &server, &["press", "--full", "--workers", "1"], || rvr(&dir, &["press", "--full"]));
    assert!(second == 1 && refused.contains(BUSY), "{refused}");
    assert_eq!(code, 0, "{said}");
    assert_eq!(rvr(&dir, &["validate", &file(&dir)]).0, 0);
}

/// What a press says when a file was put at its target, where there was none, as it pressed.
const PUT_THERE: &str = " was put there by another program while rvr pressed it, and is left as it is: move it away.";

// SPEC 7.5.3, 7.5.4, 8.12
#[test]
fn a_press_begun_with_no_file_at_its_target_leaves_one_another_program_put_there_meanwhile() {
    let (dir, server) = halted("put-there");
    let target = dir.join(file(&dir));
    let (code, said, ()) = while_held(&dir, &server, &["press", "--workers", "1"], || std::fs::write(&target, b"another program's file").unwrap());
    assert!(std::fs::read(&target).unwrap() == b"another program's file", "{said}");
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(PUT_THERE) && said.contains("press again, which carries on from the 3 of 3 files done"), "{said}");
    std::fs::remove_file(&target).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("where it stopped: 3 of 3 files done"), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file(&dir)]).0, 0);
}

// SPEC 7.5.2, 8.12
#[test]
fn two_presses_started_together_write_one_file_and_any_other_is_refused_or_finds_it_current() {
    let names: Vec<String> = (0..12).map(|i| format!("\"r{i:02}.root\"")).collect();
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"runs\"\nfiles = [{}]\ntree = \"Events\"\n", names.join(", "));
    let recipe = RECIPE.replace("\"nJet\", \"Jet_pt\"", "\"Jet_pt\", \"Jet_eta\", \"MET\", \"event\"");
    let dir = common::study("together", &samples, &recipe);
    for i in 0..12 {
        std::fs::copy(common::fixture("runs.root"), dir.join(format!("r{i:02}.root"))).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let whole = full(&dir);
    for round in 0..8 {
        let start = || {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rvr"));
            command.args(["press", "--workers", "2"]).current_dir(&*dir).env("RVR_CONFIG", dir.join(".rvr-config"));
            command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
        };
        let both = [start(), start()].map(|child| {
            let out = child.wait_with_output().unwrap();
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
        });
        let wrote = both.iter().filter(|(code, said)| *code == Some(0) && said.contains("Wrote ")).count();
        let other = |(code, said): &(Option<i32>, String)| match code {
            Some(0) => said.contains("Wrote ") || said.contains("is current: nothing to press"),
            // Begun before the first put its file in place, and staged after: it keeps what it read.
            Some(1) => said.contains(BUSY) || said.contains(" is under way (it holds ") || said.contains(PUT_THERE),
            _ => false,
        };
        assert!(wrote == 1 && both.iter().all(other), "round {round}: {both:?}");
        assert_eq!(rvr(&dir, &["validate", &file(&dir)]).0, 0);
        assert_eq!(sha256(&dir, &file(&dir)), whole, "round {round}");
        std::fs::remove_file(dir.join(file(&dir))).unwrap();
        let staged = dir.join(format!("{}.partial", file(&dir)));
        let kept = both.iter().any(|(_, said)| said.contains(PUT_THERE));
        assert_eq!(staged.exists(), kept, "round {round}: {both:?}");
        if kept {
            std::fs::remove_dir_all(staged).unwrap();
        }
    }
}

/// A RAM disk whose volume is mounted while it lives; detached when dropped.
#[cfg(target_os = "macos")]
struct RamDisk {
    device: String,
    volume: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
impl RamDisk {
    /// A disk of `mb` MB, its volume named for this process.
    fn new(mb: u64) -> RamDisk {
        let out = Command::new("hdiutil").args(["attach", "-nomount", &format!("ram://{}", mb * 2048)]).output().unwrap();
        let device = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        assert!(out.status.success() && device.starts_with("/dev/disk"), "hdiutil: {}", String::from_utf8_lossy(&out.stderr));
        let name = format!("rvrfull{}", std::process::id());
        let disk = RamDisk { device, volume: Path::new("/Volumes").join(&name) };
        let out = Command::new("diskutil").args(["erasevolume", "HFS+", &name, &disk.device]).output().unwrap();
        assert!(out.status.success() && disk.volume.is_dir(), "diskutil: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        disk
    }

    /// Fill the disk with one file until `left` bytes of it are free.
    fn fill(&self, left: u64) {
        use std::io::Write;
        let mut filler = std::fs::File::create(self.volume.join("filler")).unwrap();
        let (block, mut written) = (vec![0u8; 1 << 16], 0);
        while let Ok(n @ 1..) = filler.write(&block) {
            written += n as u64;
        }
        filler.set_len(written.saturating_sub(left)).unwrap();
        let _ = filler.sync_all();
    }
}

#[cfg(target_os = "macos")]
impl Drop for RamDisk {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil").args(["detach", "-force", &self.device]).output();
    }
}

// SPEC 2.6, 7.5.3, 7.5.4, 7.6.1
#[test]
#[cfg(target_os = "macos")]
#[ignore = "makes a 16 MB RAM disk with hdiutil, and fills it"]
fn a_full_disk_stops_the_press_keeping_what_it_read_and_the_next_carries_on() {
    let names: Vec<String> = (0..24).map(|i| format!("\"r{i:02}.root\"")).collect();
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"runs\"\nfiles = [{}]\ntree = \"Events\"\n", names.join(", "));
    let recipe = RECIPE.replace("\"nJet\", \"Jet_pt\"", "\"Jet_pt\", \"Jet_eta\", \"Jet_phi\", \"MET\", \"weight\", \"run\", \"event\"");
    let dir = common::study("full-disk", &samples, &recipe);
    for i in 0..24 {
        std::fs::copy(common::fixture("runs.root"), dir.join(format!("r{i:02}.root"))).unwrap();
    }
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let whole = full(&dir);
    let disk = RamDisk::new(16);
    let out = disk.volume.join("out.rvr");
    let target = out.to_str().unwrap();
    let mut stopped = 0;
    for left in [0, 64 << 10, 256 << 10, 512 << 10, 1 << 20, 2 << 20, 3 << 20, 4 << 20, 6 << 20] {
        disk.fill(left);
        let (code, said) = rvr(&dir, &["press", "--workers", "1", "--out", target]);
        if code != 0 {
            stopped += 1;
            assert_eq!(code, 1, "{left} bytes free: {said}");
            assert!(said.contains("disk is full") && !said.contains("No space left on device"), "{left} bytes free: {said}");
            assert!(!said.contains("hole (") && !out.exists(), "{left} bytes free: {said}");
        }
        std::fs::remove_file(disk.volume.join("filler")).unwrap();
        let (code, said) = rvr(&dir, &["press", "--workers", "1", "--out", target]);
        assert_eq!(code, 0, "{left} bytes free, then the disk emptied: {said}");
        assert_eq!(sha256(&dir, target), whole, "{left} bytes free: {said}");
        std::fs::remove_file(&out).unwrap();
        assert!(!disk.volume.join("out.rvr.partial").exists(), "{left} bytes free");
    }
    assert!(stopped >= 3, "the disk filled only {stopped} times");
}
