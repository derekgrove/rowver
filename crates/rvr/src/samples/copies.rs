//! Read a file of a catalog dataset from whichever of its copies answers. Rucio is asked once,
//! as a press starts, where every dataset file can be read; `rucio-client` keeps only copies at
//! CMS storage doors. Rucio's order is not a ranking by speed (CMS's runs by host name), so speed
//! is measured: a file's first question (a HEAD for its size and adler32) goes at once to the
//! storage that answered fastest so far and to up to two never measured, and the first to answer
//! with the lock's size (and adler32, when the storage gives one) is read; every answer is
//! timed, so each storage of a dataset gets measured as its files open. A storage that failed
//! twice since it last served a whole file is tried last. A read that fails is tried at the other
//! copies, since every copy holds the same bytes: the copies it failed at are left once another
//! serves it, and a read that every copy fails is the read's own.

use super::remote::{digest_once, HttpsFile, Storage};
use super::resolve::{self, File};
use grid_catalog::Did;
use hep_root::ByteSource;
use rucio_client::Location;
use std::collections::{BTreeMap, HashMap};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// How many copies a file's first question goes to at once.
const RACED: usize = 3;
/// The most files a refusal names before it counts the rest.
const NAMED: usize = 3;

/// What a press has learnt of the storages it reads from, by door (`host:port`).
#[derive(Default)]
struct Learnt {
    /// How long each door last took to answer a file's first question, in milliseconds.
    timed: HashMap<String, u128>,
    /// How many questions running each door has failed; a door that serves again is forgiven.
    failed: HashMap<String, u32>,
    /// Each copy passed over, as `RSE (door)`: how often, and the last reason.
    passed: BTreeMap<String, (usize, String)>,
    /// The files and bytes each storage element served.
    served: BTreeMap<String, (usize, u64)>,
}

impl Learnt {
    /// The copies in the order to try them: fastest measured first, then the unmeasured in
    /// Rucio's order, then those that failed twice running.
    fn order(&self, copies: &[Location]) -> Vec<Location> {
        let mut order = copies.to_vec();
        order.sort_by_key(|copy| {
            (
                self.failing(copy.door.as_str()),
                self.timed
                    .get(copy.door.as_str())
                    .copied()
                    .unwrap_or(u128::MAX),
            )
        });
        order
    }

    /// Whether a door failed twice running.
    fn failing(&self, door: &str) -> bool {
        self.failed.get(door).is_some_and(|n| *n >= 2)
    }

    /// The copies a file's first question goes to: the fastest measured and up to two never
    /// measured, by their place in `order`.
    fn first(&self, order: &[Location]) -> Vec<usize> {
        let measured = |i: &usize| self.timed.contains_key(order[*i].door.as_str());
        let fastest = (0..order.len())
            .filter(|i| measured(i) && !self.failing(order[*i].door.as_str()))
            .take(1);
        let fresh =
            (0..order.len()).filter(|i| !measured(i) && !self.failing(order[*i].door.as_str()));
        let mut first: Vec<usize> = fastest.collect();
        first.extend(fresh.take(RACED - first.len()));
        if first.is_empty() {
            first.push(0);
        }
        first
    }
}

fn held(learnt: &Mutex<Learnt>) -> MutexGuard<'_, Learnt> {
    learnt.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a press's dataset files can be read, and what it has learnt of their storages.
pub struct Datasets {
    copies: HashMap<String, Vec<Location>>,
    learnt: Arc<Mutex<Learnt>>,
}

