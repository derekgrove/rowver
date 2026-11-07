//! `rvr`: start a study, resolve what it reads, press it, and validate, dump, inspect and diff
//! `.rvr` files. The command's whole work is `run`, which the Python package calls too.

mod agree;
mod branches;
mod core;
mod derive;
mod diff;
mod dump;
mod empty;
mod files;
mod fill;
mod gather;
mod held;
mod holes;
mod journal;
mod key;
mod new;
mod open;
mod options;
mod plan;
mod press;
mod progress;
mod proxy;
mod reproduce;
mod reuse;
mod rowdiff;
mod samples;
mod status;
mod tables;
mod take;
mod texts;
mod tool;
mod trying;
mod written;

use options::{Command, Options, Takes::{Kinds, No, Value}};
use std::path::Path;

/// The recipe a press runs, read from its text, or what is wrong with it.
pub use texts::read_recipe;

const USAGE: &str = "usage:
  rvr new DIR                              start a study: samples.toml, recipe.toml, .gitignore
  rvr samples expand|resolve|check|plan ...
                                           what a study reads (rvr samples alone lists them)
  rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N] [--full]
            [--fill [KINDS]] [--restart]
                                           press the study in DIR (.) into DIR/<its name>.rvr;
                                           --fill reads only its holes again
  rvr status [DIR] [--out FILE] [--events N|all] [--random] [--seed S] [--holes]
                                           is the study's .rvr current and whole, and what would
                                           press read
  rvr branches [DIR] [--sample NAME] [--find TEXT]
                                           the branches a sample's file holds
  rvr try \"EXPRESSION\" [DIR] [--events N] [--sample NAME] [--kept]
                                           what an expression gives on a sample's first events
  rvr derive FILE 'NAME = EXPRESSION'... | --drop NAME... | --rename OLD NEW
                                           add, drop or rename derived columns of a pressed file
  rvr validate FILE... [--complete]        check files, their derived columns worked out too;
                                           --complete fails a file with holes, or an empty one
  rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns] [--all]
                                           the header in full, the study's texts, every column
                                           (--all: every value, one row a line)
  rvr header FILE                          the JSON header only, as the file holds it
  rvr diff A B                             which column of which sample differs, from the headers
  rvr diff A B --rows [--key COLUMN,…] [--sample NAME]
                                           event by event: the events only one holds, the first
                                           value that differs (matched by file and entry, or --key)
  rvr reproduce FILE [DIR] [--study FOLDER] [--workers N] [--texts]
                                           press FILE's own study again and diff the two;
                                           --texts writes its three texts into DIR, pressing none
  rvr empty FILE... [--study FOLDER] [--anyway] [--force | -f]
                                           let a file's columns go, keeping its header to refill
  rvr refill FILE [--study FOLDER] [--workers N]
                                           press an empty file's columns again from its study
  rvr tool FILE [KEY --set DATA.json | KEY --get [OUT.json] | KEY --drop] [--force | -f]
                                           a file's tool keys: listed, or one set, written out or
                                           dropped, in the file itself
  rvr key new [NAME]                       make the key that signs your files
  rvr key show                             your public key
  rvr key trust KEY NAME...                trust files signed by KEY, as NAME's
  rvr proxy                                your grid proxy's state
  rvr proxy init [--hours H]               make a cms proxy from ~/.globus (H: 1 to 720, 192)
  rvr --version                            this rvr's version, and the commit it was built from
  rvr, rvr help, rvr --help                this usage";

/// The usage of the commands that keep none of their own.
const NEW: &str = "usage:\n  rvr new DIR    start a study in DIR: samples.toml, recipe.toml and .gitignore";
const VALIDATE: &str = "usage:\n  rvr validate FILE... [--complete]    check files in full; --complete fails a file with holes, or an empty one";
const DUMP: &str = "usage:\n  rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns] [--all]\n      the header in full, the study's texts and every column, or only the parts named, in the order\n      given; --all prints every value of every column, one row a line (with --columns, or alone)";
const HEADER: &str = "usage:\n  rvr header FILE    the header's bytes, as the file holds them";
const DIFF: &str = "usage:\n  rvr diff A B                                     where two files differ, from their headers alone\n  rvr diff A B --rows [--key COLUMN,…] [--sample NAME]\n      event by event: the events only one holds, and the first value that differs";

