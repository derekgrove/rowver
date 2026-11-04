//! The options validator (docs/SPEC.md §5.1): every command's words and flags read one way, from
//! a table of what each command takes. Wrong use is refused before the command does anything.

use rowver::rvr::HOLE_KINDS;

/// What follows a flag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Takes {
    /// Nothing: it stands alone.
    No,
    /// Its value, the next argument.
    Value,
    /// `--fill`'s KINDS: the next argument, when every comma-separated part of it is a kind of hole.
    Kinds,
}

/// What a command takes.
pub struct Command {
    /// Its name as written after `rvr`, as `samples plan`.
    pub name: &'static str,
    /// Its flags: each with what follows it, and its short alias (`-f` for `--force`), if any.
    pub flags: &'static [(&'static str, Takes, Option<&'static str>)],
    /// The fewest and the most words it takes, and what they are, as its usage writes them.
    pub words: (usize, usize, &'static str),
    /// Whether a word that reads as a short flag (`-x`) is a word all the same (an expression).
    pub dashes: bool,
    /// Its usage, printed with a refusal of wrong use.
    pub usage: &'static str,
}

/// A command's arguments, read.
#[derive(Debug, Default)]
pub struct Options {
    /// Its words, in order.
    pub words: Vec<String>,
    /// Its flags, in the order given, each with its value.
    pub flags: Vec<(&'static str, Option<String>)>,
}

impl Options {
    /// The value given to `flag`.
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.flags.iter().find(|(f, _)| *f == flag).and_then(|(_, v)| v.as_deref())
    }

    /// Whether `flag` was given.
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|(f, _)| *f == flag)
    }

    /// The words, borrowed.
    pub fn plain(&self) -> Vec<&str> {
        self.words.iter().map(String::as_str).collect()
    }
}

/// Whether a word reads as a short flag, as `-y`: a dash, then letters alone.
fn short(word: &str) -> bool {
    word.strip_prefix('-').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic()))
}

/// Whether `word` names kinds of hole: every comma-separated part one.
pub fn kinds(word: &str) -> bool {
    word.split(',').all(|kind| HOLE_KINDS.contains(&kind))
}

/// Why the word after `--fill`, which is not kinds, is refused, when it reads as kinds meant: it
/// holds a comma, or a part of it is a slip from a kind, and it is no folder there.
fn slipped(word: &str) -> Option<String> {
    let near = |part: &str| rvr_recipe::nearest(part, HOLE_KINDS.iter().copied()).first().map(|kind| kind.to_string());
    let meant: Vec<Option<String>> = word.split(',').map(near).collect();
    let slip = word.split(',').zip(&meant).any(|(part, kind)| kind.as_deref().is_some_and(|kind| kind != part));
    if !(word.contains(',') || slip) || std::path::Path::new(word).is_dir() {
        return None;
    }
    let wrong: Vec<String> = word.split(',').filter(|part| !HOLE_KINDS.contains(part)).map(|part| format!("{part:?}")).collect();
    let all: Option<Vec<String>> = meant.into_iter().collect();
    let hint = all.map_or(String::new(), |all| format!(" (did you mean --fill {}?)", all.join(",")));
    Some(format!("--fill takes kinds of hole from {}, not {}{hint}, and {word} is not a study folder", HOLE_KINDS.join(", "), wrong.join(" or ")))
}

/// Read `args` as `command` takes them, or say how they are wrong (wrong use, SPEC §5.1.3).
pub fn read(command: &Command, args: &[&str]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.iter().copied().peekable();
    let names: Vec<&str> = command.flags.iter().map(|(name, _, _)| *name).collect();
    while let Some(word) = args.next() {
        // A short flag is its long one (SPEC §5.1.2): the one flag, however it is written.
        let Some(&(name, takes, short)) = command.flags.iter().find(|(name, _, short)| *name == word || *short == Some(word)) else {
            if word.starts_with("--") || (short(word) && !command.dashes) {
                let (flag, value) = word.split_once('=').unwrap_or((word, ""));
                if !value.is_empty() && names.contains(&flag) {
                    return Err(format!("{word} is not read: write {flag} {value}, the value after the flag"));
                }
                return Err(match names.is_empty() {
                    true => format!("{word} is not one of its flags: it takes none"),
                    false => format!("{word} is not one of its flags.{}", rvr_recipe::meant(word, names.iter().copied())),
                });
            }
            options.words.push(word.to_owned());
            continue;
        };
        if options.has(name) && name != "--drop" {
            let written = short.map_or(name.to_owned(), |short| format!("{name} (or {short})"));
            return Err(format!("{written} is given twice: give it once"));
        }
        let value = match takes {
            Takes::No => None,
            Takes::Value => match args.next().filter(|value| !value.starts_with("--")) {
                Some(value) => Some(value.to_owned()),
                None => return Err(format!("{name} needs a value, the argument after it")),
            },
            Takes::Kinds => match args.peek() {
                Some(next) if kinds(next) => args.next().map(str::to_owned),
                Some(next) if !next.starts_with("--") => match slipped(next) {
                    Some(why) => return Err(why),
                    None => None,
                },
                _ => None,
            },
        };
        options.flags.push((name, value));
    }
    let (least, most, what) = command.words;
    let given = options.words.len();
    if given < least || given > most {
        let said = match options.words.is_empty() {
            true => "none was given".to_owned(),
            false => format!("it was given {}", options.words.iter().map(|w| format!("{w:?}")).collect::<Vec<_>>().join(", ")),
        };
        return Err(format!("{} takes {what}, and {said}", command.name));
    }
    Ok(options)
}