impl Datasets {
    /// Ask Rucio once where each file (with the dataset it is of) can be read, and check what it
    /// says against the lock: the same size and adler32, and at least one copy on disk.
    pub fn ask(rucio: &rucio_client::Rucio, files: &[(&File, &str)]) -> Result<Self, String> {
        let mut dids: Vec<Did> = files
            .iter()
            .filter_map(|(file, _)| file.lfn.clone())
            .map(Did::cms)
            .collect();
        dids.sort_by(|a, b| a.name.cmp(&b.name));
        dids.dedup();
        let found = rucio.copies(&dids).map_err(|e| {
            format!(
                "Rucio could not say where the dataset files are: {}",
                e.message
            )
        })?;
        let (mut copies, mut problems) = (HashMap::new(), Vec::new());
        for (file, dataset) in files {
            let lfn = file.lfn.as_deref().unwrap_or_default();
            let Some(said) = found.get(lfn) else {
                problems.push(format!("{lfn}: Rucio listed no copy of it. If the dataset has changed, run rvr samples resolve samples.toml and review the new lock."));
                continue;
            };
            if let Some(how) = resolve::differs(file, said.bytes, said.adler32.as_deref()) {
                problems.push(format!("{how}, by Rucio's record. Run rvr samples resolve samples.toml and review the new lock."));
            } else if said.copies.is_empty() {
                problems.push(format!(
                    "{lfn} has no copy that can be read now ({}). It needs a copy on disk: ask for \
                     a Rucio rule that puts {dataset} on a disk storage, for example rucio \
                     add-rule cms:{dataset} 1 T2_US_Nebraska.",
                    said.left_out.join("; ")
                ));
            } else {
                copies.insert(lfn.to_owned(), said.copies.clone());
            }
        }
        if problems.is_empty() {
            return Ok(Datasets {
                copies,
                learnt: Default::default(),
            });
        }
        let more = problems.len().saturating_sub(NAMED);
        let mut said = problems
            .into_iter()
            .take(NAMED)
            .collect::<Vec<_>>()
            .join("\n");
        if more > 0 {
            said += &format!("\n... and {more} more files like these");
        }
        Err(said)
    }

    /// Open a dataset file at the first copy that answers with the lock's size and adler32; a
    /// failure says whether every copy failed as a busy storage does, so asking again may pass.
    pub fn open(&self, storage: &Storage, file: &File) -> Result<Reader, (String, bool)> {
        let lfn = file.lfn.as_deref().unwrap_or_default();
        let listed = self
            .copies
            .get(lfn)
            .ok_or_else(|| (format!("{}: Rucio was not asked where it is", file.at), false))?;
        let (order, first) = {
            let learnt = held(&self.learnt);
            let order = learnt.order(listed);
            let first = learnt.first(&order);
            (order, first)
        };
        let mut readers: Vec<Box<dyn ByteSource + Send + Sync>> = Vec::new();
        for copy in &order {
            let reader = HttpsFile::new(storage, &copy.url, file.bytes).map_err(|why| (why, false))?;
            readers.push(Box::new(reader));
        }
        let agent = storage.agent();
        let ask: Ask = Arc::new(move |url: &str| digest_once(&agent, url));
        Copies::open(file, order, readers, ask, self.learnt.clone(), first)
    }
}

impl Datasets {
    /// What the press read from each storage, and the copies it passed over, as lines to show.
    pub fn summary(&self) -> Vec<String> {
        let learnt = held(&self.learnt);
        let mut lines: Vec<String> = learnt
            .served
            .iter()
            .map(|(rse, (files, bytes))| {
                let s = if *files == 1 { "" } else { "s" };
                format!(
                    "read from {rse}: {files} file{s}, {:.1} MB",
                    *bytes as f64 / 1e6
                )
            })
            .collect();
        lines.extend(learnt.passed.iter().map(|(named, (times, why))| {
            format!(
                "passed over {named} {times} time{}: {why}",
                if *times == 1 { "" } else { "s" }
            )
        }));
        if learnt
            .passed
            .values()
            .any(|(_, why)| why.contains("UnknownIssuer"))
        {
            lines.push(
                "a storage whose authority rvr does not know was passed over; set X509_CERT_DIR \
                 to a folder of grid authorities (IGTF) to read from it too"
                    .into(),
            );
        }
        lines
    }
}

/// Ask a copy's storage for the file's size and adler32 at an address; a failure says why, and
/// whether it may pass when asked again.
type Ask = Arc<dyn Fn(&str) -> Result<(Option<u64>, Option<String>), (String, bool)> + Send + Sync>;

/// One dataset file and the copies it can be read from.
struct Copies {
    file: File,
    ask: Ask,
    order: Vec<Location>,
    /// A reader of each copy, in `order`.
    readers: Vec<Box<dyn ByteSource + Send + Sync>>,
    /// The bytes read from each copy, in `order`.
    bytes: Vec<std::sync::atomic::AtomicU64>,
    /// Whether what was read has been counted towards each storage.
    finished: std::sync::atomic::AtomicBool,
    /// The copy being read.
    current: Mutex<usize>,
    /// Each copy left, by its place in `order`, with why and whether it may pass when asked
    /// again; shared with the questions in flight.
    gone: Arc<Mutex<Vec<(usize, String, bool)>>>,
    learnt: Arc<Mutex<Learnt>>,
}