/// Every command: its name after `rvr`, its flags, its words.
const COMMANDS: &[Command] = &[
    Command { name: "new", flags: &[], words: (1, 1, "one folder, DIR"), dashes: false, usage: NEW },
    Command { name: "samples expand", flags: &[("--json", No, None)], words: (1, 1, "one samples.toml"), dashes: false, usage: samples::USAGE },
    Command { name: "samples resolve", flags: &[], words: (1, 1, "one samples.toml"), dashes: false, usage: samples::USAGE },
    Command { name: "samples check", flags: &[], words: (1, 1, "one samples.lock"), dashes: false, usage: samples::USAGE },
    Command { name: "samples plan", flags: &[("--events", Value, None), ("--random", No, None), ("--seed", Value, None)], words: (1, 1, "one samples.lock"), dashes: false, usage: samples::USAGE },
    Command { name: "branches", flags: &[("--sample", Value, None), ("--find", Value, None)], words: (0, 1, "at most one study folder"), dashes: false, usage: branches::USAGE },
    Command { name: "try", flags: &[("--events", Value, None), ("--sample", Value, None), ("--kept", No, None)], words: (1, 2, "one expression and at most one study folder"), dashes: true, usage: trying::USAGE },
    Command { name: "press", flags: &[("--events", Value, None), ("--random", No, None), ("--seed", Value, None), ("--out", Value, None), ("--workers", Value, None), ("--full", No, None), ("--fill", Kinds, None), ("--restart", No, None)], words: (0, 1, "at most one study folder"), dashes: false, usage: press::USAGE },
    Command { name: "status", flags: &[("--out", Value, None), ("--events", Value, None), ("--random", No, None), ("--seed", Value, None), ("--holes", No, None)], words: (0, 1, "at most one study folder"), dashes: false, usage: status::USAGE },
    Command { name: "derive", flags: &[("--drop", No, None), ("--rename", No, None)], words: (2, usize::MAX, "a file and what to change of it"), dashes: false, usage: derive::USAGE },
    Command { name: "validate", flags: &[("--complete", No, None)], words: (1, usize::MAX, "the files to check"), dashes: false, usage: VALIDATE },
    Command { name: "dump", flags: &[("--files", No, None), ("--recipe", No, None), ("--samples", No, None), ("--lock", No, None), ("--columns", No, None), ("--all", No, None)], words: (1, 1, "one file"), dashes: false, usage: DUMP },
    Command { name: "header", flags: &[], words: (1, 1, "one file"), dashes: false, usage: HEADER },
    Command { name: "diff", flags: &[("--rows", No, None), ("--key", Value, None), ("--sample", Value, None)], words: (2, 2, "two files, A and B"), dashes: false, usage: DIFF },
    Command { name: "reproduce", flags: &[("--study", Value, None), ("--workers", Value, None), ("--texts", No, None)], words: (1, 2, "a file and at most one folder"), dashes: false, usage: reproduce::USAGE },
    Command { name: "empty", flags: &[("--study", Value, None), ("--anyway", No, None), ("--force", No, Some("-f"))], words: (1, usize::MAX, "the files to empty"), dashes: false, usage: empty::USAGE },
    Command { name: "refill", flags: &[("--study", Value, None), ("--workers", Value, None)], words: (1, 1, "one file"), dashes: false, usage: empty::USAGE },
    Command { name: "tool", flags: &[("--set", Value, None), ("--get", No, None), ("--drop", No, None), ("--force", No, Some("-f"))], words: (1, 3, "a file, then a tool key and what to do with it"), dashes: true, usage: tool::USAGE },
    Command { name: "key new", flags: &[], words: (0, 1, "at most one name"), dashes: false, usage: key::USAGE },
    Command { name: "key show", flags: &[], words: (0, 0, "no words"), dashes: false, usage: key::USAGE },
    Command { name: "key trust", flags: &[], words: (2, usize::MAX, "a key and its owner's name"), dashes: false, usage: key::USAGE },
    Command { name: "proxy", flags: &[], words: (0, 0, "no words but init"), dashes: false, usage: proxy::USAGE },
    Command { name: "proxy init", flags: &[("--hours", Value, None)], words: (0, 0, "no words"), dashes: false, usage: proxy::USAGE },
    Command { name: "help", flags: &[], words: (0, 0, "no words"), dashes: false, usage: USAGE },
];

