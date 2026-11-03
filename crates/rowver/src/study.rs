//! The study a `.rvr` file carries, as text a person or rvr wrote: its `samples.toml` (what it
//! reads, docs/SPEC.md §3) and its `samples.lock` (every file pinned, docs/FORMAT.md §6.3). Each is
//! read on rails, and the two are checked against each other.

pub mod lock;
pub mod samples;

/// Every problem a validator found in its input, each said so that its author can fix it, and the
/// lines that mend what was left out where the mending is mechanical (a table or a header line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Problems {
    /// Each problem, in a sentence.
    pub said: Vec<String>,
    /// The lines to write in for what was left out.
    pub missing: Vec<&'static str>,
}

impl Problems {
    /// One problem.
    pub fn one(said: impl Into<String>) -> Self {
        Problems { said: vec![said.into()], missing: Vec::new() }
    }

    /// One more problem, unless it was said already (as by each era of one sample).
    fn say(&mut self, said: impl Into<String>) {
        let said = said.into();
        if !self.said.contains(&said) {
            self.said.push(said);
        }
    }

    /// `Ok(value)` when nothing was found, else the problems.
    fn or<T>(self, value: T) -> Result<T, Problems> {
        match self.said.is_empty() && self.missing.is_empty() {
            true => Ok(value),
            false => Err(self),
        }
    }
}

impl std::fmt::Display for Problems {
    /// One problem a line, what is missing first.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let missing = (!self.missing.is_empty()).then(|| {
            format!("it leaves out {}: write {} in", self.missing.join(", "), match self.missing.len() {
                1 => "it",
                _ => "them",
            })
        });
        let lines: Vec<&str> = missing.iter().map(String::as_str).chain(self.said.iter().map(String::as_str)).collect();
        f.write_str(&lines.join("\n"))
    }
}

/// The few `names` nearest to `asked`, when they are near enough to be what was meant: within a
/// tenth of its length in edits (at least 2, a slip of a letter or two); at most 3, nearest first.
pub fn nearest<'a>(asked: &str, names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let limit = (asked.chars().count() / 10).max(2);
    let mut ranked: Vec<(usize, &str)> = names
        .map(|name| (edits(asked, name), name))
        .filter(|(distance, _)| *distance <= limit)
        .collect();
    ranked.sort();
    ranked.into_iter().take(3).map(|(_, n)| n).collect()
}

/// " Did you mean: …?" with the `names` nearest to `asked`, or nothing.
pub fn meant<'a>(asked: &str, names: impl Iterator<Item = &'a str>) -> String {
    let near = nearest(asked, names);
    match near.is_empty() {
        true => String::new(),
        false => format!(" Did you mean: {}?", near.join(", ")),
    }
}

/// Levenshtein distance, by characters.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != *cb)).min(row[j] + 1).min(here + 1);
            previous = here;
        }
    }
    row[b.len()]
}

/// "`place` has no key `key`", with what it takes, and the known key a slip of a letter or two away.
fn unknown(place: &str, key: &str, known: &[&str]) -> String {
    format!("{place} has no key {key:?}; it takes {}.{}", known.join(", "), meant(key, known.iter().copied()))
}

/// A local path with its `.` and `..` taken out by its text alone (no link is followed), so that
/// one file written two ways is told to be one.
pub fn plain(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => drop(out.pop()),
            part => out.push(part),
        }
    }
    out
}

/// Whether `text` is `YYYY-MM-DDTHH:MM:SSZ`, a real date and time.
pub fn utc(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| text[r].bytes().all(|c| c.is_ascii_digit());
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return false;
    }
    if ![0..4, 5..7, 8..10, 11..13, 14..16, 17..19].into_iter().all(digits) {
        return false;
    }
    let n = |r: std::ops::Range<usize>| text[r].parse::<u32>().unwrap_or(99);
    let (year, month, day) = (n(0..4), n(5..7), n(8..10));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day) && n(11..13) < 24 && n(14..16) < 60 && n(17..19) < 60
}

/// Whether `text` is `count` lowercase hexadecimal digits.
pub fn hex(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_names_and_times_and_hex() {
        let names = ["choose", "tree", "trees", "files", "events"];
        assert_eq!(nearest("choice", names.into_iter()), ["choose"]);
        assert_eq!(nearest("tress", names.into_iter()), ["trees", "tree"]);
        let files = ["ST_s-channel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20UL18_106X.root", "TTJets.root"];
        let typed = "ST_s-chanel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20UL18_106X.root";
        assert_eq!(nearest(typed, files.into_iter()), [files[0]]);
        assert!(nearest("completely_different_name.root", files.into_iter()).is_empty());
        let keys = ["name", "label", "files", "tree", "trees", "eras", "events", "choose", "seed", "metadata"];
        for slip in ["kind", "xsec", "lumi"] {
            assert!(nearest(slip, keys.into_iter()).is_empty(), "{slip}");
        }
        assert!(utc("2026-02-28T23:59:59Z") && utc("2024-02-29T00:00:00Z"));
        assert!(!utc("2026-02-29T00:00:00Z") && !utc("2026-13-01T00:00:00Z") && !utc("2026-01-01 00:00:00"));
        assert!(hex("0a1b2c3d", 8) && !hex("0A1B2C3D", 8) && !hex("0a1b2c3", 8));
        let p = |text: &str| plain(std::path::Path::new(text));
        assert_eq!(p("./d/x.root"), p("d/x.root"));
        assert_eq!(p("d/../e/./x.root"), p("e/x.root"));
        assert_eq!(p("../x.root"), std::path::PathBuf::from("../x.root"));
    }
}
