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

pub(crate) use rvr_recipe::unknown;

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

/// Whether `name` matches `pattern`, where `*` is any run of characters and `?` one character.
pub fn matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            (pi, ni) = (pi + 1, ni + 1);
        } else if pi < p.len() && p[pi] == '*' {
            (star, mark, pi) = (Some(pi), ni, pi + 1);
        } else if let Some(s) = star {
            (pi, mark) = (s + 1, mark + 1);
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Whether `text` is `count` lowercase hexadecimal digits.
pub fn hex(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_hex() {
        assert!(utc("2026-02-28T23:59:59Z") && utc("2024-02-29T00:00:00Z"));
        assert!(!utc("2026-02-29T00:00:00Z") && !utc("2026-13-01T00:00:00Z") && !utc("2026-01-01 00:00:00"));
        assert!(hex("0a1b2c3d", 8) && !hex("0A1B2C3D", 8) && !hex("0a1b2c3", 8));
        let p = |text: &str| plain(std::path::Path::new(text));
        assert_eq!(p("./d/x.root"), p("d/x.root"));
        assert_eq!(p("d/../e/./x.root"), p("e/x.root"));
        assert_eq!(p("../x.root"), std::path::PathBuf::from("../x.root"));
    }

    #[test]
    fn patterns_and_paths_parted_and_joined_back_as_written() {
        assert!(matches("Run2012*_TauPlusX.root", "Run2012B_TauPlusX.root") && matches("*.root", "a.root"));
        assert!(!matches("Run2012?_Tau*.root", "Run2012BC_DoubleMuParked_Muons.root"));
        assert!(matches("SMS-Signal*_Summer22_130X.root", "SMS-Signal-2L_TuneCP5_13p6TeV_madgraphMLM-pythia8_Summer22_130X.root"));
        assert!(matches("a?c*", "abc") && !matches("a?c", "ac") && matches("*", ""));
        for asked in ["root://eospublic.cern.ch//x.root", "root://h//eos/a/x.root", "/x.root", "./x.root", "~/d/x.root", "d//x.root"] {
            let (folder, name) = split(asked);
            assert_eq!(join(folder, name), asked);
        }
        assert_eq!(join(split("x.root").0, "x.root"), "./x.root");
    }
}