/// Each command's name and the words it takes, for the tests.
#[doc(hidden)]
pub fn commands() -> impl Iterator<Item = (&'static str, &'static str)> {
    COMMANDS.iter().map(|c| (c.name, c.words.2))
}

/// The commands whose next word names what they do, and those words.
const SUBCOMMANDS: [(&str, &[&str]); 3] = [("samples", &["expand", "resolve", "check", "plan"]), ("key", &["new", "show", "trust"]), ("proxy", &["init"])];

/// Make a closed output pipe end the process with 141, as SIGPIPE does (`rvr dump … | head`).
pub fn quiet_on_closed_pipe() {
    let usual = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let said = info.payload().downcast_ref::<String>().map(String::as_str).unwrap_or("");
        if said.starts_with("failed printing to stdout") && said.contains("Broken pipe") {
            std::process::exit(CLOSED);
        }
        usual(info);
    }));
}

/// The exit status of a command whose output pipe closed before it finished (SPEC §5.1.6).
pub(crate) const CLOSED: i32 = 141;

/// Write `bytes` to standard output as they are, ending the process with 141 if its pipe closed;
/// any other failure to write them is said as `command`'s.
pub(crate) fn out(command: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(bytes).and_then(|()| stdout.flush()).map_err(|e| match e.kind() {
        std::io::ErrorKind::BrokenPipe => std::process::exit(CLOSED),
        _ => format!("rvr {command}: standard output could not be written ({}): free space where it goes, or send it elsewhere", rowver::described(&e)),
    })
}

/// Wrong use of `command`: what is wrong, its usage, and the exit status 2.
pub(crate) fn wrong(command: &str, why: &str) -> u8 {
    let usage = COMMANDS.iter().find(|c| c.name == command).map_or(USAGE, |c| c.usage);
    eprintln!("rvr {command}: {why}\n{usage}");
    2
}

/// Run the command these words (the arguments after `rvr`) name; its exit status.
pub fn run(args: &[std::ffi::OsString]) -> u8 {
    let mut words = Vec::new();
    for arg in args {
        match arg.to_str() {
            Some(word) => words.push(word),
            None => {
                eprintln!("rvr: the argument {:?} is not UTF-8 text, and rvr reads only UTF-8: give it in UTF-8 (rename a file whose name is not)", arg.to_string_lossy());
                return 2;
            }
        }
    }
    match read_words(&words) {
        Ok((command, options)) => command_run(command.name, &options),
        Err(status) => status,
    }
}

