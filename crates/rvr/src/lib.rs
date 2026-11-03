//! `rvr`: start a study, resolve what it reads, press it, and validate, dump, inspect and diff
//! `.rvr` files. The command's whole work is `run`, which the Python package calls too.

mod agree;
mod branches;
mod core;
mod derive;
mod diff;
mod dump;
mod empty;
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
mod samples;
mod status;
mod tables;
mod take;
mod texts;
mod trying;
mod written;

use options::{Command, Options, Takes::{Kinds, No, Value}};

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
  rvr reproduce FILE [DIR] [--study FOLDER] [--workers N]
                                           press FILE's own study again and diff the two
  rvr empty FILE... [--study FOLDER] [--anyway] [--force | -f]
                                           let a file's columns go, keeping its header to refill
  rvr refill FILE [--study FOLDER] [--workers N]
                                           press an empty file's columns again from its study
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
const DIFF: &str = "usage:\n  rvr diff A B    where two files differ, from their headers alone";

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
    Command { name: "diff", flags: &[], words: (2, 2, "two files, A and B"), dashes: false, usage: DIFF },
    Command { name: "reproduce", flags: &[("--study", Value, None), ("--workers", Value, None)], words: (1, 2, "a file and at most one folder"), dashes: false, usage: reproduce::USAGE },
    Command { name: "empty", flags: &[("--study", Value, None), ("--anyway", No, None), ("--force", No, Some("-f"))], words: (1, usize::MAX, "the files to empty"), dashes: false, usage: empty::USAGE },
    Command { name: "refill", flags: &[("--study", Value, None), ("--workers", Value, None)], words: (1, 1, "one file"), dashes: false, usage: empty::USAGE },
    Command { name: "key new", flags: &[], words: (0, 1, "at most one name"), dashes: false, usage: key::USAGE },
    Command { name: "key show", flags: &[], words: (0, 0, "no words"), dashes: false, usage: key::USAGE },
    Command { name: "key trust", flags: &[], words: (2, usize::MAX, "a key and its owner's name"), dashes: false, usage: key::USAGE },
    Command { name: "proxy", flags: &[], words: (0, 0, "no words but init"), dashes: false, usage: proxy::USAGE },
    Command { name: "proxy init", flags: &[("--hours", Value, None)], words: (0, 0, "no words"), dashes: false, usage: proxy::USAGE },
    Command { name: "help", flags: &[], words: (0, 0, "no words"), dashes: false, usage: USAGE },
];

/// The commands whose next word names what they do, and those words.
const SUBCOMMANDS: [(&str, &[&str]); 3] = [("samples", &["expand", "resolve", "check", "plan"]), ("key", &["new", "show", "trust"]), ("proxy", &["init"])];

/// Make a closed output pipe end the process as a C tool stopped by SIGPIPE does, with 141:
/// `rvr dump … | head` closes it early. Any other panic is reported as usual.
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
    let args = words;
    let mut tops: Vec<&str> = COMMANDS.iter().map(|c| c.name.split(' ').next().unwrap_or(c.name)).collect();
    tops.dedup();
    // The command is the first word that is not a flag (flags before it, with their values, are
    // passed over); for samples, key and proxy, the next such word names what it does.
    let Some(at) = first_word(&args) else {
        return bare(&args);
    };
    if !tops.contains(&args[at]) {
        eprintln!("rvr: {} is not a command.{}\n{USAGE}", args[at], rvr_recipe::meant(args[at], tops.iter().copied()));
        return 2;
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
                return 2;
            }
            None => {
                eprintln!("rvr {top}: give one of {}.\n{usage}", subs.join(", "));
                return 2;
            }
        }
    }
    let Some(command) = COMMANDS.iter().find(|c| c.name == name) else {
        return wrong(&name, "it is not a command");
    };
    match options::read(command, &rest) {
        Ok(options) => command_run(command.name, &options),
        Err(why) => wrong(command.name, &why),
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

/// Run a command whose options are read.
fn command_run(name: &str, options: &Options) -> u8 {
    let words = options.plain();
    match name {
        "new" => report("new", new::main(words[0])),
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
            report("dump", dump::main(words[0], &flags, options.has("--all")))
        }
        "header" => header(words[0]),
        "diff" => diff::main(words[0], words[1]),
        "reproduce" => reproduce::main(options),
        "empty" => empty::empty_main(options),
        "refill" => empty::refill_main(options),
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

/// Print what failed, as `rvr COMMAND: …`, and give the exit status.
fn report(command: &str, result: Result<(), String>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rvr {command}: {error}");
            1
        }
    }
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
        Ok(Err(why)) => report("header", Err(format!("{file}: its header is printed as the file holds it, but it fails its checks: {why}"))),
        Err(e) => report("header", Err(rowver::plain(file, &e))),
    }
}

/// One line per file: valid, or the first problem found. Every derived column is worked out and
/// checked against what the file records of it, unless the file is empty. With `--complete`, a
/// valid file with holes, or an empty one, fails too.
fn validate(words: &[&str], complete: bool) -> u8 {
    let mut ok = true;
    for file in words {
        let checked = rowver::rvr::validate_file(file).map_err(|e| match e {
            rowver::Error::Io(e) => rowver::plain_it(&e),
            e => e.to_string(),
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
                (read, total) if read == total => format!("read all {} entries, ", press::count(total)),
                (read, total) => format!("read {} of {}, ", press::count(read), samples::some(total as usize, "entry", "entries")),
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

fn said(file: &str, verdict: &Result<(usize, usize, String), String>) -> bool {
    match verdict {
        Ok((rows, columns, signed)) => {
            println!("{file}: valid, {rows} rows, {columns} columns, {signed}")
        }
        Err(why) => println!("{file}: INVALID: {why}"),
    }
    verdict.is_ok()
}

#[cfg(test)]
mod tests {
    /// A short flag is in its command's usage, and in rvr's, beside its long one.
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
