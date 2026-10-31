//! Read a file of a catalog dataset from whichever of its copies answers. Rucio is asked once,
//! as a press starts, where every dataset file can be read; `rucio-client` keeps only copies at
//! CMS storage doors. The storages the lock recorded for a sample come first, and when Rucio is
//! unreachable the files are read from them alone. Rucio's order is not a ranking by speed (CMS's runs by host name), so speed
//! is measured: a file's first question (a HEAD for its size and adler32) goes at once to the
//! storage that answered fastest so far and to up to two never measured, and the first to answer
//! with the lock's size (and adler32, when the storage gives one) is read; every answer is
//! timed, so each storage of a dataset gets measured as its files open. A storage that failed
//! twice since it last served a whole file is tried last. A read that fails is tried at the other
//! copies, since every copy holds the same bytes: the copies it failed at are left once another
//! serves it, and a read that every copy fails is the read's own.

use super::remote::{digest_once, Digest, HttpsFile, Refusal, Storage};
use super::resolve::{self, File, Site};
use crate::holes::{Failed, Fault, Seen};
use grid_catalog::Did;
use hep_root::ByteSource;
use rucio_client::Location;
use std::collections::{BTreeMap, HashMap};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// How many copies a file's first question goes to at once.
const RACED: usize = 3;

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
    /// The copies in the order to try them: fastest measured first, then the unmeasured in the
    /// order given (the sample's recorded sites, then Rucio's), then those that failed twice running.
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
    /// Why a file cannot be read at all: only on tape, not listed, not the lock's file, or, with
    /// Rucio unreachable, at no recorded site.
    problems: HashMap<String, Failed>,
    learnt: Arc<Mutex<Learnt>>,
    /// Why Rucio did not answer, when the files are read from the sites the lock recorded.
    pub unasked: Option<String>,
}

/// A dataset file to read: the file, the dataset it is of, and its sample's recorded sites.
pub type Wanted<'a> = (&'a File, &'a str, &'a [Site]);

/// Whether a storage's answer to a file's first question (`digest`) says it holds the lock's file:
/// its size said and the lock's, and its adler32 the lock's when the storage gives one.
pub(crate) fn holding(file: &File, digest: Result<Digest, Refusal>) -> Result<(), Refusal> {
    let (bytes, adler32) = digest?;
    if bytes.is_none() {
        return Err(Refusal::lasting(Fault::Unreachable, "its storage did not say how big the file is"));
    }
    match resolve::differs(file, bytes, adler32.as_deref()) {
        Some(how) => Err(Refusal::lasting(Fault::Changed, format!("it is not the file the lock names ({how})"))),
        None => Ok(()),
    }
}

/// Whether a press reads around Rucio failing with `code`, from the recorded sites: only when
/// Rucio could not be reached, and some file has them. Any other answer stops it with its words.
fn reads_around(code: &str, files: &[Wanted]) -> bool {
    code == "network" && files.iter().any(|(_, _, sites)| !sites.is_empty())
}

/// A file's copies with those at its sample's recorded sites first, in their order, then the
/// rest in Rucio's.
fn recorded_first(mut copies: Vec<Location>, sites: &[Site]) -> Vec<Location> {
    copies.sort_by_key(|copy| sites.iter().position(|site| site.rse == copy.rse).unwrap_or(sites.len()));
    copies
}