impl Copies {
    /// The copies of a file, opened at the first that answers of those `first` names.
    fn open(
        file: &File,
        order: Vec<Location>,
        readers: Vec<Box<dyn ByteSource + Send + Sync>>,
        ask: Ask,
        learnt: Arc<Mutex<Learnt>>,
        first: Vec<usize>,
    ) -> Result<Reader, (String, bool)> {
        let bytes = readers.iter().map(|_| Default::default()).collect();
        let copies = Copies {
            file: file.clone(),
            ask,
            order,
            readers,
            bytes,
            finished: Default::default(),
            current: Mutex::new(usize::MAX),
            gone: Default::default(),
            learnt,
        };
        let mut current = copies.current();
        copies.choose(&mut current, first)?;
        drop(current);
        Ok(Reader(Arc::new(copies)))
    }

    fn current(&self) -> MutexGuard<'_, usize> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn gone(&self) -> MutexGuard<'_, Vec<(usize, String, bool)>> {
        self.gone.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a copy holds the file the lock names, by its storage's size and adler32 (the
    /// adler32 only when the storage gives one); why not, when it does not.
    fn holds(ask: &Ask, file: &File, copy: &Location) -> Result<(), (String, bool)> {
        let (bytes, adler32) = ask(&copy.url)?;
        if bytes.is_none() {
            return Err(("its storage did not say how big the file is".into(), false));
        }
        match resolve::differs(file, bytes, adler32.as_deref()) {
            Some(how) => Err((format!("it is not the file the lock names ({how})"), false)),
            None => Ok(()),
        }
    }

    /// Leave a copy for good, counting the failure against its storage.
    fn leave(&self, index: usize, why: &str, busy: bool) {
        let said = failed(&self.learnt, &self.order[index], why);
        self.gone().push((index, said, busy));
    }

    /// Choose the copy to read: ask the copies `first` names at once, and take the first to
    /// answer with the lock's size and adler32; then the others not yet left, one at a time.
    fn choose(&self, current: &mut usize, first: Vec<usize>) -> Result<(), (String, bool)> {
        let mut asked = first;
        loop {
            let gone: Vec<usize> = self.gone().iter().map(|(index, _, _)| *index).collect();
            asked.retain(|i| !gone.contains(i));
            if asked.is_empty() {
                asked.extend((0..self.order.len()).find(|i| !gone.contains(i)));
            }
            if asked.is_empty() {
                *current = self.order.len();
                let gone = self.gone();
                let why: Vec<&str> = gone.iter().map(|(_, why, _)| why.as_str()).collect();
                let said = format!("{}: no copy of it could be read: {}", self.file.at, why.join("; "));
                return Err((said, gone.iter().all(|(_, _, busy)| *busy)));
            }
            let (send, answers) = mpsc::channel();
            // Each answer is timed, or counted and left, as it comes, even after another copy
            // was chosen.
            for &index in &asked {
                let (ask, file, send) = (self.ask.clone(), self.file.clone(), send.clone());
                let (copy, learnt) = (self.order[index].clone(), self.learnt.clone());
                let gone = self.gone.clone();
                std::thread::spawn(move || {
                    let started = Instant::now();
                    let answer = Copies::holds(&ask, &file, &copy);
                    match &answer {
                        Ok(()) => {
                            let took = started.elapsed().as_millis();
                            held(&learnt).timed.insert(copy.door.clone(), took);
                        }
                        Err((why, busy)) => {
                            let said = failed(&learnt, &copy, why);
                            gone.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push((index, said, *busy));
                        }
                    }
                    let _ = send.send((index, answer.is_ok()));
                });
            }
            drop(send);
            if let Some((index, _)) = answers.iter().find(|(_, held)| *held) {
                *current = index;
                return Ok(());
            }
            asked = Vec::new();
        }
    }

    /// Do a read at the current copy; `size` says how many bytes a read gave. When it fails,
    /// the same read is tried at each other copy still holding the file: the copies it failed at
    /// are left once one serves it, and when every copy fails it the failure is the read's own,
    /// no copy is left, and it is the error.
    fn read<T>(
        &self,
        read: impl Fn(&dyn ByteSource) -> hep_root::Result<T>,
        size: impl Fn(&T) -> usize,
    ) -> hep_root::Result<T> {
        let served = |index: usize, value: T| {
            let bytes = size(&value) as u64;
            self.bytes[index].fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
            value
        };
        let index = *self.current();
        let Some(reader) = self.readers.get(index) else {
            let said = format!("{}: no copy of it is left to read", self.file.at);
            return Err(hep_root::Error::Remote(said));
        };
        let error = match read(reader.as_ref()) {
            Ok(value) => return Ok(served(index, value)),
            Err(error) => error,
        };
        let mut current = self.current();
        let mut failed = vec![(index, error.to_string())];
        let gone: Vec<usize> = self.gone().iter().map(|(at, _, _)| *at).collect();
        for other in (0..self.order.len()).filter(|at| *at != index && !gone.contains(at)) {
            if let Err((why, busy)) = Copies::holds(&self.ask, &self.file, &self.order[other]) {
                self.leave(other, &why, busy);
                continue;
            }
            match read(self.readers[other].as_ref()) {
                Ok(value) => {
                    for (at, why) in &failed {
                        self.leave(*at, why, true);
                    }
                    *current = other;
                    return Ok(served(other, value));
                }
                Err(error) => failed.push((other, error.to_string())),
            }
        }
        Err(error)
    }
}

/// A dataset file being read; it says which storage served it.
pub struct Reader(Arc<Copies>);

impl Reader {
    /// A handle that outlives the reader inside a ROOT file, to ask which copy served it.
    pub fn handle(&self) -> Reader {
        Reader(self.0.clone())
    }

