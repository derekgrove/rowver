//! The prefetching byte source a press reads a file through: the baskets of a run, fetched in a
//! few requests at once, then answered from memory (docs/SPEC.md §7.2.5, §7.2.6).

use crate::holes::Seen;
use crate::samples::remote::{read_again, Watch};
use hep_root::ByteSource;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A source that keeps the ranges it was last told to hold, so that the basket reads of a chunk
/// are answered from memory: every basket the recipe may read of a remote file, fetched in a
/// few requests at once instead of branch after branch. A read that held ranges cover only in
/// part fetches just the rest.
pub(crate) struct Held {
    /// The file's bytes themselves.
    pub(crate) inner: Box<dyn ByteSource>,
    /// Disjoint ranges, in order of position.
    pub(crate) held: Mutex<Vec<(u64, Vec<u8>)>>,
    /// For a remote file: how long a read that failed waits to be asked again, and the watch
    /// that ends the waiting (another input failed). Fetching ahead is never asked again.
    pub(crate) again: Option<(&'static [u64], Arc<Watch>)>,
    /// For a remote file, what its last failed request said.
    pub(crate) seen: Option<Arc<Seen>>,
    /// What the file is, for what is printed while a read waits.
    pub(crate) shown: String,
    /// Where the bytes read from it are counted, for the bars.
    pub(crate) bytes: Arc<AtomicU64>,
}

/// The parts of `offset..offset + len` that no held range covers.
fn gaps(held: &[(u64, Vec<u8>)], offset: u64, len: usize) -> Vec<(u64, usize)> {
    let (end, mut at, mut out) = (offset + len as u64, offset, Vec::new());
    for (start, bytes) in held {
        let stop = start + bytes.len() as u64;
        if stop <= at {
            continue;
        }
        if *start >= end {
            break;
        }
        if *start > at {
            out.push((at, (start - at) as usize));
        }
        at = stop;
        if at >= end {
            break;
        }
    }
    if at < end {
        out.push((at, (end - at) as usize));
    }
    out
}

/// Copy into `buf` (which starts at `offset`) whatever `pieces` hold of it.
fn copy_from(pieces: &[(u64, Vec<u8>)], offset: u64, buf: &mut [u8]) {
    let end = offset + buf.len() as u64;
    for (start, bytes) in pieces {
        let (from, to) = ((*start).max(offset), (start + bytes.len() as u64).min(end));
        if from < to {
            let (f, t) = ((from - offset) as usize, (to - offset) as usize);
            buf[f..t].copy_from_slice(&bytes[(from - start) as usize..(to - start) as usize]);
        }
    }
}

impl Held {
    /// Hold these spans, joined where they lie within 16 kB (a storage that answers many
    /// ranges in one request carries a short gap faster than it seeks past one). What is
    /// already held of them is kept and not fetched again; the rest of what was held is dropped.
    pub(crate) fn hold(&self, spans: Vec<(u64, u64)>) -> hep_root::Result<()> {
        const NEAR: u64 = 16 * 1024;
        let ranges: Vec<(u64, usize)> = spans.iter().map(|&(at, len)| (at, len as usize)).collect();
        let wanted = crate::samples::remote::joined(&ranges, NEAR);
        let overlaps = |start: u64, len: usize| {
            wanted
                .iter()
                .any(|&(at, l)| start < at + l as u64 && at < start + len as u64)
        };
        let mut kept = std::mem::take(&mut *self.held.lock().unwrap());
        kept.retain(|(start, bytes)| overlaps(*start, bytes.len()));
        let missing: Vec<(u64, usize)> = wanted
            .iter()
            .flat_map(|&(at, len)| gaps(&kept, at, len))
            .collect();
        let fetched = match self.inner.read_ranges(&missing) {
            Ok(fetched) => {
                self.answered();
                self.counted(&missing);
                fetched
            }
            Err(e) => {
                *self.held.lock().unwrap() = kept;
                return Err(e);
            }
        };
        kept.extend(missing.iter().map(|r| r.0).zip(fetched));
        kept.sort_unstable_by_key(|(start, _)| *start);
        *self.held.lock().unwrap() = kept;
        Ok(())
    }