impl Datasets {
    /// Where each file can be read, as Rucio says it, or as the lock's recorded sites do.
    pub fn ask(rucio: &rucio_client::Rucio, files: &[Wanted]) -> Result<Self, String> {
        let mut dids: Vec<Did> = files
            .iter()
            .filter_map(|(file, _, _)| file.lfn.clone())
            .map(Did::cms)
            .collect();
        dids.sort_by(|a, b| a.name.cmp(&b.name));
        dids.dedup();
        // Each file is checked against what Rucio says: the same size and adler32, and a copy on
        // disk; a file that fails is kept with why, and fails when it is opened.
        let rucio_failed = |why: &str| format!("Rucio could not say where the dataset files are: {why}");
        let found = match rucio.copies(&dids) {
            Ok(found) => found,
            Err(e) if reads_around(&e.code, files) => {
                let around = Datasets::recorded(files, e.message.clone())?;
                // A Rucio out of reach is read around, a network gone is not: a recorded door answers.
                let mut doors: Vec<String> = around.copies.values().flatten().map(|copy| copy.door.clone()).collect();
                doors.sort();
                doors.dedup();
                return match super::remote::any_door_answers(&doors) {
                    true => Ok(around),
                    false => Err(rucio_failed(&e.message)),
                };
            }
            Err(e) => return Err(rucio_failed(&e.message)),
        };
        let (mut copies, mut problems) = (HashMap::new(), HashMap::new());
        for (file, dataset, sites) in files {
            let lfn = file.lfn.as_deref().unwrap_or_default();
            let problem = match found.get(lfn) {
                None => Failed::new(Fault::Gone, format!("{lfn}: Rucio listed no copy of it. If the dataset has changed, run rvr samples resolve samples.toml and review the new lock.")),
                Some(said) => match resolve::differs(file, said.bytes, said.adler32.as_deref()) {
                    Some(how) => Failed::new(Fault::Changed, format!("{how}, by Rucio's record. Run rvr samples resolve samples.toml and review the new lock.")),
                    None if said.copies.is_empty() => Failed::new(Fault::Tape, format!(
                        "{lfn} has no copy that can be read now ({}). It needs a copy on disk: ask \
                         for a Rucio rule that puts {dataset} on a disk storage, for example rucio \
                         add-rule cms:{dataset} 1 T2_US_Nebraska.",
                        said.left_out.join("; ")
                    )),
                    None => {
                        copies.insert(lfn.to_owned(), recorded_first(said.copies.clone(), sites));
                        continue;
                    }
                },
            };
            problems.insert(lfn.to_owned(), problem);
        }
        Ok(Datasets {
            copies,
            problems,
            learnt: Default::default(),
            unasked: None,
        })
    }

    /// The files read from the sites their samples recorded, as Rucio was unreachable (`why`):
    /// each from those of them its lock says hold it on disk; a file none holds is a hole of its
    /// own, filled once Rucio answers. Refused when no file is at such a site.
    fn recorded(files: &[Wanted], why: String) -> Result<Self, String> {
        let (mut copies, mut problems) = (HashMap::new(), HashMap::new());
        for (file, _, sites) in files {
            let lfn = file.lfn.as_deref().unwrap_or_default();
            let held: Vec<Location> = sites
                .iter()
                .filter(|site| file.disk.as_ref().is_none_or(|disk| disk.contains(&site.rse)))
                .filter_map(|site| site.copy(lfn))
                .collect();
            let tape = file.disk.as_ref().is_some_and(Vec::is_empty);
            match (held.is_empty(), tape) {
                (false, _) => {
                    copies.insert(lfn.to_owned(), held);
                }
                (true, true) => {
                    let said = format!("{lfn} had no copy on disk when samples.lock was written, only on tape: ask for a Rucio rule that puts its dataset on a disk storage, then resolve again.");
                    problems.insert(lfn.to_owned(), Failed::new(Fault::Tape, said));
                }
                (true, false) => {
                    let said = format!("{lfn}: Rucio did not answer, and no storage samples.lock recorded holds it at a CMS storage door. Once Rucio answers, rvr press --fill unreachable reads it.");
                    problems.insert(lfn.to_owned(), Failed::new(Fault::Unreachable, said));
                }
            }
        }
        // A file two samples name is read wherever one of their recorded sites holds it.
        problems.retain(|lfn, _| !copies.contains_key(lfn));
        if copies.is_empty() && problems.values().all(|problem| problem.fault == Fault::Tape) {
            return Err(format!("Rucio could not say where the dataset files are ({why}), and when samples.lock was written each was on tape only: ask for a Rucio rule that puts its dataset on a disk storage, then resolve again."));
        }
        if copies.is_empty() {
            return Err(format!("Rucio could not say where the dataset files are ({why}), and none of them is at a storage samples.lock recorded. Try again once Rucio answers."));
        }
        Ok(Datasets { copies, problems, learnt: Default::default(), unasked: Some(why) })
    }

    /// Why a file cannot be read at all, when Rucio's answer said so.
    pub fn problem(&self, file: &File) -> Option<&Failed> {
        self.problems.get(file.lfn.as_deref().unwrap_or_default())
    }