/// The command these words name, with its options read; or, when they name none rightly, the exit
/// status of saying so (or of `rvr` alone).
fn read_words(args: &[&str]) -> Result<(&'static Command, Options), u8> {
    let mut tops: Vec<&str> = COMMANDS.iter().map(|c| c.name.split(' ').next().unwrap_or(c.name)).collect();
    tops.dedup();
    // The command is the first word that is not a flag (flags before it, with their values, are
    // passed over); for samples, key and proxy, the next such word names what it does.
    let Some(at) = first_word(args) else {
        return Err(bare(args));
    };
    if !tops.contains(&args[at]) {
        eprintln!("rvr: {} is not a command.{}\n{USAGE}", args[at], rvr_recipe::meant(args[at], tops.iter().copied()));
        return Err(2);
    }
    let mut rest: Vec<&str> = args.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, a)| *a).collect();
    let mut name = args[at].to_owned();
    if let Some((top, subs)) = SUBCOMMANDS.iter().find(|(top, _)| *top == args[at]) {
        let usage = COMMANDS.iter().find(|c| c.name.starts_with(top)).map_or(USAGE, |c| c.usage);
        match first_word(&rest) {
            Some(sub) if subs.contains(&rest[sub]) => name = format!("{top} {}", rest.remove(sub)),
            _ if *top == "proxy" => {}
            Some(sub) => {
                eprintln!("rvr {top}: {} is not one of its commands, {}.{}\n{usage}", rest[sub], subs.join(", "), rvr_recipe::meant(rest[sub], subs.iter().copied()));
                return Err(2);
            }
            None => {
                eprintln!("rvr {top}: give one of {}.\n{usage}", subs.join(", "));
                return Err(2);
            }
        }
    }
    let Some(command) = COMMANDS.iter().find(|c| c.name == name) else {
        return Err(wrong(&name, "it is not a command"));
    };
    options::read(command, &rest).map(|options| (command, options)).map_err(|why| wrong(command.name, &why))
}

/// What the fuzz harness (fuzz/) calls, built only by cargo fuzz (`--cfg fuzzing`).
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzzing {
    /// Read `args` as `rvr` reads its arguments, running no command.
    pub fn words(args: &[&str]) {
        let _ = super::read_words(args);
    }

    /// Read `answer` as rvr reads a server's answer of this content type to a request for `asked` bytes.
    pub fn multipart(content_type: &str, answer: &[u8], asked: u64) {
        if let Some(boundary) = crate::samples::multipart::boundary(content_type) {
            let _ = crate::samples::multipart::parts(answer, &boundary, asked);
        }
    }
}

/// Where the first word of `args` that is not a flag is, the flags before it passed over with
/// the values any command's flag of that name takes.
fn first_word(args: &[&str]) -> Option<usize> {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        if !word.starts_with('-') {
            return Some(at);
        }
        let takes = COMMANDS.iter().flat_map(|c| c.flags).filter(|(flag, _, short)| flag == word || *short == Some(*word)).map(|(_, takes, _)| *takes).next();
        at += match takes {
            Some(Value) => 2,
            Some(Kinds) if args.get(at + 1).is_some_and(|next| options::kinds(next)) => 2,
            _ => 1,
        };
    }
    None
}

/// `rvr` with no command: its usage, its version, or wrong use.
fn bare(args: &[&str]) -> u8 {
    match args {
        [] | ["--help"] => {
            println!("{USAGE}\n\n{}", samples::USAGE);
            0
        }
        ["--version"] => {
            println!("rvr {} (reads and writes rowver {}; commit {})", env!("CARGO_PKG_VERSION"), rowver::VERSION, env!("RVR_COMMIT"));
            0
        }
        _ => {
            let own = ["--version", "--help"];
            let why = match args.iter().find(|flag| !own.contains(flag)) {
                Some(flag) => format!("{flag} is not a flag of rvr alone, which takes --version or --help: give the command it is for"),
                None if args[1..].contains(&args[0]) => format!("{} is given twice: give it once", args[0]),
                None => "rvr alone takes --version or --help, not both: give one".to_owned(),
            };
            eprintln!("rvr: {why}\n{USAGE}");
            2
        }
    }
}

/// The words of the command `name` that name files it opens: each put back first when a change to
/// it was cut short (SPEC §8.12). validate and empty put back each of theirs as they come to it, so
/// that one that cannot be is one failure among the rest.
fn opened<'a>(name: &str, words: &[&'a str]) -> Vec<&'a str> {
    match name {
        "diff" => words.iter().take(2).copied().collect(),
        "derive" | "dump" | "header" | "reproduce" | "refill" | "tool" => words.iter().take(1).copied().collect(),
        _ => Vec::new(),
    }
}

