//! Which entries of a sample a press reads, from its files' entries and its `events`: all of them,
//! the first n in the files' order, or n chosen at random. A pure function of the lock, so
//! `rvr press --events …` can plan again without asking any storage.

use super::spec::Events;
use rowver::choice::counts;

/// What a press reads of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read {
    /// Entries read from the file.
    pub read: u64,
    /// For the first n: read entries 0..stop (absent when the whole file is read).
    pub stop: Option<u64>,
    /// For a random choice: how many of the chosen entries are in this file.
    pub take: Option<u64>,
}

/// What a press reads of each file, given each file's entries.
pub fn plan(entries: &[u64], events: Events) -> Vec<Read> {
    let total: u64 = entries.iter().sum();
    match events {
        Events::All => entries
            .iter()
            .map(|&e| Read {
                read: e,
                stop: None,
                take: None,
            })
            .collect(),
        Events::First(n) => {
            let mut left = n;
            entries
                .iter()
                .map(|&e| {
                    let read = e.min(left);
                    left -= read;
                    Read {
                        read,
                        stop: (read < e).then_some(read),
                        take: None,
                    }
                })
                .collect()
        }
        Events::Random { n, seed } => counts(entries, n.min(total), seed)
            .into_iter()
            .map(|take| Read {
                read: take,
                stop: None,
                take: Some(take),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_n_reads_files_in_order_and_stops_in_one() {
        let p = plan(&[100, 50, 80], Events::First(120));
        assert_eq!(
            p.iter().map(|r| (r.read, r.stop)).collect::<Vec<_>>(),
            [(100, None), (20, Some(20)), (0, Some(0))]
        );
        assert_eq!(
            plan(&[100, 50], Events::First(1000))
                .iter()
                .map(|r| r.read)
                .sum::<u64>(),
            150
        );
    }

    #[test]
    fn a_random_choice_is_exactly_n_and_the_same_for_the_same_seed() {
        let entries = [867_540, 8_332, 714_243];
        let a = plan(&entries, Events::Random { n: 50_000, seed: 7 });
        let b = plan(&entries, Events::Random { n: 50_000, seed: 7 });
        let c = plan(&entries, Events::Random { n: 50_000, seed: 8 });
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.iter().map(|r| r.read).sum::<u64>(), 50_000);
        // Spread like the files: about 54.9 %, 0.5 % and 45.1 % of the choice.
        let share = |i: usize| a[i].read as f64 / 50_000.0;
        assert!(
            (share(0) - 0.5485).abs() < 0.01
                && (share(1) - 0.0053).abs() < 0.003
                && (share(2) - 0.4516).abs() < 0.01
        );
    }
}
