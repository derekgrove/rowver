//! Which entries of a sample a press reads, from its files' entries and its `events`: all of them,
//! the first n in the files' order, or n chosen at random. A pure function of the lock, so
//! `rvr press --events …` can plan again without asking any storage.

use super::spec::Events;

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

/// SplitMix64 (Steele, Lea and Flood 2014): the generator a random choice is drawn from.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in [0, 1) from the top 53 bits.
    pub fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The entries a random choice of n out of `total` picks, ascending, counted over the files
/// one after another: Knuth's selection sampling (Algorithm S), entry i chosen when
/// (total - i) * u < still needed, with u drawn from SplitMix64(seed) for every entry in turn.
pub fn chosen(total: u64, n: u64, seed: u64, mut each: impl FnMut(u64)) {
    let mut rng = SplitMix64::new(seed);
    let mut needed = n.min(total);
    let mut i = 0;
    while needed > 0 {
        if ((total - i) as f64) * rng.uniform() < needed as f64 {
            each(i);
            needed -= 1;
        }
        i += 1;
    }
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
        Events::Random { n, .. } if n >= total => plan(entries, Events::All),
        Events::Random { n, seed } => {
            let starts: Vec<u64> = entries
                .iter()
                .scan(0, |at, &e| {
                    let s = *at;
                    *at += e;
                    Some(s)
                })
                .collect();
            let mut take = vec![0u64; entries.len()];
            let mut file = 0;
            chosen(total, n, seed, |i| {
                while i >= starts[file] + entries[file] {
                    file += 1;
                }
                take[file] += 1;
            });
            take.into_iter()
                .map(|t| Read {
                    read: t,
                    stop: None,
                    take: Some(t),
                })
                .collect()
        }
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
        let mut seen = Vec::new();
        chosen(1000, 10, 3, |i| seen.push(i));
        assert_eq!(seen.len(), 10);
        assert!(seen.windows(2).all(|w| w[0] < w[1]) && *seen.last().unwrap() < 1000);
    }
}