/// Put back the file at `file` as it was when a change to it was cut short, saying so on standard
/// error; what failed, when it could not be.
pub(crate) fn restored(file: &Path) -> Result<(), String> {
    rowver::change::restore(file).map(|restored| told(file, restored)).map_err(|e| rowver::plain(file, &e))
}

/// The file at `file` locked for this one writer (SPEC §8.12), put back first when a change to it
/// was cut short, saying so on standard error; what failed, when it could not be.
pub(crate) fn locked(file: &Path) -> Result<rowver::change::Locked, String> {
    let locked = rowver::change::Locked::take(file).map_err(|e| rowver::plain(file, &e))?;
    told(file, locked.restored());
    Ok(locked)
}

/// Say on standard error what became of a change to `file` cut short (SPEC §8.12): of the file
/// it names, through a link, which the journal lies beside.
fn told(file: &Path, restored: rowver::change::Restored) {
    let real = rowver::change::resolved(file);
    let shown = real.display();
    match restored {
        rowver::change::Restored::PutBack => eprintln!("Put {shown} back as it was: a change to it was cut short."),
        rowver::change::Restored::PutBackWithoutLock => eprintln!("Put {shown} back as it was: a change to it was cut short (put back without a lock: this filesystem gives none)."),
        rowver::change::Restored::LetGo => eprintln!("Let go {shown}-journal: a change to {shown} was cut short before {shown} was touched, so {shown} is as it was."),
        rowver::change::Restored::Finished => eprintln!("Let go {shown}-journal: the change to {shown} had finished, so {shown} is as the change made it."),
        rowver::change::Restored::NoFile => eprintln!("Let go {shown}-journal: {shown} is not there."),
        rowver::change::Restored::Nothing => {}
    }
}

/// Say that `file` was written anew and moved over, when a change could not be made in place
/// (SPEC §8.12).
pub(crate) fn anew(file: &Path, locked: &rowver::change::Locked) {
    if let Some(why) = locked.anew() {
        told_anew(file, &why, false);
    }
}

/// Say that `file` cannot be changed in place (`why`), so it was written anew and moved over, or,
/// as rvr status says it, `would` be.
pub(crate) fn told_anew(file: &Path, why: &str, would: bool) {
    let done = match would {
        true => "rvr press would write it anew and move it over",
        false => "wrote it anew and moved it over",
    };
    println!("{} cannot be changed in place ({why}): {done}.", file.display());
}

