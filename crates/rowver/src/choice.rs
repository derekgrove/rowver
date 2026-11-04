//! Which entries of a sample a press reads when it chooses them at random: the algorithm a file
//! names as `splitmix64-selection-sampling`, so a reader can choose them again from the seed.

use crate::json::natural as whole;
use crate::{invalid, Result};
use serde_json::{json, Value};

/// The name a file gives this algorithm in `made.files[sample].events.algorithm`.
pub const ALGORITHM: &str = "splitmix64-selection-sampling";

/// How many of a sample's entries a press reads, and which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Events {
    /// Every entry of every file.
    All,
    /// The first n entries, in the files' order.
    First(u64),
    /// n entries chosen at random, the same n for the same seed.
    Random {
        /// How many.
        n: u64,
        /// The seed they are chosen by.
        seed: u64,
    },
}

impl Events {
    /// The `events` record a file holds of this choice.
    pub fn record(self) -> Value {
        match self {
            Events::All => json!({"choose": "all"}),
            Events::First(n) => json!({"choose": "first", "n": n}),
            Events::Random { n, seed } => {
                json!({"choose": "random", "n": n, "seed": seed, "algorithm": ALGORITHM})
            }
        }
    }

    /// The choice an `events` record names, refused with any other field or algorithm.
    pub fn read(record: &Value) -> Result<Events> {
        let n = || {
            whole(&record["n"])
                .filter(|n| (1..=i64::MAX as u64).contains(n))
                .ok_or_else(|| invalid("events.n must be a whole number from 1 to 2^63 − 1"))
        };
        let (events, fields) = match record["choose"].as_str() {
            Some("all") => (Events::All, 1),
            Some("first") => (Events::First(n()?), 2),
            Some("random") => {
                if record["algorithm"] != ALGORITHM {
                    return Err(invalid(format!(
                        "events.algorithm is not {ALGORITHM}, the one this reader knows"
                    )));
                }
                let seed = whole(&record["seed"]).ok_or_else(|| {
                    invalid("events.seed must be a whole number from 0 to 2^64 - 1")
                })?;
                (Events::Random { n: n()?, seed }, 4)
            }
            _ => return Err(invalid("events.choose is not all, first or random")),
        };
        if record.as_object().map(serde_json::Map::len) != Some(fields) {
            return Err(invalid(format!(
                "events holds fields a {} choice does not have (exactly choose{})",
                record["choose"],
                ["", ", n", ", n, seed, algorithm"][fields.min(4) / 2]
            )));
        }
        Ok(events)
    }
}

/// The entries a random choice of n out of `total` picks, ascending, counted over the files
/// one after another: Knuth's selection sampling (Algorithm S), entry i chosen when
/// (total - i) * u < still needed, with u drawn from SplitMix64(seed) (Steele, Lea and Flood
/// 2014) for every entry in turn, from the top 53 bits of each draw.
pub(crate) fn chosen(total: u64, n: u64, seed: u64, mut each: impl FnMut(u64)) {
    let mut state = seed;
    let mut uniform = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut needed = n.min(total);
    let mut i = 0;
    while needed > 0 {
        if ((total - i) as f64) * uniform() < needed as f64 {
            each(i);
            needed -= 1;
        }
        i += 1;
    }
}

/// How many entries of each file a random choice picks, given each file's entries in order.
pub(crate) fn counts(entries: &[u64], n: u64, seed: u64) -> Vec<u64> {
    let mut taken = vec![0; entries.len()];
    each_file(entries, n, seed, |file, _| taken[file] += 1);
    taken
}

/// The chosen entries of each file of these entries, counted from the file's own first entry.
pub fn per_file(entries: &[u64], n: u64, seed: u64) -> Vec<Vec<u64>> {
    let mut taken = vec![Vec::new(); entries.len()];
    each_file(entries, n, seed, |file, entry| taken[file].push(entry));
    taken
}

/// Every chosen entry as (its file, its entry in that file).
fn each_file(entries: &[u64], n: u64, seed: u64, mut each: impl FnMut(usize, u64)) {
    let (mut file, mut start) = (0, 0);
    chosen(entries.iter().sum(), n, seed, |i| {
        while i >= start + entries[file] {
            start += entries[file];
            file += 1;
        }
        each(file, i - start);
    });
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_exactly_n_ascending_and_the_same_for_the_same_seed() {
        let mut seen = Vec::new();
        chosen(1000, 10, 3, |i| seen.push(i));
        assert_eq!(seen.len(), 10);
        assert!(seen.windows(2).all(|w| w[0] < w[1]) && *seen.last().unwrap() < 1000);
        let files = per_file(&[5, 0, 7], 6, 3);
        assert_eq!(files.iter().map(Vec::len).sum::<usize>(), 6);
        assert!(files[1].is_empty() && files[0].iter().all(|e| *e < 5));
        assert_eq!(files, per_file(&[5, 0, 7], 6, 3));
        assert_eq!(per_file(&[3, 2], 9, 1), [vec![0, 1, 2], vec![0, 1]]);
        assert_eq!(
            counts(&[5, 0, 7], 6, 3),
            files.iter().map(|f| f.len() as u64).collect::<Vec<_>>()
        );
        // Pinned: a change here changes which events every random press of every file holds.
        assert_eq!(per_file(&[10, 10], 4, 7), [vec![1, 8], vec![0, 7]]);
        for events in [
            Events::All,
            Events::First(3),
            Events::Random {
                n: 3,
                seed: u64::MAX,
            },
        ] {
            assert_eq!(Events::read(&events.record()).unwrap(), events);
        }
        let mut other = Events::Random { n: 3, seed: 1 }.record();
        other["algorithm"] = "another".into();
        assert!(Events::read(&other).is_err());
        assert!(Events::read(&json!({"choose": "all", "n": 3})).is_err());
    }
}