    /// The storage element and door of the copy read last.
    pub fn served(&self) -> Option<(String, String)> {
        let copy = self.0.order.get(*self.0.current())?;
        Some((copy.rse.clone(), copy.door.as_str().to_owned()))
    }

    /// Count the file towards the storage that served it last, and each copy's bytes towards its
    /// own storage; once, when the file has been read.
    pub fn finish(&self) {
        if self
            .0
            .finished
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        // `current` before `learnt`, the order a read takes them in.
        let served = self.served();
        let mut learnt = held(&self.0.learnt);
        for (copy, bytes) in self.0.order.iter().zip(&self.0.bytes) {
            let bytes = bytes.load(std::sync::atomic::Ordering::Relaxed);
            if bytes > 0 {
                learnt.served.entry(copy.rse.clone()).or_default().1 += bytes;
            }
        }
        if let Some((rse, door)) = served {
            learnt.served.entry(rse).or_default().0 += 1;
            // A storage that served a whole file is forgiven its failures.
            learnt.failed.remove(&door);
        }
    }
}

impl ByteSource for Reader {
    fn len(&self) -> u64 {
        self.0.file.bytes
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
        let read = self.0.read(
            |copy| {
                let mut bytes = vec![0; buf.len()];
                copy.read_at(offset, &mut bytes).map(|()| bytes)
            },
            Vec::len,
        )?;
        buf.copy_from_slice(&read);
        Ok(())
    }

    fn describe(&self) -> String {
        self.0.file.at.clone()
    }