/// In a debug build, wait while the file `RVR_HOLD` names is there, saying so first: so that a
/// test starts another writer while this one holds the first of its two locks (SPEC §8.12).
pub(crate) fn hold() {
    #[cfg(debug_assertions)]
    if let Some(hold) = std::env::var_os("RVR_HOLD") {
        eprintln!("Holding the first of its locks, as RVR_HOLD asks.");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while Path::new(&hold).exists() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Run a command whose options are read.
fn command_run(name: &str, options: &Options) -> u8 {
    let words = options.plain();
    for file in opened(name, &words) {
        if let Err(why) = restored(Path::new(file)) {
            return report(name, Err(why));
        }
    }
    match name {
        "new" => report("new", new::main(words[0]).map(|()| 0)),
        "samples expand" | "samples resolve" | "samples check" | "samples plan" => samples::main(name, options),
        "branches" => branches::main(options),
        "try" => trying::main(options),
        "press" => press::main(options),
        "status" => status::main(options),
        "derive" => derive::main(options),
        "validate" => validate(&words, options.has("--complete")),
        "dump" => {
            let flags: Vec<&str> = options.flags.iter().map(|(flag, _)| *flag).filter(|flag| *flag != "--all").collect();
            if let Some(part) = flags.iter().find(|flag| **flag != "--columns").filter(|_| options.has("--all")) {
                return wrong("dump", &format!("--all prints every value of the columns, and {part} prints no column: give --all alone or with --columns"));
            }
            report("dump", dump::main(words[0], &flags, options.has("--all")).map(|()| 0))
        }
        "header" => header(words[0]),
        "diff" if options.has("--rows") => rowdiff::main(words[0], words[1], options.value("--key"), options.value("--sample")),
        "diff" if options.has("--key") || options.has("--sample") => wrong("diff", "--key and --sample compare events, which only rvr diff A B --rows does: add --rows"),
        "diff" => diff::main(words[0], words[1]),
        "reproduce" => reproduce::main(options),
        "empty" => empty::empty_main(options),
        "refill" => empty::refill_main(options),
        "tool" => tool::main(options),
        "key new" | "key show" | "key trust" => key::main(name, &words),
        "proxy" | "proxy init" => proxy::main(name, options),
        _ => {
            println!("{USAGE}\n\n{}", samples::USAGE);
            0
        }
    }
}

/// Whether someone is at a terminal to answer a question: stdin and stderr both are one.
pub(crate) fn at_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Ask `question`, which ends "[y/N] ", at the terminal: whether the answer was yes.
pub(crate) fn yes(question: &str) -> Result<bool, String> {
    eprint!("{question}");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| format!("the answer could not be read ({})", rowver::described(&e)))?;
    Ok(["y", "yes"].iter().any(|yes| answer.trim().eq_ignore_ascii_case(yes)))
}

/// The exit status `result` holds, or 1 with what failed printed as `rvr COMMAND: …`.
pub(crate) fn report(command: &str, result: Result<u8, String>) -> u8 {
    result.unwrap_or_else(|error| {
        eprintln!("rvr {command}: {error}");
        1
    })
}

/// Print a file's header as its bytes lie in the file; one that fails its checks is printed all
/// the same, and why is said on standard error (exit 1).
fn header(file: &str) -> u8 {
    let text = match rowver::rvr::header_text(file) {
        Ok(text) => text,
        Err(e) => return report("header", Err(rowver::plain(file, &e))),
    };
    if let Err(why) = out("header", &text) {
        eprintln!("{why}");
        return 1;
    }
    match rowver::rvr::Reader::inspect(file).map(|(_, checked)| checked) {
        Ok(Ok(())) => 0,
        Ok(Err(why)) => report("header", Err(format!("{file}: its header is printed as the file holds it, but it fails its checks: {}", rowver::words(Path::new(file), &why)))),
        Err(e) => report("header", Err(rowver::plain(file, &e))),
    }
}

/// One line per file: valid, or the first problem found. Every derived column is worked out and
/// checked against what the file records of it, unless the file is empty. With `--complete`, a
/// valid file with holes, or an empty one, fails too.
fn validate(words: &[&str], complete: bool) -> u8 {
    let mut ok = true;
    for file in words {
        let restored = rowver::change::restore(file).map(|restored| told(Path::new(file), restored));
        let checked = restored.and_then(|()| rowver::rvr::validate_file(file)).map_err(|e| match e {
            rowver::Error::Io(e) => rowver::plain_it(&e),
            e => rowver::words(Path::new(file), &e),
        });
        let compared = checked.map(|summary| {
            let derived = match (summary.derived, summary.empty) {
                (0, _) => String::new(),
                (n, true) => format!("{n} derived (worked out once refilled), "),
                (n, false) => format!("{n} derived, "),
            };
            let signed = match summary.header.get("signature") {
                Some(signature) => key::signer(signature["key"].as_str().unwrap_or_default()),
                None => "not signed".into(),
            };
            let read = match entries_read(&summary.header["made"]) {
                (1, 1) => "read its 1 entry, ".to_owned(),
                (read, total) if read == total => format!("read all {} entries, ", rowver::rvr::grouped(total)),
                (read, total) => format!("read {} of {}, ", rowver::rvr::grouped(read), samples::some(total as usize, "entry", "entries")),
            };
            let holes = rowver::rvr::holes(&summary.header);
            let empty = match summary.empty {
                true => "empty (rvr refill presses its columns again), ",
                false => "",
            };
            let said = derived + &read + &holed(&holes) + empty + &signed;
            (summary.rows, summary.columns, said, holes.is_empty(), summary.empty)
        });
        // With --complete, a file with holes, or an empty one, fails, on its one line.
        let compared = compared.and_then(|(rows, columns, said, whole, empty)| match complete && (!whole || empty) {
            true if empty => Err(format!("it is empty, and --complete asks for its columns ({said}): rvr refill presses them again")),
            true => Err(format!("it has holes, and --complete asks for none ({said}): rvr status lists them, rvr press --fill reads them again")),
            false => Ok((rows, columns, said)),
        });
        ok &= said(file, &compared);
    }
    u8::from(!ok)
}

/// The entries a press read and the entries its samples hold: the sums of their `events_read` and
/// `events_total`, which `validate_file` checked.
fn entries_read(made: &serde_json::Value) -> (u64, u64) {
    let samples = made["files"].as_object().into_iter().flatten().map(|(_, sample)| &sample["metadata"]);
    samples.fold((0, 0), |(read, total), metadata| {
        (read + metadata["events_read"].as_u64().unwrap_or(0), total + metadata["events_total"].as_u64().unwrap_or(0))
    })
}

/// How many holes, by kind: `1 hole (gone), ` or `4 holes (3 unreachable, 1 damaged), `; nothing for none.
fn holed(holes: &[rowver::rvr::Hole]) -> String {
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    for hole in holes {
        match kinds.iter_mut().find(|(kind, _)| *kind == hole.kind) {
            Some((_, n)) => *n += 1,
            None => kinds.push((&hole.kind, 1)),
        }
    }
    match kinds[..] {
        [] => String::new(),
        [(kind, _)] => format!("{} ({kind}), ", samples::some(holes.len(), "hole", "holes")),
        _ => {
            let each: Vec<String> = kinds.iter().map(|(kind, n)| format!("{n} {kind}")).collect();
            format!("{} holes ({}), ", holes.len(), each.join(", "))
        }
    }
}

/// Print a file's line of `rvr validate`; whether it is valid.
fn said(file: &str, verdict: &Result<(usize, usize, String), String>) -> bool {
    match verdict {
        Ok((rows, columns, summary)) => println!("{file}: valid, {}, {}, {summary}", samples::some(*rows, "row", "rows"), samples::some(*columns, "column", "columns")),
        Err(why) => println!("{file}: INVALID: {why}"),
    }
    verdict.is_ok()
}

#[cfg(test)]
mod tests {
    /// Every command and each of its flags is in its own usage and in what rvr --help prints.
    // SPEC 5.1.3
    #[test]
    fn every_command_and_flag_is_in_both_usages() {
        let help = format!("{}\n\n{}", super::USAGE, super::samples::USAGE);
        for command in super::COMMANDS {
            let named = format!("rvr {}", command.name);
            assert!(command.usage.contains(&named) && help.contains(&named), "{named}");
            for (flag, _, _) in command.flags {
                assert!(command.usage.contains(flag) && help.contains(flag), "{named} {flag}");
            }
        }
    }

    /// A short flag is in its command's usage, and in rvr's, beside its long one.
    // SPEC 5.1.2
    #[test]
    fn every_short_flag_is_in_the_usage_beside_its_long_one() {
        for command in super::COMMANDS {
            for (name, _, short) in command.flags {
                if let Some(short) = short {
                    let beside = format!("[{name} | {short}]");
                    assert!(command.usage.contains(&beside) && super::USAGE.contains(&beside), "{}: {beside}", command.name);
                }
            }
        }
    }
}
