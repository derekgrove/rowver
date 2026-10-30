//! `rvr`: start a study, resolve what it reads, press it, and validate, dump, inspect and diff
//! `.rvr` files. The command's whole work is `run`, which the Python package calls too.

mod branches;
mod diff;
mod dump;
mod key;
mod new;
mod press;
mod proxy;
mod reproduce;
mod reuse;
mod samples;
mod status;
mod trying;

const USAGE: &str = "usage:
  rvr new DIR                              start a study: samples.toml, recipe.toml, .gitignore
  rvr samples COMMAND ...                  what a study reads (rvr samples, alone, lists them)
  rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N]
                                           press the study in DIR (.) into DIR/<its name>.rvr
  rvr status [DIR] [--out FILE] [--events N|all] [--random] [--seed S]
                                           is the study's .rvr current, and what would press read
  rvr branches [DIR] [--sample NAME] [--find TEXT]
                                           the branches and collections a sample's file holds
  rvr try \"EXPRESSION\" [DIR] [--events N] [--sample NAME] [--kept]
                                           what an expression gives on a sample's first events
  rvr validate FILE... [--parent PARENT]   check files; a derived file against its parent
  rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns]
                                           the header in full, the study's texts, every column
  rvr header FILE                          the JSON header only, as the file holds it
  rvr diff A B                             which column of which sample differs, from the headers
  rvr reproduce FILE [DIR]                 press FILE's own study again and diff the two
  rvr key new|show|trust ...               the key that signs your files; keys you trust
  rvr proxy [init]                         your grid proxy's state; make one from ~/.globus";

/// Make a closed output pipe end the process quietly: `rvr dump … | head` closes it early, and
/// that ends the command as it would a C tool. Any other panic is reported as usual.
pub fn quiet_on_closed_pipe() {
    let usual = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let said = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or("");
        if said.starts_with("failed printing to stdout") && said.contains("Broken pipe") {
            std::process::exit(0);
        }
        usual(info);
    }));
}

/// Run the command these words (the arguments after `rvr`) name; its exit status.
pub fn run(args: &[String]) -> u8 {
    let (flags, words): (Vec<&str>, Vec<&str>) = args
        .iter()
        .map(String::as_str)
        .partition(|a| a.starts_with("--"));
    if words.first() == Some(&"samples") {
        let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
        return samples::main(&rest);
    }
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    match words.first() {
        Some(&"press") => return press::main(&rest),
        Some(&"status") => return status::main(&rest),
        Some(&"try") => return trying::main(&rest),
        Some(&"branches") => return branches::main(&rest),
        Some(&"reproduce") => return reproduce::main(&rest),
        Some(&"key") => return key::main(&rest),
        Some(&"proxy") => return proxy::main(&rest),
        _ => {}
    }
    let code = match (words.first().copied(), &words[1.min(words.len())..]) {
        (Some("new"), [dir]) => report(new::main(dir)),
        (Some("validate"), files) if !files.is_empty() => validate(files, args),
        (Some("dump"), [file]) => report(dump::main(file, &flags)),
        (Some("header"), [file]) => report(header(file)),
        (Some("diff"), [a, b]) => diff::main(a, b),
        (None, _) if flags == ["--version"] => {
            println!(
                "rvr {} (reads and writes rowver {}; commit {})",
                env!("CARGO_PKG_VERSION"),
                rowver::VERSION,
                env!("RVR_COMMIT")
            );
            0
        }
        (Some("help") | None, _) => {
            println!("{USAGE}\n\n{}", samples::USAGE);
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    code
}

/// A command's words: those that are not flags, in order, and the flags given; a flag it does
/// not know, or one without its value, is refused with the command's usage.
pub(crate) struct Words<'a> {
    pub plain: Vec<&'a str>,
    flags: Vec<(&'a str, &'a str)>,
}

impl<'a> Words<'a> {
    /// Read `args`: each flag of `valued` takes the word after it; each of `switches` stands alone.
    pub fn read(args: &[&'a str], valued: &[&str], switches: &[&str], usage: &str) -> Result<Self, String> {
        let mut words = Words { plain: Vec::new(), flags: Vec::new() };
        let mut args = args.iter().copied();
        while let Some(word) = args.next() {
            if valued.contains(&word) {
                let value = args.next().filter(|v| !v.starts_with("--"));
                words.flags.push((word, value.ok_or(format!("{word} needs a value.\n{usage}"))?));
            } else if switches.contains(&word) {
                words.flags.push((word, ""));
            } else if word.starts_with("--") {
                return Err(format!("{word} is not one of its flags.\n{usage}"));
            } else {
                words.plain.push(word);
            }
        }
        Ok(words)
    }

    /// The value given to `flag` (the last, when it was given twice).
    pub fn value(&self, flag: &str) -> Option<&'a str> {
        self.flags.iter().rev().find(|(f, _)| *f == flag).map(|(_, v)| *v)
    }

    /// Whether `flag` was given.
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|(f, _)| *f == flag)
    }
}