    /// Open a dataset file at the first copy that answers with the lock's size and adler32,
    /// noting the failed requests of its reads in `seen`; a failure says what it is, and whether
    /// a copy failed as a busy storage does (so that asking again may pass).
    pub fn open(&self, storage: &Storage, file: &File, seen: &Arc<Seen>) -> Result<Reader, Refusal> {
        if let Some(problem) = self.problem(file) {
            return Err(Refusal::lasting(problem.fault, &problem.why));
        }
        let lfn = file.lfn.as_deref().unwrap_or_default();
        let listed = self.copies.get(lfn).ok_or_else(|| {
            Refusal::lasting(Fault::Mistake, format!("{}: Rucio was not asked where it is (a bug in rvr: please report it)", file.at))
        })?;
        let (order, first) = {
            let learnt = held(&self.learnt);
            let order = learnt.order(listed);
            let first = learnt.first(&order);
            (order, first)
        };
        let mut readers: Vec<Box<dyn ByteSource + Send + Sync>> = Vec::new();
        for copy in &order {
            let reader = HttpsFile::new(storage, &copy.url, file.bytes)
                .map_err(|why| Refusal::lasting(Fault::Mistake, why))?;
            readers.push(Box::new(reader.seeing(seen)));
        }
        let (agent, watch) = (storage.agent(), storage.watch.clone());
        let ask: Ask = Arc::new(move |url: &str| digest_once(&agent, url, &watch));
        let opened = Copies::open(file, order, readers, ask, self.learnt.clone(), first, seen.clone());
        // An address taken from the recorded sites is said to be one: its storage's answer keeps its
        // kind, so that a fill keeps the kinds of the holes it reads.
        match &self.unasked {
            Some(_) => opened.map_err(|refused| Refusal {
                why: format!("{} (at an address samples.lock's recorded storages give, as Rucio could not be reached)", refused.why),
                ..refused
            }),
            None => opened,
        }
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

/// Ask a copy's storage for the file's size and adler32 at an address.
type Ask = Arc<dyn Fn(&str) -> Result<Digest, Refusal> + Send + Sync>;

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
    /// Each copy left, by its place in `order`, with why; shared with the questions in flight.
    gone: Arc<Mutex<Vec<(usize, Refusal)>>>,
    learnt: Arc<Mutex<Learnt>>,
    /// What its copies' failed requests said, the last.
    seen: Arc<Seen>,
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
        seen: Arc<Seen>,
    ) -> Result<Reader, Refusal> {
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
            seen,
        };
        let mut current = copies.current();
        copies.choose(&mut current, first)?;
        drop(current);
        Ok(Reader(Arc::new(copies)))
    }

    fn current(&self) -> MutexGuard<'_, usize> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn gone(&self) -> MutexGuard<'_, Vec<(usize, Refusal)>> {
        self.gone.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a copy holds the file the lock names, by its storage's size and adler32 (the
    /// adler32 only when the storage gives one); why not, when it does not.
    fn holds(ask: &Ask, file: &File, copy: &Location) -> Result<(), Refusal> {
        holding(file, ask(&copy.url))
    }

    /// Leave a copy for good, counting the failure against its storage.
    fn leave(&self, index: usize, refusal: Refusal) {
        let why = failed(&self.learnt, &self.order[index], &refusal.why);
        self.gone().push((index, Refusal { why, ..refusal }));
    }

    /// Choose the copy to read: ask the copies `first` names at once, and take the first to
    /// answer with the lock's size and adler32; then the others not yet left, one at a time.
    fn choose(&self, current: &mut usize, first: Vec<usize>) -> Result<(), Refusal> {
        let mut asked = first;
        loop {
            let gone: Vec<usize> = self.gone().iter().map(|(index, _)| *index).collect();
            asked.retain(|i| !gone.contains(i));
            if asked.is_empty() {
                asked.extend((0..self.order.len()).find(|i| !gone.contains(i)));
            }
            if asked.is_empty() {
                *current = self.order.len();
                let gone = self.gone();
                let why: Vec<&str> = gone.iter().map(|(_, refused)| refused.why.as_str()).collect();
                let said = format!("{}: no copy of it could be read: {}", self.file.at, why.join("; "));
                let faults: Vec<Fault> = gone.iter().map(|(_, refused)| refused.fault).collect();
                let all = |one: Fault| faults.iter().all(|f| *f == one);
                // A copy that is busy may serve when asked again. If none does, every copy refusing
                // it with a valid proxy is refused; else a copy that is not the lock's file says
                // the lock is out of date; else what every copy says, or that none could be reached.
                let busy = gone.iter().any(|(_, refused)| refused.busy);
                let fault = match () {
                    _ if all(Fault::Refused) => Fault::Refused,
                    _ if faults.contains(&Fault::Changed) => Fault::Changed,
                    _ if all(Fault::Gone) => Fault::Gone,
                    _ if all(Fault::Save) => Fault::Save,
                    _ => Fault::Unreachable,
                };
                return Err(Refusal { why: said, fault, busy });
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
                        Err(refused) => {
                            let why = failed(&learnt, &copy, &refused.why);
                            gone.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push((index, Refusal { why, ..refused.clone() }));
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
        let mut errors = vec![error];
        let gone: Vec<usize> = self.gone().iter().map(|(at, _)| *at).collect();
        for other in (0..self.order.len()).filter(|at| *at != index && !gone.contains(at)) {
            if let Err(refused) = Copies::holds(&self.ask, &self.file, &self.order[other]) {
                self.leave(other, refused);
                continue;
            }
            match read(self.readers[other].as_ref()) {
                Ok(value) => {
                    for (at, why) in failed {
                        let refused = Refusal { why, fault: Fault::Unreachable, busy: true };
                        self.leave(at, refused);
                    }
                    *current = other;
                    return Ok(served(other, value));
                }
                Err(failed_too) => {
                    failed.push((other, failed_too.to_string()));
                    errors.push(failed_too);
                }
            }
        }
        // A copy that failed as a busy storage does may serve when asked again: its failure is
        // the read's, and noted as such. Otherwise the last copy's, which `seen` names.
        match errors.iter().position(|e| matches!(e, hep_root::Error::Remote(_))) {
            Some(busy) => {
                self.seen.note(Fault::Unreachable);
                Err(errors.swap_remove(busy))
            }
            None => Err(errors.pop().expect("one read failed")),
        }
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

    #[test]
    fn a_samples_recorded_sites_come_first_and_stand_in_when_rucio_does_not_answer() {
        let lfn = "/store/f.root";
        let at = |rse: &str, door: &str| Location { url: format!("https://{door}/prefix{lfn}"), door: door.into(), rse: rse.into(), priority: 0 };
        let (cern, fnal) = (at("T2_CH_CERN", "eoscms.cern.ch:443"), at("T1_US_FNAL_Disk", "cmsdcadisk.fnal.gov:2880"));
        let sites: Vec<Site> = [&fnal, &cern].iter().map(|copy| Site::of(copy, lfn).unwrap()).collect();
        assert_eq!(sites[0], Site { rse: "T1_US_FNAL_Disk".into(), prefix: "https://cmsdcadisk.fnal.gov:2880/prefix".into() });
        assert_eq!(sites[0].copy(lfn), Some(fnal.clone()));
        // An address that is not a prefix and the file's name gives no site; a door that is no CMS
        // storage door, as a lock from elsewhere might name, is never asked.
        assert!(Site::of(&cern, "/store/other.root").is_none());
        let elsewhere = Site { rse: "T2_US_Purdue".into(), prefix: "https://example.org:443/store".into() };
        assert_eq!(elsewhere.copy(lfn), None);
        let order = recorded_first(vec![copy("A", "a"), cern.clone(), fnal.clone(), copy("D", "d")], &sites);
        assert_eq!(order.iter().map(|c| c.rse.as_str()).collect::<Vec<_>>(), ["T1_US_FNAL_Disk", "T2_CH_CERN", "A", "D"]);
        // Rucio unreachable: each file from the recorded sites its lock says hold it.
        let file = |lfn: &str, disk: &[&str]| File {
            at: format!("root://cms-xrd-global.cern.ch/{lfn}"),
            bytes: 1,
            adler32: None,
            modified: None,
            lfn: Some(lfn.into()),
            events: Some(1),
            disk: Some(disk.iter().map(|rse| rse.to_string()).collect()),
            uuid: None,
            entries: Some(1),
            stop: None,
            take: None,
        };
        let (held, other) = (file(lfn, &["A", "T2_CH_CERN"]), file("/store/g.root", &["A"]));
        let dataset = "/Primary/Processed/NANOAODSIM";
        let tape = file("/store/t.root", &[]);
        let found = Datasets::recorded(&[(&held, dataset, &sites), (&other, dataset, &sites), (&tape, dataset, &sites)], "it did not answer".into()).unwrap();
        assert_eq!(found.problem(&tape).unwrap().fault, Fault::Tape);
        // A file two samples name: one sample's recorded site holds it, so it is read there.
        let both = Datasets::recorded(&[(&held, dataset, &[]), (&held, dataset, &sites)], "it did not answer".into()).unwrap();
        assert!(both.problem(&held).is_none() && both.copies.contains_key(lfn));
        assert_eq!(found.copies[lfn], [cern]);
        let hole = found.problem(&other).unwrap();
        assert!(hole.fault == Fault::Unreachable && hole.why.contains("rvr press --fill unreachable") && !hole.why.contains("it did not answer"), "{}", hole.why);
        assert_eq!(found.unasked.as_deref(), Some("it did not answer"));
        // Only Rucio out of reach is read around, and only with recorded sites.
        assert!(reads_around("network", &[(&held, dataset, &sites)]));
        assert!(!reads_around("network", &[(&held, dataset, &[])]));
        for code in ["proxy_refused", "catalog_unavailable", "catalog_error", "ca_untrusted"] {
            assert!(!reads_around(code, &[(&held, dataset, &sites)]), "{code}");
        }
        // A name that is not a path, which could change the host it is asked of, has no copy.
        assert_eq!(sites[0].copy("@example.org/store/f.root"), None);
        // With no file at a recorded site, the press stops, saying why.
        let refused = Datasets::recorded(&[(&other, dataset, &sites)], "it did not answer".into()).err().unwrap();
        assert!(refused.contains("(it did not answer), and none of them is at a storage samples.lock recorded"), "{refused}");
    }

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
    ) -> Result<Reader, Refusal> {
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
                return Err(Refusal { why: "it went down".into(), fault: Fault::Unreachable, busy: true });
            }
            answers[at].map(|bytes| (Some(bytes), None)).map_err(|why| Refusal {
                why: why.to_owned(),
                fault: if why == "refused" { Fault::Refused } else { Fault::Unreachable },
                busy: why == "busy" || why == "refused",
            })
        });
        let file = File {
            at: "root://global//store/f.root".into(),
            bytes: 8,
            ..toml::from_str("at = \"x\"\nbytes = 0").unwrap()
        };
        Copies::open(&file, order, readers, ask, Default::default(), first, Default::default())
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
            gone[0].1.why.contains("the connection was reset"),
            "{}",
            gone[0].1.why
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
        let failed = opened(
            [Err("down"), Err("denied"), Ok(7), Err("gone")],
            [0; 4],
            vec![0, 1, 2],
        )
        .err()
        .unwrap();
        for said in [
            "A (a:1): down",
            "B (b:1): denied",
            "C (c:1): it is not the file",
            "D (d:1): gone",
        ] {
            assert!(failed.why.contains(said), "{}", failed.why);
        }
        assert!(!failed.busy, "a copy that is not the file stays so");
        assert_eq!(failed.fault, Fault::Changed);
        let failed = opened([Err("busy"), Err("busy"), Err("busy"), Err("busy")], [0; 4], vec![0])
            .err()
            .unwrap();
        assert!(failed.busy, "every copy busy: asking again may pass");
        let failed = opened([Ok(7), Err("busy"), Err("down"), Err("down")], [0; 4], vec![0, 1])
            .err()
            .unwrap();
        assert!(failed.busy, "one busy copy may serve it yet: asked again");
        assert_eq!(failed.fault, Fault::Changed, "if none does, the copy not the lock's file names it");
        let failed = opened([Err("refused"), Err("refused"), Err("refused"), Err("refused")], [0; 4], vec![0])
            .err()
            .unwrap();
        assert_eq!(failed.fault, Fault::Refused, "every copy refuses it with a proxy");
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