/// A whole number written in digits with `_` allowed between two of them (`50_000`).
fn whole(text: &str) -> Option<u64> {
    let digits = text.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    let between = !text.starts_with('_') && !text.ends_with('_') && !text.contains("__");
    (digits && between && !text.is_empty()).then(|| text.replace('_', "").parse().ok()).flatten()
}

/// A count given as `flag`'s value: a whole number from 1 to `most` (SPEC §5.1.4).
pub fn count(flag: &str, text: &str, most: u64) -> Result<u64, String> {
    let range = match most {
        u64::MAX => "from 1".to_owned(),
        most => format!("from 1 to {most}"),
    };
    whole(text).filter(|n| (1..=most).contains(n)).ok_or_else(|| {
        format!("{flag} is a whole number {range}, written in digits (with _ between them if you like, as 50_000), not {text:?}")
    })
}

/// `--seed S`: a whole number from 0 to 2^64 − 1.
pub fn seed(text: &str) -> Result<u64, String> {
    whole(text).ok_or_else(|| format!("--seed is a whole number from 0 to 2^64 − 1, written in digits, not {text:?}"))
}

/// `--events N|all`: every event, or the first N, from 1 to 2^63 − 1.
pub fn events(text: &str) -> Result<rowver::choice::Events, String> {
    match text {
        "all" => Ok(rowver::choice::Events::All),
        n => count("--events", n, i64::MAX as u64).map(rowver::choice::Events::First).map_err(|_| format!("--events is a number of events from 1 to 2^63 − 1, or all, not {n:?} (in digits, with _ between them if you like, as 50_000)")),
    }
}

/// `--workers N`, or the number of cores (1 when the system says none).
pub fn workers(options: &Options) -> Result<usize, String> {
    match options.value("--workers") {
        Some(n) => count("--workers", n, u64::MAX).map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        None => Ok(rowver::parallel::cores().max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESS: Command = Command {
        name: "press",
        flags: &[("--events", Takes::Value, None), ("--random", Takes::No, None), ("--fill", Takes::Kinds, None), ("--drop", Takes::No, None)],
        words: (0, 1, "at most one study folder"),
        dashes: false,
        usage: "",
    };

    // SPEC 5.1.2, 5.1.3, 5.1.4, 7.1.2
    #[test]
    fn flags_are_read_one_way_and_wrong_use_is_said() {
        let read = |args: &[&str]| super::read(&PRESS, args);
        let options = read(&["study", "--events", "10", "--fill", "gone,damaged"]).unwrap();
        assert_eq!((options.plain(), options.value("--events"), options.value("--fill")), (vec!["study"], Some("10"), Some("gone,damaged")));
        // The word after --fill is KINDS only when every part is a kind, whatever the folders are.
        assert_eq!(read(&["--fill", "gone"]).unwrap().value("--fill"), Some("gone"));
        let folder = read(&["--fill", "study"]).unwrap();
        assert_eq!((folder.value("--fill"), folder.plain()), (None, vec!["study"]));
        // One that reads as kinds meant, and is no folder, is refused wherever it stands.
        for args in [&["--fill", "gon"][..], &["--fill", "gone,damage", "study"]] {
            assert!(read(args).unwrap_err().contains("did you mean --fill gone"), "{args:?}");
        }
        assert!(read(&["--evnts", "1"]).unwrap_err().contains("Did you mean: --events?"));
        assert!(read(&["--events=1"]).unwrap_err().contains("write --events 1"));
        assert!(read(&["--events"]).unwrap_err().contains("needs a value"));
        assert!(read(&["--events", "--random"]).unwrap_err().contains("needs a value"));
        assert!(read(&["--random", "--random"]).unwrap_err().contains("given twice"));
        assert!(read(&["--drop", "--drop"]).is_ok(), "--drop may be given again");
        let empty = Command { name: "empty", flags: &[("--force", Takes::No, Some("-f"))], words: (1, 9, "files"), dashes: false, usage: "" };
        assert!(super::read(&empty, &["a", "-f"]).unwrap().has("--force"));
        assert!(super::read(&empty, &["a", "-f", "--force"]).unwrap_err().contains("--force (or -f) is given twice"));
        assert!(read(&["a", "b"]).unwrap_err().contains("takes at most one study folder"));
        assert!(read(&["-y"]).unwrap_err().contains("not one of its flags"));
        assert_eq!(count("--events", "50_000", 100_000), Ok(50_000));
        for wrong in ["0", "_5", "5_", "5__0", "-1", "1e3", ""] {
            assert!(count("--events", wrong, 100_000).is_err(), "{wrong}");
        }
        assert_eq!(seed("18_446_744_073_709_551_615"), Ok(u64::MAX));
        assert!(seed("18446744073709551616").is_err() && seed("0").is_ok());
    }
}