    /// Bytes of `ranges` read from the file.
    fn counted(&self, ranges: &[(u64, usize)]) {
        let bytes: usize = ranges.iter().map(|(_, len)| len).sum();
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Read from the file itself, asked again while a remote read fails as a busy storage does.
    fn fetch(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        let read = match &self.again {
            Some((waits, watch)) => {
                let read = read_again(waits, watch, &self.shown, || {
                    self.inner.read_ranges(ranges)
                });
                if read.is_ok() {
                    self.answered();
                }
                read
            }
            None => self.inner.read_ranges(ranges),
        };
        if read.is_ok() {
            self.counted(ranges);
        }
        read
    }

    /// A remote read succeeded: what failed before it is past.
    fn answered(&self) {
        if let Some(seen) = &self.seen {
            seen.clear();
        }
    }

    /// For a remote file, what its last failed request said.
    pub(crate) fn seen(&self) -> Option<&Seen> {
        self.seen.as_deref()
    }
}

impl ByteSource for Held {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
        let [read] = &mut self.read_ranges(&[(offset, buf.len())])?[..] else {
            return Err(hep_root::Error::Io(std::io::Error::other("one range was asked for, and another number came back (a bug in rvr: please report it)")));
        };
        buf.copy_from_slice(read);
        Ok(())
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn read_ranges(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        let held = self.held.lock().unwrap();
        if held.is_empty() {
            drop(held);
            return self.fetch(ranges);
        }
        let mut out: Vec<Vec<u8>> = ranges.iter().map(|&(_, len)| vec![0; len]).collect();
        for (buf, &(offset, _)) in out.iter_mut().zip(ranges) {
            copy_from(&held, offset, buf);
        }
        let missing: Vec<(u64, usize)> = ranges
            .iter()
            .flat_map(|&(at, len)| gaps(&held, at, len))
            .collect();
        drop(held);
        if !missing.is_empty() {
            let fetched: Vec<(u64, Vec<u8>)> = missing
                .iter()
                .map(|r| r.0)
                .zip(self.fetch(&missing)?)
                .collect();
            for (buf, &(offset, _)) in out.iter_mut().zip(ranges) {
                copy_from(&fetched, offset, buf);
            }
        }
        Ok(out)
    }
}

/// The on-disk spans of the baskets of `branches` holding entries `first..last`.
pub(crate) fn spans(branches: &[&hep_root::Branch], first: i64, last: i64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for branch in branches {
        let baskets =
            (branch.write_basket.max(0) as usize).min(branch.basket_entry.len().saturating_sub(1));
        for i in 0..baskets {
            let (seek, bytes) = (
                branch.basket_seek.get(i).copied().unwrap_or(0),
                branch.basket_bytes.get(i).copied().unwrap_or(0),
            );
            if branch.basket_entry[i] < last
                && branch.basket_entry[i + 1] > first
                && seek > 0
                && bytes > 0
            {
                out.push((seek as u64, bytes as u64));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source of known bytes that records every range asked of it.
    struct Asked(Vec<u8>, std::sync::Arc<Mutex<Vec<(u64, usize)>>>);

    impl ByteSource for Asked {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
            self.1.lock().unwrap().push((offset, buf.len()));
            buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
            Ok(())
        }

        fn describe(&self) -> String {
            "memory".into()
        }
    }

    #[test]
    fn held_ranges_answer_reads_and_only_the_rest_is_fetched() {
        let bytes: Vec<u8> = (0..=255).cycle().take(200_000).collect();
        let log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let held = Held {
            inner: Box::new(Asked(bytes.clone(), log.clone())),
            held: Mutex::new(Vec::new()),
            again: None,
            seen: None,
            shown: String::new(),
            bytes: Default::default(),
        };
        held.hold(vec![(1_000, 100), (1_200, 100), (100_000, 50)])
            .unwrap();
        // The two near spans were joined into one; the far one stayed apart.
        assert_eq!(*log.lock().unwrap(), [(1_000, 300), (100_000, 50)]);
        log.lock().unwrap().clear();

        // Held bytes come from memory; a read reaching past them fetches only what is missing.
        let read = held.read_ranges(&[(1_050, 200), (100_000, 80)]).unwrap();
        assert_eq!(read[0], bytes[1_050..1_250]);
        assert_eq!(read[1], bytes[100_000..100_080]);
        assert_eq!(*log.lock().unwrap(), [(100_050, 30)]);
        log.lock().unwrap().clear();

        // The next chunk keeps what it shares with the last and fetches only the rest.
        held.hold(vec![(100_000, 50), (100_050, 100)]).unwrap();
        assert_eq!(*log.lock().unwrap(), [(100_050, 100)]);
        let mut buf = vec![0; 150];
        held.read_at(100_000, &mut buf).unwrap();
        assert_eq!(buf, bytes[100_000..100_150]);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn gaps_are_what_no_held_range_covers() {
        let held = vec![(10, vec![0; 10]), (30, vec![0; 10])];
        assert_eq!(gaps(&held, 0, 50), [(0, 10), (20, 10), (40, 10)]);
        assert_eq!(gaps(&held, 12, 5), []);
        assert_eq!(gaps(&held, 15, 20), [(20, 10)]);
        assert_eq!(gaps(&[], 5, 5), [(5, 5)]);
    }
}
