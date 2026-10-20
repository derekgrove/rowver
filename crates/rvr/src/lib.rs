//! `rvr`: start a study, resolve what it reads, press it, and validate, dump, inspect and diff
//! `.rvr` files. The command's whole work is `run`, which the Python package calls too.

mod diff;
mod dump;
mod key;
mod new;
mod press;
mod proxy;
mod reproduce;
mod reuse;
mod samples;

const USAGE: &str = "usage:
  rvr new DIR                              start a study: samples.toml, recipe.toml, .gitignore
  rvr samples COMMAND ...                  what a study reads (rvr samples, alone, lists them)
  rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N]
                                           press the study in DIR (.) into DIR/<its name>.rvr
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
            Ok((summary.rows, summary.columns, signed))
        });
        ok &= said(file, &compared);
    }
    u8::from(!ok)
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