    fn read_ranges(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        self.0.read(
            |copy| copy.read_ranges(ranges),
            |read| read.iter().map(Vec::len).sum(),
        )
    }
}

/// Count a failure against a copy's storage and keep why; what to say of it, the copy's address
/// left out, since its name says where it is.
fn failed(learnt: &Mutex<Learnt>, copy: &Location, why: &str) -> String {
    let named = format!("{} ({})", copy.rse, copy.door.as_str());
    let why = why
        .replace(&format!("{}: ", copy.url), "")
        .replace(&format!("{} ", copy.url), "");
    let mut learnt = held(learnt);
    *learnt
        .failed
        .entry(copy.door.as_str().to_owned())
        .or_default() += 1;
    let passed = learnt.passed.entry(named.clone()).or_default();
    *passed = (passed.0 + 1, why.clone());
    format!("{named}: {why}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(rse: &str, host: &str) -> Location {
        Location {
            url: format!("https://{host}/store/f.root"),
            door: host.into(),
            rse: rse.into(),
            priority: 0,
        }
    }

    /// A copy held in memory that fails every read after its first `good` ones, its storage
    /// then down; with `good` below zero every read fails and the storage stays up.
    struct Held {
        bytes: Vec<u8>,
        good: std::sync::atomic::AtomicIsize,
        stays: bool,
        down: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ByteSource for Held {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
            use std::sync::atomic::Ordering::Relaxed;
            if self.good.fetch_sub(1, Relaxed) <= 0 {
                if self.stays {
                    return Err(hep_root::Error::Remote(
                        "the range asked is too long".into(),
                    ));
                }
                self.down.store(true, Relaxed);
                return Err(hep_root::Error::Remote("the connection was reset".into()));
            }
            buf.copy_from_slice(&self.bytes[offset as usize..offset as usize + buf.len()]);
            Ok(())
        }

        fn describe(&self) -> String {
            "held".into()
        }
    }

    /// Open a file of 8 bytes whose copies A, B, C, D answer as `answers` says (size, or why
    /// not), each serving `good` reads.
    fn opened(
        answers: [Result<u64, &'static str>; 4],
        good: [isize; 4],
        first: Vec<usize>,
    ) -> Result<Reader, (String, bool)> {
        let order: Vec<Location> = ["A", "B", "C", "D"]
            .iter()
            .map(|rse| copy(rse, &format!("{}:1", rse.to_lowercase())))
            .collect();
        let downs: Vec<Arc<std::sync::atomic::AtomicBool>> =
            good.iter().map(|_| Default::default()).collect();
        let readers: Vec<Box<dyn ByteSource + Send + Sync>> = good
            .iter()
            .zip(&downs)
            .map(|(&good, down)| {
                Box::new(Held {
                    bytes: (0..8).collect(),
                    good: good.max(0).into(),
                    stays: good < 0,
                    down: down.clone(),
                }) as Box<dyn ByteSource + Send + Sync>
            })
            .collect();
        let urls: Vec<String> = order.iter().map(|c| c.url.clone()).collect();
        let ask: Ask = Arc::new(move |url: &str| {
            let at = urls.iter().position(|u| u == url).unwrap();
            if answers[at] == Err("slow") {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if downs[at].load(std::sync::atomic::Ordering::Relaxed) {
                return Err(("it went down".to_owned(), true));
            }
            answers[at]
                .map(|bytes| (Some(bytes), None))
                .map_err(|why| (why.to_owned(), why == "busy"))
        });
        let file = File {
            at: "root://global//store/f.root".into(),
            bytes: 8,
            ..toml::from_str("at = \"x\"\nbytes = 0").unwrap()
        };
        Copies::open(&file, order, readers, ask, Default::default(), first)
    }

    #[test]
    fn a_copy_that_is_down_or_not_the_file_is_passed_over() {
        let reader = opened([Err("down"), Ok(9), Ok(8), Ok(8)], [9; 4], vec![0, 1]).unwrap();
        assert_eq!(reader.served().unwrap().0, "C");
        let learnt = held(&reader.0.learnt);
        let passed: Vec<&String> = learnt.passed.keys().collect();
        assert_eq!(passed, ["A (a:1)", "B (b:1)"]);
        assert!(learnt.passed["B (b:1)"]
            .1
            .contains("not the file the lock names"));
    }

    #[test]
    fn a_copy_that_fails_after_another_was_chosen_is_still_counted() {
        let reader = opened([Ok(8), Err("slow"), Ok(8), Ok(8)], [9; 4], vec![0, 1]).unwrap();
        assert_eq!(reader.served().unwrap().0, "A");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let learnt = held(&reader.0.learnt);
        assert!(learnt.timed.contains_key("a:1"));
        assert_eq!(learnt.failed.get("b:1"), Some(&1));
        assert_eq!(learnt.passed["B (b:1)"], (1, "slow".to_owned()));
    }

    #[test]
    fn a_read_that_fails_part_way_carries_on_at_the_next_copy() {
        let reader = opened([Ok(8), Ok(8), Ok(8), Ok(8)], [1, 0, 5, 5], vec![0]).unwrap();
        let mut buf = [0; 4];
        reader.read_at(0, &mut buf).unwrap();
        assert_eq!(reader.served().unwrap().0, "A");
        reader.read_at(4, &mut buf).unwrap();
        assert_eq!(buf, [4, 5, 6, 7], "B failed too; C served it");
        assert_eq!(reader.served().unwrap().0, "C");
        reader.finish();
        reader.finish();
        let learnt = held(&reader.0.learnt);
        assert_eq!(learnt.served["A"], (0, 4), "A's bytes, not its file");
        assert_eq!(learnt.served["C"], (1, 4), "the file, counted once");
        drop(learnt);
        let gone = reader.0.gone();
        assert_eq!(gone.len(), 2);
        assert!(
            gone[0].1.contains("the connection was reset"),
            "{}",
            gone[0].1
        );
    }

    #[test]
    fn a_read_every_copy_fails_is_the_reads_own_and_leaves_no_copy() {
        let reader = opened([Ok(8), Ok(8), Ok(8), Ok(8)], [-1; 4], vec![0]).unwrap();
        let mut buf = [0; 4];
        let said = reader.read_at(0, &mut buf).unwrap_err().to_string();
        assert!(said.contains("the range asked is too long"), "{said}");
        assert_eq!(reader.served().unwrap().0, "A");
        assert!(reader.0.gone().is_empty());
        assert!(held(&reader.0.learnt).failed.is_empty());
    }

    #[test]
    fn a_read_one_copy_fails_and_another_serves_leaves_the_first() {
        let reader = opened([Ok(8), Ok(8), Ok(8), Ok(8)], [-1, 9, 9, 9], vec![0]).unwrap();
        let mut buf = [0; 4];
        reader.read_at(4, &mut buf).unwrap();
        assert_eq!(buf, [4, 5, 6, 7]);
        assert_eq!(reader.served().unwrap().0, "B");
        assert_eq!(reader.0.gone().len(), 1);
    }

    #[test]
    fn a_file_no_copy_can_serve_says_why_for_each() {
        let (why, busy) = opened(
            [Err("down"), Err("refused"), Ok(7), Err("gone")],
            [0; 4],
            vec![0, 1, 2],
        )
        .err()
        .unwrap();
        for said in [
            "A (a:1): down",
            "B (b:1): refused",
            "C (c:1): it is not the file",
            "D (d:1): gone",
        ] {
            assert!(why.contains(said), "{why}");
        }
        assert!(!busy, "a copy that is not the file stays so");
        let (_, busy) = opened([Err("busy"), Err("busy"), Err("busy"), Err("busy")], [0; 4], vec![0])
            .err()
            .unwrap();
        assert!(busy, "every copy busy: asking again may pass");
        let reader = opened(
            [Ok(8), Err("down"), Err("down"), Err("down")],
            [0; 4],
            vec![0],
        )
        .unwrap();
        let mut buf = [0; 1];
        assert!(reader.read_at(0, &mut buf).is_err());
        assert!(reader.read_at(0, &mut buf).is_err(), "and stays failed");
    }

    #[test]
    fn the_first_question_goes_to_the_fastest_and_two_unmeasured() {
        let copies = [
            copy("A", "a:1"),
            copy("B", "b:1"),
            copy("C", "c:1"),
            copy("D", "d:1"),
        ];
        let mut learnt = Learnt::default();
        assert_eq!(learnt.first(&copies), [0, 1, 2]);
        learnt.timed.insert("c:1".into(), 40);
        let order = learnt.order(&copies);
        assert_eq!(learnt.first(&order), [0, 1, 2], "C first, then A and B");
        assert_eq!(order[0].rse, "C");
        learnt
            .timed
            .extend([("a:1".into(), 50), ("b:1".into(), 60), ("d:1".into(), 70)]);
        assert_eq!(
            learnt.first(&learnt.order(&copies)),
            [0],
            "all measured: the fastest alone"
        );
    }

    #[test]
    fn the_fastest_measured_storage_comes_first_and_twice_failed_ones_last() {
        let copies = [
            copy("A", "a:1"),
            copy("B", "b:1"),
            copy("C", "c:1"),
            copy("D", "d:1"),
        ];
        let mut learnt = Learnt::default();
        let order = |learnt: &Learnt| {
            learnt
                .order(&copies)
                .iter()
                .map(|c| c.rse.clone())
                .collect::<String>()
        };
        assert_eq!(
            order(&learnt),
            "ABCD",
            "Rucio's order while nothing is measured"
        );
        learnt.timed.insert("c:1".into(), 40);
        learnt.timed.insert("d:1".into(), 900);
        assert_eq!(order(&learnt), "CDAB");
        learnt.failed.insert("c:1".into(), 1);
        assert_eq!(order(&learnt), "CDAB", "one failure may be the file's own");
        learnt.failed.insert("c:1".into(), 2);
        assert_eq!(order(&learnt), "DABC");
    }
}