/// Print an error and give the exit status of the command.
fn report(result: Result<(), String>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rvr: {error}");
            1
        }
    }
}

fn header(file: &str) -> Result<(), String> {
    let header = rowver::rvr::read_header(file).map_err(|e| format!("{file}: {e}"))?;
    println!("{}", serde_json::to_string_pretty(&header).unwrap());
    Ok(())
}

/// One line per file: valid, or the first problem found. The parent, when named, is checked
/// as a file itself and compared with what each derived file records of it.
fn validate(words: &[&str], args: &[String]) -> u8 {
    let parent = args
        .iter()
        .position(|a| a == "--parent")
        .and_then(|i| args.get(i + 1));
    let files: Vec<&&str> = words
        .iter()
        .filter(|w| Some(**w) != parent.map(String::as_str))
        .collect();
    let mut ok = true;
    let above = parent.map(|p| {
        let checked = rowver::rvr::validate_file(p);
        ok &= said(
            p,
            &checked
                .as_ref()
                .map(|s| (s.rows, s.columns, String::from("checked as the parent")))
                .map_err(ToString::to_string),
        );
        checked.ok().map(|s| s.header)
    });
    for file in files {
        let checked = rowver::rvr::validate_file(file).map_err(|e| e.to_string());
        let compared = checked.and_then(|summary| {
            let derived = summary.header.get("derived").is_some();
            match (&above, derived) {
                (Some(Some(parent)), true) => rowver::rvr::confirm_parent(&summary.header, parent)
                    .map_err(|e| format!("its parent is not the file it records: {e}"))?,
                (Some(None), true) => return Err("its parent is not a valid file".into()),
                (None, true) => {
                    println!("{file}: the parent was not checked (name it with --parent)")
                }
                (Some(_), false) => {
                    println!("{file}: the parent was not compared: this file is not derived")
                }
                (None, false) => {}
            }
            let signed = match summary.header.get("signature") {
                Some(signature) => key::signer(signature["key"].as_str().unwrap_or_default()),
                None => "not signed".into(),
            };
            let read = match entries_read(&summary.header["made"])? {
                Some((read, total)) if read == total => format!("read all {total} entries, "),
                Some((read, total)) => format!("read {read} of {total} entries, "),
                None => String::new(),
            };
            Ok((summary.rows, summary.columns, read + &signed))
        });
        ok &= said(file, &compared);
    }
    u8::from(!ok)
}

/// The entries a press read and the entries its samples hold (`events_read` and `events_total`,
/// the metadata rvr press adds), refused when what each file's record says was read disagrees.
/// `None` for a file without them.
fn entries_read(made: &serde_json::Value) -> Result<Option<(u64, u64)>, String> {
    let Some(samples) = made["files"].as_object() else {
        return Ok(None);
    };
    let (mut read, mut total) = (0, 0);
    for (name, sample) in samples {
        let (Some(said_read), Some(said_total)) = (
            sample["metadata"]["events_read"].as_u64(),
            sample["metadata"]["events_total"].as_u64(),
        ) else {
            return Ok(None);
        };
        let files = sample["files"].as_object().into_iter().flatten();
        let (mut held, mut taken) = (0, 0);
        for (_, file) in files {
            let entries = file["entries"].as_u64().unwrap_or(0);
            held += entries;
            taken += file["take"]
                .as_u64()
                .or(file["stop"].as_u64())
                .unwrap_or(entries);
        }
        // A file the first N events never reach is not recorded: its entries are in the total alone.
        if said_read != taken || said_total < held {
            return Err(format!("sample {name} says it read {said_read} of {said_total} entries, but its files record {taken} read of {held}"));
        }
        read += taken;
        total += said_total;
    }
    Ok(Some((read, total)))
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
