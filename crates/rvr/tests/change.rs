//! Changes made in a file itself (SPEC §8.11, §8.12): `rvr tool`, a press that only adds columns,
//! `rvr derive`, each journaled and put back by the next command when cut short; a second writer
//! refused; the build that made a file asked about (§8.6); and `rvr reproduce --texts` (§9.6.1).

mod common;

use common::{at_terminal, at_terminal_then, framed, inode, journal, parts, reheadered, rvr, rvr_with, study, Study};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// What a writer is told while another holds the file's lock (SPEC 8.12).
const BUSY: &str = "is being changed by another program (it holds the file's lock): wait for it to finish, then try again";

const FLAT: &str = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nlabel = \"Flat ntuple\"\nfiles = \"uproot_flat.root\"\ntree = \"analysis\"\nmetadata = { kind = \"test\" }\n";

/// A recipe of these `[columns]` lines and `[derived]` lines.
fn recipe(columns: &str, derived: &str) -> String {
    format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\njets = \"nJet >= 1\"\n\n[columns]\n{columns}\n\n[totals]\n\n[derived]\n{derived}")
}

/// A study of FLAT and `recipe`, resolved and pressed (signed by a key of its own): its file's name.
fn pressed(name: &str, recipe: &str) -> (Study, String) {
    let dir = study(name, FLAT, recipe);
    assert_eq!(rvr(&dir, &["key", "new", "Tester"]).0, 0);
    assert_eq!(rvr(&dir, &["samples", "resolve", "samples.toml"]).0, 0);
    let (code, said) = rvr(&dir, &["press"]);
    assert_eq!(code, 0, "{said}");
    let file = format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap());
    (dir, file)
}

/// The header of `file` in `dir`, as `rvr header` prints it.
fn header(dir: &Path, file: &str) -> Value {
    serde_json::from_str(&rvr(dir, &["header", file]).1).unwrap()
}

/// The bytes of a file before its header: the magic and its arrays.
fn before_header(bytes: &[u8]) -> Vec<u8> {
    bytes[..8 + parts(bytes).1.len()].to_vec()
}

// SPEC 5.1.3, 5.1.7, 8.11, 8.12; FORMAT 11
#[test]
fn tool_lists_sets_gets_and_drops_tool_keys_asking_first() {
    let (dir, file) = pressed("tool", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let (before, ino) = (std::fs::read(&path).unwrap(), inode(&path));
    let signature = header(&dir, &file)["signature"].clone();
    assert_eq!(rvr(&dir, &["tool", &file]), (0, format!("{file} has no tool keys.\n")));
    std::fs::write(dir.join("data.json"), r#"{"note": "μ", "n": [1, 2.5]}"#).unwrap();
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--set", "data.json"]);
    assert_eq!((code, said.as_str()), (0, format!("Set the tool key my-tool of {file} (25 bytes).\n").as_str()));
    let after = std::fs::read(&path).unwrap();
    assert_eq!((inode(&path), before_header(&after)), (ino, before_header(&before)), "made in the file itself, its arrays untouched");
    assert_eq!(header(&dir, &file)["my-tool"], json!({"note": "μ", "n": [1, 2.5]}));
    assert_eq!(header(&dir, &file)["signature"], signature, "a tool key is outside the signature");
    assert_eq!(rvr(&dir, &["tool", &file]), (0, format!("{file} has 1 tool key:\n  my-tool (25 bytes)\n")));
    assert_eq!(rvr(&dir, &["tool", &file, "my-tool", "--get"]), (0, "{\"note\":\"μ\",\"n\":[1,2.5]}\n".to_owned()));
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--get", "out.json"]);
    assert_eq!((code, said.as_str()), (0, format!("Wrote the tool key my-tool of {file} to out.json (25 bytes).\n").as_str()));
    assert_eq!(std::fs::read_to_string(dir.join("out.json")).unwrap(), "{\"note\":\"μ\",\"n\":[1,2.5]}\n");
    // Each number as DATA.json writes it, but for an exponent, written e with its sign.
    std::fs::write(dir.join("spelt.json"), r#"{"a": 10.50, "b": 1E2, "c": 2e-3}"#).unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "spelt", "--set", "spelt.json"]).0, 0);
    assert_eq!(rvr(&dir, &["tool", &file, "spelt", "--get"]).1, "{\"a\":10.50,\"b\":1e+2,\"c\":2e-3}\n");
    assert_eq!(rvr(&dir, &["tool", &file, "spelt", "--drop", "--force"]).0, 0);
    // A file there already, perhaps edited by hand, is written over only once you say so.
    std::fs::write(dir.join("out.json"), "edited\n").unwrap();
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--get", "out.json"]);
    assert!(code == 1 && said.contains("nothing was written: out.json is there already, and rvr tool asks before it writes over a file, and no one is at a terminal to answer. Give --force (or -f) to write over it without asking."), "{said}");
    let (code, said) = at_terminal(&dir, &["tool", &file, "my-tool", "--get", "out.json"], "n");
    assert!(code == 1 && said.contains(&format!("Write the tool key my-tool of {file} (25 bytes) over out.json (7 bytes)? [y/N] ")) && said.contains("out.json is as it was: the tool key my-tool not written to it."), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("out.json")).unwrap(), "edited\n");
    let (code, said) = at_terminal(&dir, &["tool", &file, "my-tool", "--get", "out.json"], "y");
    assert!(code == 0 && said.contains(&format!("Wrote the tool key my-tool of {file} to out.json (25 bytes).")), "{said}");
    std::fs::write(dir.join("out.json"), "edited\n").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "my-tool", "--get", "out.json", "-f"]).0, 0);
    assert_eq!(std::fs::read_to_string(dir.join("out.json")).unwrap(), "{\"note\":\"μ\",\"n\":[1,2.5]}\n");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // Replacing one asks first; away from a terminal, only --force replaces it.
    std::fs::write(dir.join("other.json"), "{}").unwrap();
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--set", "other.json"]);
    assert!(code == 1 && said.contains("nothing was set: my-tool is a tool key of it already, and rvr tool asks before it replaces one, and no one is at a terminal to answer. Give --force (or -f) to replace it without asking."), "{said}");
    let (code, said) = at_terminal(&dir, &["tool", &file, "my-tool", "--set", "other.json"], "n");
    assert!(code == 1 && said.contains(&format!("Replace the tool key my-tool of {file} (25 bytes) with other.json (2 bytes)? [y/N] ")) && said.contains("is as it was: my-tool not replaced."), "{said}");
    assert_eq!(header(&dir, &file)["my-tool"]["note"], "μ");
    let (code, said) = at_terminal(&dir, &["tool", &file, "my-tool", "--set", "other.json"], "y");
    assert!(code == 0 && said.contains(&format!("Replaced the tool key my-tool of {file} (2 bytes; it held 25).")), "{said}");
    assert_eq!(header(&dir, &file)["my-tool"], json!({}));
    assert_eq!(rvr(&dir, &["tool", &file, "my-tool", "--set", "data.json", "-f"]).0, 0);
    // Dropping one asks first too, naming it and its size.
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--drop"]);
    assert!(code == 1 && said.contains("nothing was dropped: rvr tool asks before it drops a tool key, and no one is at a terminal to answer. Give --force (or -f) to drop it without asking."), "{said}");
    let (code, said) = at_terminal(&dir, &["tool", &file, "my-tool", "--drop"], "yes");
    assert!(code == 0 && said.contains(&format!("Drop the tool key my-tool of {file} (25 bytes)? [y/N] ")) && said.contains(&format!("Dropped the tool key my-tool of {file} (25 bytes).")), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "dropped, the file is as it was, byte for byte");
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--get"]);
    assert!(code == 1 && said.contains(&format!("has no tool key my-tool: rvr tool {file} lists its tool keys")), "{said}");
    // An import the header's rules refuse leaves the file as it was.
    std::fs::write(dir.join("bad.json"), r#"{"a": [-0]}"#).unwrap();
    let (code, said) = rvr(&dir, &["tool", &file, "my-tool", "--set", "bad.json"]);
    assert!(code == 1 && said.contains("[\"my-tool\"].a[0] is written -0"), "{said}");
    let (code, said) = rvr(&dir, &["tool", &file, "data", "--set", "data.json"]);
    assert!(code == 1 && said.contains("data is the format's own key, not a tool key"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Wrong use: no file, a key without what to do with it, or two things at once.
    for args in [&["tool"][..], &["tool", &file, "my-tool"], &["tool", &file, "my-tool", "--get", "--drop"], &["tool", &file, "--drop"]] {
        assert_eq!(rvr(&dir, args).0, 2, "{args:?}");
    }
}

// SPEC 5.1.2, 8.11
#[test]
fn a_file_a_tool_key_keeps_from_opening_has_its_keys_listed_and_written_out_and_the_drop_mends_it() {
    let (dir, file) = pressed("too-deep", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    // Written by another program: nested 70 deep counted from the header (its object 2, and 68 arrays).
    let deep = format!("{{\"d\":{}{}}}", "[".repeat(68), "]".repeat(68));
    reheadered(&path, &path, |header| header["deep"] = serde_json::from_str(&deep).unwrap());
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 1 && said.contains(&format!("rvr tool {file} says whether dropping a tool key lets it open")), "{said}");
    let (code, said) = rvr(&dir, &["tool", &file]);
    assert!(code == 0 && said.starts_with(&format!("{file} has 1 tool key:\n  deep ({} bytes)\n{file} does not open (", deep.len())), "{said}");
    assert_eq!(said.matches("--drop").count(), 1, "{said}");
    assert!(said.trim_end().ends_with(&format!("): rvr tool {file} deep --drop takes out the key that breaks it")), "{said}");
    assert_eq!(rvr(&dir, &["tool", &file, "deep", "--get"]), (0, format!("{deep}\n")));
    // Set over, the key that breaks it is replaced in one step, in a copy.
    std::fs::copy(&path, dir.join("copy.rvr")).unwrap();
    std::fs::write(dir.join("fixed.json"), r#"{"d": []}"#).unwrap();
    let (code, said) = rvr(&dir, &["tool", "copy.rvr", "deep", "--set", "fixed.json", "-f"]);
    assert!(code == 0 && said.contains(&format!("Replaced the tool key deep of copy.rvr ({} bytes; it held {}).", r#"{"d":[]}"#.len(), deep.len())), "{said}");
    assert_eq!(rvr(&dir, &["validate", "copy.rvr"]).0, 0);
    let (code, said) = rvr(&dir, &["tool", &file, "deep", "--drop", "-f"]);
    assert!(code == 0 && said.contains(&format!("Dropped the tool key deep of {file} ({} bytes).", deep.len())), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    assert_eq!(rvr(&dir, &["tool", &file]), (0, format!("{file} has no tool keys.\n")));
    // A file kept from opening by what no tool key holds names no key to drop.
    std::fs::write(dir.join("k.json"), "{}").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "k.json"]).0, 0);
    reheadered(&path, &path, |header| header["made"]["study"] = json!("another"));
    let (code, said) = rvr(&dir, &["tool", &file]);
    assert!(code == 0 && said.contains(&format!("{file} does not open (")) && !said.contains("--drop"), "{said}");
    // A key whose name begins with a dash is named all the same.
    let (dir, file) = pressed("dashed", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    reheadered(&path, &path, |header| header["-x"] = json!(3));
    let (code, said) = rvr(&dir, &["tool", &file]);
    assert!(code == 0 && said.contains(&format!("rvr tool {file} -x --drop takes out the key that breaks it")), "{said}");
    let (code, said) = rvr(&dir, &["tool", &file, "-x", "--get", "-F"]);
    assert!(code == 2 && said.contains("-F is not one of its flags") && !dir.join("-F").exists(), "{said}");
    assert_eq!(rvr(&dir, &["tool", &file, "-x", "--drop", "-f"]).0, 0);
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // Right after FILE, even a name that is a short flag, or begins as a long one, is KEY (SPEC 5.1.2).
    for key in ["-f", "--x"] {
        reheadered(&path, &path, |header| header[key] = json!(3));
        let (code, said) = rvr(&dir, &["tool", &file]);
        assert!(code == 0 && said.contains(&format!("rvr tool {file} {key} --drop takes out the key that breaks it")), "{said}");
        for look in [vec!["header", &file], vec!["dump", &file], vec!["diff", &file, &file]] {
            assert!(rvr(&dir, &look).1.contains(&format!("rvr tool {file} says whether dropping a tool key lets it open")), "{look:?}");
        }
        assert_eq!(rvr(&dir, &["tool", &file, key, "--drop", "-f"]).0, 0, "{key}");
        assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    }
    // After a flag right after FILE, -f is --force; right after FILE it is KEY, and said so where it was meant as the force.
    std::fs::write(dir.join("m.json"), "{}").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "m", "--set", "m.json"]).0, 0);
    let (code, said) = rvr(&dir, &["tool", &file, "-f", "m", "--drop"]);
    assert!(code == 1 && said.contains("has no tool key -f") && said.contains("(-f right after FILE is taken as KEY: give it after KEY, or write --force)"), "{said}");
    assert_eq!(rvr(&dir, &["tool", &file, "--drop", "-f", "m"]).0, 0);
}

// SPEC 8.11
#[test]
fn the_list_names_the_tool_key_that_keeps_a_file_from_opening_and_a_set_replaces_it_in_its_place() {
    let (dir, file) = pressed("which-key", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let whole = std::fs::read(&path).unwrap();
    let (text, arrays) = parts(&whole);
    let text = std::str::from_utf8(text).unwrap();
    let with = |text: &str, member: &str| std::fs::write(&path, framed(format!("{},{member}}}", &text[..text.len() - 1]).as_bytes(), arrays)).unwrap();
    // Written by another program: a lone surrogate, and an object that is not one.
    for member in [r#""k":"\ud800""#, r#""k":5"#] {
        with(text, member);
        let (code, said) = rvr(&dir, &["tool", &file]);
        assert!(code == 0 && said.trim_end().ends_with(&format!("): rvr tool {file} k --drop takes out the key that breaks it")), "{member}: {said}");
    }
    // Two keys that each keep it from opening, beside one that does not: the two are named, and dropped in one change.
    // Keys that begin with a dash too: one with -- (or named -f) is named first, where it is KEY (SPEC 5.1.2).
    let cases = [
        (r#""a":5,"ok":{},"b":5"#, "a b", ["a", "b"]),
        (r#""k":"\ud800","ok":{},"b c":5"#, "k 'b c'", ["k", "b c"]),
        (r#""-a":5,"ok":{},"--b":5"#, "--b -a", ["--b", "-a"]),
    ];
    for (members, drop, keys) in cases {
        with(text, members);
        let (code, said) = rvr(&dir, &["tool", &file]);
        assert!(code == 0 && said.trim_end().ends_with(&format!("): more than one of its tool keys breaks it, and rvr tool {file} {drop} --drop takes them out")), "{members}: {said}");
        let mut args = vec!["tool", file.as_str()];
        args.extend(keys);
        args.extend(["--drop", "-f"]);
        let (code, said) = rvr(&dir, &args);
        assert!(code == 0 && said.contains(&format!("Dropped the tool keys {} (", keys[0])), "{members}: {said}");
        assert_eq!(rvr(&dir, &["tool", &file]), (0, format!("{file} has 1 tool key:\n  ok (2 bytes)\n")));
    }
    // Of keys to drop, the one the file does not hold is said, though another breaks its header.
    with(text, r#""a":"\ud800""#);
    let (code, said) = rvr(&dir, &["tool", &file, "a", "bb", "--drop", "-f"]);
    assert!(code == 1 && said.contains(&format!("{file} has no tool key bb: rvr tool {file} lists its tool keys")), "{said}");
    // A key in the file that breaks it is said as the file's, not as the one given.
    with(text, r#""x y":{}"#);
    std::fs::write(dir.join("k.json"), "{}").unwrap();
    for args in [vec!["tool", &file, "k", "--set", "k.json", "-f"], vec!["tool", &file, "--drop", "-f", "x y"]] {
        let (code, said) = rvr(&dir, &args);
        let expected = if args[2] == "k" { (1, format!("rvr tool {file} says whether dropping a tool key lets it open")) } else { (0, "Dropped the tool key x y".to_owned()) };
        assert!(code == expected.0 && said.contains(&expected.1) && !said.contains("it was given"), "{args:?}: {said}");
    }
    // A key spelled as one of rvr tool's flags, or two it names only right after FILE, are dropped from Python, which can name them.
    for (members, advice) in [
        (r#""--force":5"#, format!(r#"rowver.drop_tool("{file}", "--force") in Python (rvr tool reads --force as its flag) takes out the key that breaks it"#)),
        (r#""--drop=x":5"#, format!(r#"rowver.drop_tool("{file}", "--drop=x") in Python (rvr tool reads --drop=x as its flag) takes out the key that breaks it"#)),
        (r#""--a":5,"ok":{},"-f":5"#, format!(r#"rowver.drop_tool("{file}", "--a", "-f") in Python (rvr tool names only one key beginning with -- or named -f, right after FILE) takes them out"#)),
    ] {
        with(text, members);
        let (code, said) = rvr(&dir, &["tool", &file]);
        assert!(code == 0 && said.trim_end().ends_with(&advice), "{members}: {said}");
    }
    // After KEY, a word beginning with -- is a flag, and a slip of one is said so.
    let (code, said) = rvr(&dir, &["tool", &file, "ok", "--drop", "--forse"]);
    assert!(code == 2 && said.contains("--forse is not one of its flags") && said.contains("--force"), "{said}");
    // A name a shell takes apart is quoted in the advice.
    with(text, r#""a;rm x":5"#);
    assert!(rvr(&dir, &["tool", &file]).1.trim_end().ends_with(&format!("rvr tool {file} 'a;rm x' --drop takes out the key that breaks it")));
    // Set over, a key that does not parse is replaced where it lies.
    with(text, r#""k":"\ud800","z":{}"#);
    std::fs::write(dir.join("k.json"), "{}").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "k.json", "-f"]).0, 0);
    assert_eq!(rvr(&dir, &["tool", &file]), (0, format!("{file} has 2 tool keys:\n  k (2 bytes)\n  z (2 bytes)\n")));
    // A break under the format's own keys is no tool key's, though a misnamed key, or one that is no object, is there too.
    let minus = text.replacen(r#""at":0,"#, r#""at":-0,"#, 1);
    let lone = text.replacen(r#""study":""#, r#""study":"\ud800"#, 1);
    for (broken, member) in [(&minus, r#""Made":{}"#), (&lone, r#""k":5"#)] {
        assert_ne!(broken, text);
        with(broken, member);
        let (code, said) = rvr(&dir, &["tool", &file]);
        assert!(code == 0 && said.contains(&format!("{file} does not open (")) && !said.contains("--drop"), "{member}: {said}");
    }
}

// SPEC 8.11
#[test]
fn tool_get_never_writes_a_key_over_the_file_it_is_read_from() {
    let (dir, file) = pressed("tool-get-itself", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("data.json"), "{}").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0);
    let before = std::fs::read(&path).unwrap();
    std::os::unix::fs::symlink(&file, dir.join("link.rvr")).unwrap();
    let dotted = format!("./{file}");
    // Named as it is, another way, through a link to it, and as the file a link names.
    for (named, out) in [(file.as_str(), file.as_str()), (file.as_str(), dotted.as_str()), (file.as_str(), "link.rvr"), ("link.rvr", file.as_str())] {
        let (code, said) = rvr(&dir, &["tool", named, "k", "--get", out, "-f"]);
        assert!(code == 1 && said.contains(&format!("{out} is {named} itself: write the key's object to another file")), "{named} {out}: {said}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{named} {out}");
        assert!(std::fs::symlink_metadata(dir.join("link.rvr")).unwrap().file_type().is_symlink());
    }
}

// SPEC 8.12
#[test]
fn a_press_whose_target_cannot_be_opened_stops_and_says_how_to_mend_it() {
    let (dir, file) = pressed("unopenable-target", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Pressed again as it is, and with a column added: either way rvr cannot carry over what it cannot read.
    for columns in ["branches = [\"nJet\"]", "branches = [\"nJet\", \"Jet_pt\"]"] {
        std::fs::write(dir.join("recipe.toml"), recipe(columns, "")).unwrap();
        let (code, said) = rvr(&dir, &["press"]);
        assert!(code == 1 && said.contains(&format!("chmod u+rw {file}, or remove it")), "{columns}: {said}");
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

// SPEC 8.12
#[test]
fn a_change_stopped_after_each_step_is_put_back_by_the_next_command() {
    let (dir, file) = pressed("stopped", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    // A debug build stops after step N of the change, as RVR_STOP_CHANGE asks: the lock taken (1),
    // the journal written (2), the new tail written (3).
    // After step 3 the change has finished but for letting its journal go, which the next command does.
    for (step, command) in [(1, "validate"), (2, "header"), (2, "dump"), (3, "tool"), (2, "press"), (3, "dump")] {
        let (code, said) = rvr_with(&dir, &[("RVR_STOP_CHANGE", &step.to_string())], &["tool", &file, "k", "--set", "data.json"]);
        assert!(code == 99 && said.contains(&format!("Stopped after step {step} of the change, as RVR_STOP_CHANGE asks.")), "{step}: {said}");
        assert_eq!(journal(&path).exists(), step >= 2, "{step}");
        let args: Vec<&str> = match command {
            "tool" => vec!["tool", &file],
            "press" => vec!["press"],
            other => vec![other, &file],
        };
        let (_, said) = rvr(&dir, &args);
        if step == 3 {
            assert!(said.contains(&format!("Let go {file}-journal: the change to {file} had finished, so {file} is as the change made it.")) && !said.contains("Put "), "{command}: {said}");
            assert_eq!((header(&dir, &file)["k"].clone(), journal(&path).exists()), (json!({"n": 1}), false), "{command}");
            std::fs::write(&path, &before).unwrap();
            continue;
        }
        assert!(step < 2 || said.contains(&format!("Put {file} back as it was: a change to it was cut short.")), "{step} {command}: {said}");
        assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before.clone(), false), "{step} {command}: put back byte for byte");
    }
    // Not stopped, the change is made, and the journal is gone.
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0);
    assert_eq!((header(&dir, &file)["k"].clone(), journal(&path).exists()), (json!({"n": 1}), false));
    // A change that shrank the file, whose putting back was cut short too, the file grown back to
    // its old length with zeros past the new one, is put back.
    let held = std::fs::read(&path).unwrap();
    assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "3")], &["tool", &file, "k", "--drop", "-f"]).0, 99);
    let shrunk = std::fs::metadata(&path).unwrap().len() as usize;
    let mut torn = held[..shrunk].to_vec();
    torn.resize(held.len(), 0);
    std::fs::write(&path, &torn).unwrap();
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && said.contains(&format!("Put {file} back as it was")), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (held, false));
    // A journal, and a new file written anew that a stop leaves, are no wider open than the file,
    // and open to its owner: a private file's are private, a group's the group's to read.
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    for (file_mode, journal_mode) in [(0o600, 0o600), (0o640, 0o640)] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(file_mode)).unwrap();
        assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "2")], &["tool", &file, "k", "--drop", "-f"]).0, 99);
        assert_eq!(mode(&journal(&path)), journal_mode);
        assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440)).unwrap();
    assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "3")], &["tool", &file, "k", "--drop", "-f"]).0, 99);
    let staged: Vec<_> = std::fs::read_dir(&*dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().contains(".anew-")).collect();
    assert!(staged.len() == 1 && mode(&staged[0]) == 0o640, "{staged:?}");
    std::fs::remove_file(&staged[0]).unwrap();
}

// SPEC 8.9, 8.10, 8.12
#[test]
fn every_writer_is_refused_while_another_holds_the_file() {
    let (dir, file) = pressed("locked", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("data.json"), "{}").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0);
    // A column added, which a press writes in place, and --full, which writes a new file.
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\", \"weight\"]", "")).unwrap();
    let before = std::fs::read(&path).unwrap();
    let held = std::fs::File::open(&path).unwrap();
    held.lock().unwrap();
    let writers = [
        &["tool", &file, "other", "--set", "data.json"][..],
        &["tool", &file, "k", "--drop", "-f"],
        &["derive", &file, "twice = nJet * 2"],
        &["press"],
        &["press", "--full"],
        &["empty", &file, "-f"],
    ];
    for args in writers {
        let (code, said) = rvr(&dir, args);
        assert!(code == 1 && said.contains(BUSY), "{args:?}: {said}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{args:?}");
    }
    held.unlock().unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\"]", "")).unwrap();
    assert_eq!(rvr(&dir, &["empty", &file, "-f"]).0, 0);
    let held = std::fs::File::open(&path).unwrap();
    held.lock().unwrap();
    let (code, said) = rvr(&dir, &["refill", &file]);
    assert!(code == 1 && said.contains(BUSY), "{said}");
    held.unlock().unwrap();
    let (code, said) = rvr(&dir, &["refill", &file]);
    assert_eq!(code, 0, "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

// SPEC 8.9, 8.12, 9.1
#[test]
fn a_file_being_changed_is_one_failure_among_the_files_validate_and_empty_are_given() {
    let (dir, file) = pressed("held-among", &recipe("branches = [\"nJet\"]", ""));
    let names = ["a.rvr", "b.rvr", "c.rvr"];
    for name in names {
        std::fs::copy(dir.join(&file), dir.join(name)).unwrap();
    }
    // Another program is changing b.rvr now: its journal beside it, its lock held.
    std::fs::write(dir.join("b.rvr-journal"), "a change under way").unwrap();
    let held = std::fs::File::open(dir.join("b.rvr")).unwrap();
    held.lock().unwrap();
    let (code, said) = rvr(&dir, &[&["validate"][..], &names].concat());
    assert!(code == 1 && said.contains("a.rvr: valid, ") && said.contains("c.rvr: valid, "), "{said}");
    assert!(said.lines().any(|line| line.starts_with("b.rvr: INVALID: ") && line.contains(BUSY)), "{said}");
    let (code, said) = rvr(&dir, &[&["empty"][..], &names, &["-f"]].concat());
    assert!(code == 1 && said.contains(BUSY), "{said}");
    held.unlock().unwrap();
    std::fs::remove_file(dir.join("b.rvr-journal")).unwrap();
    let emptied = names.map(|name| rowver::rvr::Reader::open(dir.join(name)).unwrap().empty());
    assert_eq!(emptied, [true, false, true], "{said}");
}

// SPEC 8.6, 8.12
#[test]
fn a_press_holds_the_file_from_its_first_read_while_it_asks() {
    let (dir, file) = pressed("held-asking", &recipe("branches = [\"nJet\"]", ""));
    made_by(&dir, &file, "0123456789abcdef0123456789abcdef01234567");
    std::fs::write(dir.join("data.json"), "{}").unwrap();
    let (code, said) = at_terminal_then(&dir, &["press"], "n", || {
        let (code, said) = rvr(&dir, &["tool", &file, "k", "--set", "data.json"]);
        assert!(code == 1 && said.contains(BUSY), "{said}");
    });
    assert!(code == 1 && said.contains("Press every column? [y/N] "), "{said}");
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0, "let go when the press ends");
}

// SPEC 8.9, 8.10, 8.12
#[test]
fn a_file_written_anew_over_another_keeps_its_permissions() {
    let (dir, file) = pressed("permissions", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    for args in [&["press", "--full"][..], &["empty", &file, "-f"], &["refill", &file]] {
        let ino = inode(&path);
        let (code, said) = rvr(&dir, args);
        assert_eq!(code, 0, "{args:?}: {said}");
        assert_ne!(inode(&path), ino, "{args:?}: a new file");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640, "{args:?}");
    }
}

// SPEC 8.12
#[test]
fn a_journal_that_cannot_be_written_whole_is_removed_and_the_file_left_as_it_was() {
    let (dir, file) = pressed("journal-cut", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    // No file may grow past one block, as on a full disk: the journal, which holds the header
    // (some kB), fails as it is written.
    let script = format!("trap '' XFSZ; ulimit -f 1; exec '{}' tool '{file}' k --set data.json", env!("CARGO_BIN_EXE_rvr"));
    let out = std::process::Command::new("sh").args(["-c", &script]).current_dir(&*dir).env("RVR_CONFIG", dir.join(".rvr-config")).output().unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.code() == Some(1) && said.contains(&format!("{file}-journal")), "{:?}: {said}", out.status);
    assert!(!journal(&path).exists(), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && !said.contains("cut short"), "{said}");
}

// SPEC 8.10, 8.12
#[test]
fn a_file_a_column_was_added_to_in_place_is_refilled_byte_for_byte() {
    let (dir, file) = pressed("refill-in-place", &recipe("branches = [\"weight\", \"nJet\"]", ""));
    let path = dir.join(&file);
    // The new column's record between the two, its arrays after both.
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"Jet_pt\", \"nJet\"]", "")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains("in place"), "{said}");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(rvr(&dir, &["empty", &file, "-f"]).0, 0);
    let (code, said) = rvr(&dir, &["refill", &file]);
    assert!(code == 0 && said.contains("is as it was") && !said.contains("otherwise"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "refilled, byte for byte as it was");
}

// SPEC 8.11, 8.12
#[test]
fn a_file_that_may_not_be_written_is_changed_anew_and_moved_over() {
    let (dir, file) = pressed("read-only", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::write(dir.join("data.json"), "{}").unwrap();
    let anew = format!("{file} cannot be changed in place (you may not write it): wrote it anew and moved it over.");
    let changes = [
        &["tool", &file, "k", "--set", "data.json"][..],
        &["tool", &file, "k", "--drop", "-f"],
        &["derive", &file, "twice = nJet * 2"],
        &["derive", &file, "--drop", "twice"],
    ];
    for args in changes {
        let ino = inode(&path);
        let (code, said) = rvr(&dir, args);
        assert!(code == 0 && said.contains(&anew), "{args:?}: {said}");
        assert_ne!(inode(&path), ino, "{args:?}");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o444, "{args:?}");
        assert_eq!(rvr(&dir, &["validate", &file]).0, 0, "{args:?}");
    }
    // Emptying loses data: only a file you may write gives back its space.
    let before = std::fs::read(&path).unwrap();
    let (code, said) = rvr(&dir, &["empty", &file, "-f"]);
    assert!(code == 1 && said.contains(&format!("{file} is read-only: rvr empty gives back the space only of a file you may write (chmod u+w {file} first)")), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // A press that only adds a column writes a new file, decided before it reads.
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\", \"weight\"]", "")).unwrap();
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 1 && !said.contains("to the file in place") && said.contains("cannot be changed in place (you may not write it)"), "{said}");
    let ino = inode(&path);
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains(&anew) && !said.contains(&format!("Wrote {file} in place")), "{said}");
    assert_ne!(inode(&path), ino);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o444);
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // In a folder that may not be written, the folder is said, for the journal or the new file.
    std::fs::create_dir(dir.join("shut")).unwrap();
    std::fs::copy(&path, dir.join("shut/f.rvr")).unwrap();
    // As a stopped press leaves it, so that rvr empty gets as far as its new file.
    std::fs::create_dir(dir.join("shut/f.rvr.partial")).unwrap();
    std::fs::set_permissions(dir.join("shut"), std::fs::Permissions::from_mode(0o555)).unwrap();
    /// The folder opened again however the test ends, so that its study can be removed.
    struct Opened(std::path::PathBuf);
    impl Drop for Opened {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    let _opened = Opened(dir.join("shut"));
    for (mode, what) in [(0o644, "the journal of the change"), (0o444, "the new file that replaces it")] {
        std::fs::set_permissions(dir.join("shut/f.rvr"), std::fs::Permissions::from_mode(mode)).unwrap();
        let before = std::fs::read(dir.join("shut/f.rvr")).unwrap();
        let (code, said) = rvr(&dir, &["tool", "shut/f.rvr", "k", "--set", "data.json"]);
        assert!(code == 1 && said.contains(&format!("shut/f.rvr cannot be changed: its folder may not be written, and {what} is written there first: give yourself the right to write it (chmod u+w shut, if it is yours)")), "{said}");
        assert_eq!(std::fs::read(dir.join("shut/f.rvr")).unwrap(), before);
    }
    std::fs::set_permissions(dir.join("shut/f.rvr"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, said) = rvr(&dir, &["empty", "shut/f.rvr", "--anyway", "-f"]);
    assert!(code == 1 && said.contains("shut/f.rvr cannot be changed: its folder may not be written, and the new file that replaces it is written there first"), "{said}");
}

// SPEC 8.12
#[test]
fn a_file_reached_through_a_link_is_changed_where_it_lies() {
    let (dir, file) = pressed("linked", &recipe("branches = [\"nJet\"]", ""));
    std::fs::create_dir(dir.join("store")).unwrap();
    let real = dir.join("store").join(&file);
    std::fs::rename(dir.join(&file), &real).unwrap();
    std::os::unix::fs::symlink(&real, dir.join(&file)).unwrap();
    let linked = || std::fs::symlink_metadata(dir.join(&file)).unwrap().file_type().is_symlink();
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    let (code, said) = rvr(&dir, &["tool", &file, "k", "--set", "data.json"]);
    assert!(code == 0 && linked(), "{said}");
    assert_eq!(header(&dir, &format!("store/{file}"))["k"], json!({"n": 1}));
    let ino = inode(&real);
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 0 && linked() && inode(&real) != ino, "{said}");
    assert_eq!(header(&dir, &format!("store/{file}"))["k"], json!({"n": 1}), "the press keeps its tool keys");
    // Its inputs lie beside the link, not beside the file it names: --study says so.
    let (code, said) = rvr(&dir, &["empty", &file, "-f", "--study", "."]);
    assert!(code == 0 && linked(), "{said}");
    assert!(rowver::rvr::Reader::open(&real).unwrap().empty(), "the file it names is emptied");
    let (code, said) = rvr(&dir, &["refill", &file, "--study", "."]);
    assert!(code == 0 && linked() && !rowver::rvr::Reader::open(&real).unwrap().empty(), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
}

// SPEC 8.9, 8.12
#[test]
fn empty_holds_the_file_while_it_asks() {
    let (dir, file) = pressed("empty-asking", &recipe("branches = [\"nJet\"]", ""));
    std::fs::write(dir.join("data.json"), "{}").unwrap();
    let (code, said) = at_terminal_then(&dir, &["empty", &file], "n", || {
        let (code, said) = rvr(&dir, &["tool", &file, "k", "--set", "data.json"]);
        assert!(code == 1 && said.contains(BUSY), "{said}");
    });
    assert!(code == 1 && said.contains("[y/N] ") && said.contains("is as it was: not emptied."), "{said}");
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0, "let go once it ends");
}

// SPEC 8.12
#[test]
fn a_journal_that_does_not_check_out_is_let_go_and_said_so() {
    let (dir, file) = pressed("let-go", &recipe("branches = [\"nJet\"]", ""));
    let before = std::fs::read(dir.join(&file)).unwrap();
    std::fs::write(journal(&dir.join(&file)), "a journal cut short").unwrap();
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && said.contains(&format!("Let go {file}-journal: a change to {file} was cut short before {file} was touched, so {file} is as it was.")) && !said.contains("Put "), "{said}");
    assert_eq!((std::fs::read(dir.join(&file)).unwrap(), journal(&dir.join(&file)).exists()), (before, false));
}

// SPEC 8.12
#[test]
fn a_read_only_file_whose_change_was_cut_short_is_told_how_to_be_put_back() {
    let (dir, file) = pressed("read-only-journal", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "2")], &["tool", &file, "k", "--set", "data.json"]).0, 99);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let (code, said) = rvr(&dir, &["header", &file]);
    let told = format!("{file} has the journal of a change to it cut short, and is read-only, so it cannot be put back: chmod u+w {file}, then run any rvr command on it.");
    assert!(code == 1 && said.contains(&told), "{said}");
    assert!(journal(&path).exists());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, said) = rvr(&dir, &["header", &file]);
    assert!(code == 0 && said.contains(&format!("Put {file} back as it was")), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before.clone(), false));
    // The journal of a change adding arrays, beside a read-only file it is not of, is said to be so first.
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\", \"Jet_pt\"]", "")).unwrap();
    assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "2")], &["press"]).0, 99);
    std::fs::write(&path, vec![7; 2 * before.len()]).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let (code, said) = rvr(&dir, &["header", &file]);
    assert!(code == 1 && said.contains(&format!("{file}-journal is not of this {file}")), "{said}");
    std::fs::remove_file(journal(&path)).unwrap();
}

// SPEC 8.12
#[test]
fn a_change_cut_short_through_a_link_is_put_back_naming_the_file_it_names() {
    let (dir, file) = pressed("linked-journal", &recipe("branches = [\"nJet\"]", ""));
    std::fs::create_dir(dir.join("store")).unwrap();
    let real = dir.join("store").join(&file);
    std::fs::rename(dir.join(&file), &real).unwrap();
    std::os::unix::fs::symlink(&real, dir.join(&file)).unwrap();
    let (real, before) = (std::fs::canonicalize(&real).unwrap(), std::fs::read(&real).unwrap());
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "2")], &["tool", &file, "k", "--set", "data.json"]).0, 99);
    assert!(journal(&real).exists() && !journal(&dir.join(&file)).exists());
    let shown = real.display();
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && said.contains(&format!("Put {shown} back as it was: a change to it was cut short.")), "{said}");
    assert_eq!(std::fs::read(&real).unwrap(), before);
    std::fs::write(journal(&real), "a journal cut short").unwrap();
    let (code, said) = rvr(&dir, &["header", &file]);
    assert!(code == 0 && said.contains(&format!("Let go {shown}-journal: a change to {shown} was cut short before {shown} was touched, so {shown} is as it was.")), "{said}");
}

// SPEC 8.11
#[test]
fn data_for_a_tool_key_is_checked_before_rvr_asks_to_replace_one() {
    let (dir, file) = pressed("tool-checked", &recipe("branches = [\"nJet\"]", ""));
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0);
    let before = std::fs::read(dir.join(&file)).unwrap();
    for (name, text, refused) in [("minus.json", r#"{"a": -0}"#, "is written -0"), ("list.json", "[1]", "must be one JSON object"), ("cut.json", "{\"a\": ", "")] {
        std::fs::write(dir.join(name), text).unwrap();
        let (code, said) = at_terminal(&dir, &["tool", &file, "k", "--set", name], "y");
        assert!(code == 1 && said.contains(refused) && !said.contains("[y/N]"), "{name}: {said}");
    }
    assert_eq!(std::fs::read(dir.join(&file)).unwrap(), before);
    // The size asked about is the data's as the header would hold it: written compactly.
    std::fs::write(dir.join("spaced.json"), "{ \"n\" :  2 }\n").unwrap();
    let (code, said) = at_terminal(&dir, &["tool", &file, "k", "--set", "spaced.json"], "n");
    assert!(code == 1 && said.contains(&format!("Replace the tool key k of {file} (7 bytes) with spaced.json (7 bytes)? [y/N] ")), "{said}");
}

// SPEC 8.6
#[test]
fn a_file_of_another_build_that_no_build_could_build_on_is_pressed_again_without_asking() {
    let (dir, file) = pressed("another-build-kind", &recipe("branches = [\"nJet\"]", ""));
    reheadered(&dir.join(&file), &dir.join(&file), |h| h["data"]["cols"][0]["kind"] = "u4".into());
    made_by(&dir, &file, "0123456789abcdef0123456789abcdef01234567");
    let (code, said) = rvr(&dir, &["press"]);
    let why = format!("Pressing every column: {file} cannot be built on, as its column nJet is of another kind than this press makes it (i4).");
    assert!(code == 0 && said.contains(&why) && !said.contains("[y/N]") && !said.contains("was made by rvr"), "{said}");
}

// SPEC 8.12
#[test]
#[cfg(debug_assertions)]
fn rows_read_in_place_unlike_the_files_write_nothing() {
    let (dir, file) = pressed("unlike-rows", &recipe("branches = [\"weight\", \"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"nJet\", \"Jet_pt\"]", "")).unwrap();
    let stamp = || (std::fs::read(&path).unwrap(), std::fs::metadata(&path).unwrap().modified().unwrap(), journal(&path).exists());
    let before = stamp();
    // A debug build hands the check other entries than those read, as RVR_OTHER_ROWS asks.
    let (code, said) = rvr_with(&dir, &[("RVR_OTHER_ROWS", "1")], &["press"]);
    assert!(code == 1 && said.contains("the rows read are not the file's rows"), "{said}");
    assert_eq!(stamp(), before, "nothing written, so nothing put back");
}

// SPEC 8.12
#[test]
#[cfg(debug_assertions)]
fn no_command_opens_a_file_again_while_it_holds_its_lock() {
    // A debug build stops at a new descriptor of a file it holds locked, as closing one lets the
    // lock go where flock is POSIX locks (Linux NFS): each of these runs to its end.
    let (dir, file) = pressed("one-descriptor", &recipe("branches = [\"weight\", \"nJet\"]", ""));
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    let ran = |args: &[&str]| {
        let (code, said) = rvr(&dir, args);
        assert!(code == 0 && !said.contains("while this program holds it locked"), "{args:?}: {said}");
        said
    };
    ran(&["tool", &file, "k", "--set", "data.json"]);
    ran(&["tool", &file, "k", "--set", "data.json", "-f"]);
    ran(&["tool", &file, "k", "--drop", "-f"]);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"nJet\", \"Jet_pt\"]", "")).unwrap();
    assert!(ran(&["press"]).contains("in place"));
    ran(&["derive", &file, "twice = nJet * 2"]);
    ran(&["derive", &file, "--drop", "twice"]);
    ran(&["empty", &file, "-f"]);
    ran(&["refill", &file]);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"Jet_pt\"]", "")).unwrap();
    assert!(ran(&["press"]).contains("every column is copied from it"));
}

// SPEC 8.12
#[test]
fn a_journal_whose_file_is_gone_is_let_go_by_status_and_press() {
    let (dir, file) = pressed("orphan", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    // A change cut short once its journal is written, then the file removed.
    let orphaned = || {
        assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "2")], &["tool", &file, "k", "--set", "data.json"]).0, 99);
        std::fs::remove_file(&path).unwrap();
    };
    let told = format!("Let go {file}-journal: {file} is not there.");
    orphaned();
    let (_, said) = rvr(&dir, &["status"]);
    assert!(said.contains(&told) && !said.contains("is not there:") && !journal(&path).exists(), "{said}");
    assert_eq!(rvr(&dir, &["press"]).0, 0);
    orphaned();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && said.contains(&told) && !journal(&path).exists(), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
}

// SPEC 8.2, 8.12
#[test]
fn a_press_in_place_that_cannot_read_an_input_stops_and_leaves_the_file_as_it_was() {
    let (dir, file) = pressed("unreadable-input", &recipe("branches = [\"weight\", \"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"nJet\", \"Jet_pt\"]", "")).unwrap();
    let stamp = || (std::fs::read(&path).unwrap(), std::fs::metadata(&path).unwrap().modified().unwrap(), journal(&path).exists());
    let before = stamp();
    let input = dir.join("uproot_flat.root");
    // Not there, then there but not to be read.
    std::fs::rename(&input, dir.join("away.root")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("uproot_flat.root no longer exists") && said.contains("a file that cannot be read stops the press"), "{said}");
    assert!(!said.contains("the rows read are not the file's rows"), "{said}");
    assert_eq!(stamp(), before);
    std::fs::rename(dir.join("away.root"), &input).unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(code == 1 && said.contains("uproot_flat.root is not yours to read") && said.contains(&format!("{file} was not written")), "{said}");
    assert!(!said.contains("the rows read are not the file's rows"), "{said}");
    assert_eq!(stamp(), before);
    // Another file under its name.
    std::fs::copy(dir.join("codecs-zlib.root"), &input).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 1 && said.contains("uproot_flat.root") && !said.contains("the rows read are not the file's rows"), "{said}");
    assert_eq!(stamp(), before);
}

// SPEC 8.12
#[test]
fn every_command_given_a_file_puts_it_back_first_when_a_change_to_it_was_cut_short() {
    let (dir, file) = pressed("every-command", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    let put_back = format!("Put {file} back as it was: a change to it was cut short.");
    let named: Vec<&str> = rvr::commands().filter(|(_, words)| words.contains("file")).map(|(name, _)| name).collect();
    assert!(named.len() >= 9, "{named:?}");
    for name in named {
        let args: Vec<&str> = match name {
            "derive" => vec!["derive", &file, "twice = nJet * 2"],
            "validate" | "dump" | "header" | "refill" | "tool" => vec![name, &file],
            "diff" => vec!["diff", &file, &file],
            "reproduce" => vec!["reproduce", &file, "again", "--texts"],
            "empty" => vec!["empty", &file, "-f"],
            other => panic!("rvr {other} names a file: give this test a way to run it on one"),
        };
        // A change cut short as its new tail was written, the file's old length all a crash kept:
        // the file is not what it was.
        let before = std::fs::read(&path).unwrap();
        assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "3")], &["tool", &file, "k", "--set", "data.json"]).0, 99);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(before.len() as u64).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), before);
        let (_, said) = rvr(&dir, &args);
        assert!(said.contains(&put_back) && !journal(&path).exists(), "{args:?}: {said}");
    }
}

// SPEC 8.12
#[test]
fn a_full_file_copied_over_an_empty_one_a_change_to_which_was_cut_short_is_not_put_back() {
    let (dir, file) = pressed("copied-over-empty", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let full = std::fs::read(&path).unwrap();
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    for step in ["2", "3"] {
        std::fs::write(&path, &full).unwrap();
        assert_eq!(rvr(&dir, &["empty", &file, "-f"]).0, 0);
        assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", step)], &["tool", &file, "k", "--set", "data.json"]).0, 99);
        // cp keeps the inode: the full file now lies where the empty one did, the journal beside it.
        let ino = inode(&path);
        std::fs::write(&path, &full).unwrap();
        assert_eq!(inode(&path), ino);
        let (code, said) = rvr(&dir, &["header", &file]);
        assert!(code == 1 && said.contains(&format!("{file}-journal is not of this {file}")), "{step}: {said}");
        assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (full.clone(), true), "{step}");
        std::fs::remove_file(journal(&path)).unwrap();
    }
}

// SPEC 8.11; FORMAT 1, 11
#[test]
fn tool_drop_takes_out_a_tool_key_that_breaks_the_header_rules() {
    let (dir, file) = pressed("drop-broken", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    let (text, arrays) = parts(&before);
    let text = std::str::from_utf8(text).unwrap();
    let with = |text: &str, key: &str, value: &str| framed(format!("{},\"{key}\":{value}}}", &text[..text.len() - 1]).as_bytes(), arrays);
    let deep = |n: usize| format!("{{\"d\":{}{}}}", "[".repeat(n), "]".repeat(n));
    let (deep, deeper) = (deep(63), deep(198));
    for (key, value) in [("minus", r#"{"a":[-0]}"#), ("deep", deep.as_str()), ("lone", r#"{"s":"\ud800"}"#), ("deeper", deeper.as_str())] {
        std::fs::write(&path, with(text, key, value)).unwrap();
        assert_eq!(rvr(&dir, &["validate", &file]).0, 1, "{key}");
        let (code, said) = rvr(&dir, &["tool", &file, key, "--drop", "-f"]);
        assert!(code == 0 && said.contains(&format!("Dropped the tool key {key} of {file}")), "{key}: {said}");
        assert_eq!(rvr(&dir, &["validate", &file]).0, 0, "{key}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{key}");
    }
    // Broken outside the key dropped: refused, the file as it was.
    let broken = with(&text.replacen("\"made\":{", "\"made\":{\"x\":-0,", 1), "sound", "{}");
    std::fs::write(&path, &broken).unwrap();
    let (code, said) = rvr(&dir, &["tool", &file, "sound", "--drop", "-f"]);
    assert!(code == 1 && said.contains("made.x is written -0"), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (broken, false));
}

// SPEC 8.9, 8.12
#[test]
#[cfg(debug_assertions)]
fn a_file_on_a_read_only_filesystem_is_refused_a_press_before_it_stages_and_told_how_to_empty_it() {
    // A debug build meets a read-only disk where RVR_READ_ONLY_DISK says.
    let (dir, file) = pressed("read-only-disk", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\", \"weight\"]", "")).unwrap();
    let disk = [("RVR_READ_ONLY_DISK", path.to_str().unwrap())];
    let refused = format!("{file} lies on a read-only filesystem: rvr press cannot write it or a new file beside it; copy the study to a disk you may write");
    let (_, said) = rvr_with(&dir, &disk, &["status"]);
    assert!(said.contains(&refused) && !said.contains("would write it anew"), "{said}");
    for args in [&["press"][..], &["press", "--restart"], &["press", "--full"]] {
        let (code, said) = rvr_with(&dir, &disk, args);
        assert!(code == 1 && said.contains(&refused), "{args:?}: {said}");
        assert!(!dir.join(format!("{file}.partial")).exists(), "{args:?}: nothing staged");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{args:?}");
    }
    let (code, said) = rvr_with(&dir, &disk, &["empty", &file, "-f"]);
    assert!(code == 1 && said.contains(&format!("{file} lies on a read-only filesystem: rvr empty gives back the space only of a file you may write (mount it to be written first)")), "{said}");
    // Current, it has nothing to press: it is not refused.
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\"]", "")).unwrap();
    let (code, said) = rvr_with(&dir, &disk, &["press"]);
    assert!(code == 0 && said.contains("is current: nothing to press"), "{said}");
}

// SPEC 8.9, 8.12
#[test]
#[cfg(debug_assertions)]
fn rvr_empty_checks_a_file_before_it_stages_so_a_read_only_disk_or_folder_gets_its_message_or_note() {
    let (dir, file) = pressed("empty-before-staging", &recipe("branches = [\"nJet\"]", ""));
    // A folder nothing can be made in, as on a read-only filesystem: a full file, an empty one and one of no rows.
    let folder = dir.join("shut");
    std::fs::create_dir(&folder).unwrap();
    std::fs::copy(dir.join(&file), folder.join("full.rvr")).unwrap();
    std::fs::write(folder.join("empty.rvr"), corpus("fixture-empty.rvr")).unwrap();
    std::fs::write(folder.join("rowless.rvr"), corpus("fixture-rowless.rvr")).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let empty = |name: &str, disk: bool| {
        let path = folder.join(name).to_str().unwrap().to_owned();
        let set: Vec<(&str, &str)> = if disk { vec![("RVR_READ_ONLY_DISK", &path)] } else { Vec::new() };
        rvr_with(&dir, &set, &["empty", &format!("shut/{name}"), "-f"])
    };
    let said: Vec<(bool, (i32, String))> = [true, false].into_iter().flat_map(|disk| ["full.rvr", "empty.rvr", "rowless.rvr"].map(|name| (disk, empty(name, disk)))).collect();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    let full = "shut/full.rvr lies on a read-only filesystem: rvr empty gives back the space only of a file you may write (mount it to be written first)";
    let staged = "shut/full.rvr could not be staged in shut/full.rvr.partial";
    let notes = ["shut/empty.rvr is empty already.", "shut/rowless.rvr has no rows, so its arrays hold no bytes: there is nothing to empty."];
    for (disk, (code, said)) in &said {
        let ok = match () {
            _ if said.contains(notes[0]) || said.contains(notes[1]) => *code == 0,
            _ if *disk => *code == 1 && said.contains(full),
            _ => *code == 1 && said.contains(staged) && said.contains("Make its folder writable"),
        };
        assert!(ok, "{disk}: {said}");
    }
    let told = |note: &str| said.iter().filter(|(_, (_, said))| said.contains(note)).count();
    assert_eq!((told(notes[0]), told(notes[1]), told(full), told(staged)), (2, 2, 1, 1), "{said:?}");
    assert_eq!(std::fs::read(folder.join("full.rvr")).unwrap(), std::fs::read(dir.join(&file)).unwrap());
}

// SPEC 8.12
#[test]
#[cfg(debug_assertions)]
fn the_journal_of_a_change_to_a_file_with_no_array_bytes_names_it_by_its_old_and_new_tails() {
    let dir = Study::new("journal-tails");
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    for (name, other) in [("fixture-empty.rvr", "fixture-empty-derived.rvr"), ("fixture-rowless.rvr", "fixture-rowless-derived.rvr")] {
        let path = dir.join(name);
        let old = corpus(name);
        assert_eq!(parts(&old).1.len(), 0, "{name}: no array bytes");
        // A tool change made but for letting its journal go (stopped after step 3), and its journal.
        std::fs::write(&path, &old).unwrap();
        assert_eq!(rvr_with(&dir, &[("RVR_STOP_CHANGE", "3")], &["tool", name, "k", "--set", "data.json"]).0, 99);
        let (new, kept) = (std::fs::read(&path).unwrap(), std::fs::read(journal(&path)).unwrap());
        assert!(new.len() > old.len(), "{name}");
        let cut_short = |bytes: &[u8]| {
            std::fs::write(&path, bytes).unwrap();
            std::fs::write(journal(&path), &kept).unwrap();
            rvr(&dir, &["validate", name])
        };
        // The new tail whole: the change finished, its journal let go, the file untouched.
        let (code, said) = cut_short(&new);
        assert!(code == 0 && said.contains(&format!("Let go {name}-journal: the change to {name} had finished, so {name} is as the change made it.")), "{name}: {said}");
        assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (new.clone(), false), "{name}");
        // Torn: some of the new tail written over the old, and past the old length a page not kept.
        let half = (old.len() - 8) / 2;
        let mut torn = old.clone();
        torn[8..8 + half].copy_from_slice(&new[8..8 + half]);
        let mut paged = new.clone();
        paged[8 + half..old.len()].copy_from_slice(&old[8 + half..]);
        paged[old.len()..].fill(0);
        for bytes in [&torn, &paged] {
            let (code, said) = cut_short(bytes);
            assert!(code == 0 && said.contains(&format!("Put {name} back as it was: a change to it was cut short.")), "{name}: {said}");
            assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (old.clone(), false), "{name}");
        }
        // Another file with no array bytes copied in place over it, or the old one with a byte of
        // its tail other than either tail's: refused, and neither it nor the journal is touched.
        let mut odd = old.clone();
        assert_eq!((old[8], new[8]), (b'{', b'{'));
        odd[8] = b'x';
        for bytes in [corpus(other), odd] {
            let (code, said) = cut_short(&bytes);
            assert!(code == 1 && said.contains(&format!("{name}-journal is not of this {name} ({name} was replaced since a change to it was cut short)")), "{name}: {said}");
            assert_eq!((std::fs::read(&path).unwrap(), std::fs::read(journal(&path)).unwrap()), (bytes, kept.clone()), "{name}");
        }
        std::fs::remove_file(journal(&path)).unwrap();
    }
}

// SPEC 8.11; FORMAT 1, 11
#[test]
fn a_header_the_rules_refuse_is_refused_before_rvr_tool_asks_and_a_misnamed_key_that_breaks_it_is_dropped() {
    let (dir, file) = pressed("tool-refused-first", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let before = std::fs::read(&path).unwrap();
    let (text, arrays) = parts(&before);
    let text = std::str::from_utf8(text).unwrap();
    let with = |text: &str, key: &str| format!("{},\"{key}\":{{}}}}", &text[..text.len() - 1]);
    std::fs::write(dir.join("data.json"), "{}").unwrap();
    // Broken outside the key by a rule of the parse (-0) or of the header's checks (a slip of data's name).
    let minus = with(&text.replacen("\"made\":{", "\"made\":{\"x\":-0,", 1), "k");
    for (broken, said) in [(minus.as_str(), "made.x is written -0"), (&with(&with(text, "dta"), "k"), "the top-level key \"dta\" is a slip of data")] {
        let broken = framed(broken.as_bytes(), arrays);
        std::fs::write(&path, &broken).unwrap();
        for args in [&["tool", &file, "k", "--set", "data.json"][..], &["tool", &file, "k", "--drop"]] {
            let (code, told) = at_terminal(&dir, args, "yes");
            assert!(code == 1 && told.contains(said) && !told.contains("[y/N]"), "{args:?}: {told}");
            assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (broken.clone(), false), "{args:?}");
        }
    }
    // A key misnamed (a slip of the format's own, or no tool key's name) keeps the file from opening; dropped, it opens.
    for key in ["dta", "a b"] {
        std::fs::write(&path, framed(with(text, key).as_bytes(), arrays)).unwrap();
        assert_eq!(rvr(&dir, &["validate", &file]).0, 1, "{key}");
        let (code, said) = rvr(&dir, &["tool", &file, key, "--drop", "-f"]);
        assert!(code == 0 && said.contains(&format!("Dropped the tool key {key} of {file} (2 bytes).")), "{key}: {said}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{key}");
    }
    let (code, said) = rvr(&dir, &["tool", &file, "data", "--drop", "-f"]);
    assert!(code == 1 && said.contains("data is the format's own key, not a tool key"), "{said}");
}

// SPEC 8.11
#[test]
fn data_that_is_not_json_is_told_against_the_file_it_came_from() {
    let (dir, file) = pressed("tool-data-named", &recipe("branches = [\"nJet\"]", ""));
    for (text, said) in [("{\n  \"a\": 1,\n  \"b\" 2\n}", "data.json: the tool key k is not JSON as docs/FORMAT.md §1 reads it (expected `:` at line 3 column 7)"), ("{\n  \"a\": 1,\n  \"b\": -0\n}", "data.json: the tool key k breaks docs/FORMAT.md §1: k.b is written -0 (line 3, column 9)")] {
        std::fs::write(dir.join("data.json"), text).unwrap();
        let (code, told) = rvr(&dir, &["tool", &file, "k", "--set", "data.json"]);
        assert!(code == 1 && told.contains(said) && !told.contains(&format!("{file}:")), "{told}");
    }
}

// SPEC 8.11
#[test]
fn a_tool_key_written_out_over_a_file_replaces_it_whole_keeping_its_permissions() {
    let (dir, file) = pressed("tool-get-whole", &recipe("branches = [\"nJet\"]", ""));
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--set", "data.json"]).0, 0);
    let out = dir.join("out.json");
    std::fs::write(&out, "a longer text written by hand, more than the key holds\n").unwrap();
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o640)).unwrap();
    let ino = inode(&out);
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--get", "out.json", "-f"]).0, 0);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "{\"n\":1}\n");
    assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o640);
    assert_ne!(inode(&out), ino, "written beside it, then moved over it");
    let left: Vec<_> = std::fs::read_dir(&*dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("out.json.")).collect();
    assert!(left.is_empty(), "{left:?}");
}

// SPEC 8.11
#[test]
fn a_tool_key_is_written_out_and_sized_as_the_header_holds_its_bytes() {
    let (dir, file) = pressed("tool-bytes", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let bytes = std::fs::read(&path).unwrap();
    let (text, arrays) = parts(&bytes);
    // Written by hand: spaced, a trailing zero, an escape, none of which a writer makes.
    let held = r#"{"x":1.50, "s":"\u00e9"}"#;
    let text = format!("{},\"k\":{held}}}", std::str::from_utf8(&text[..text.len() - 1]).unwrap());
    std::fs::write(&path, framed(text.as_bytes(), arrays)).unwrap();
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    let n = held.len();
    assert_eq!(rvr(&dir, &["tool", &file]), (0, format!("{file} has 1 tool key:\n  k ({n} bytes)\n")));
    assert_eq!(rvr(&dir, &["tool", &file, "k", "--get"]), (0, format!("{held}\n")));
    let (code, said) = rvr(&dir, &["tool", &file, "k", "--get", "out.json"]);
    assert!(code == 0 && said.contains(&format!("({n} bytes)")), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("out.json")).unwrap(), format!("{held}\n"));
    let (code, said) = rvr(&dir, &["tool", &file, "k", "--drop", "-f"]);
    assert!(code == 0 && said.contains(&format!("Dropped the tool key k of {file} ({n} bytes).")), "{said}");
}

// SPEC 8.11
#[test]
fn a_tool_key_held_twice_is_listed_per_copy_and_dropped_whole_and_one_a_terminal_cannot_show_is_quoted() {
    let (dir, file) = pressed("tool-twice", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let bytes = std::fs::read(&path).unwrap();
    let (text, arrays) = parts(&bytes);
    // Written by another program: k twice, which FORMAT 1 refuses, and a name with a bell in it.
    let text = format!(r#"{},"k":{{"a":1}},"k":{{}},"b\u0007":{{}}}}"#, std::str::from_utf8(&text[..text.len() - 1]).unwrap());
    std::fs::write(&path, framed(text.as_bytes(), arrays)).unwrap();
    let (code, said) = rvr(&dir, &["tool", &file]);
    assert!(code == 0 && said.starts_with(&format!("{file} has 3 tool keys:\n  k (7 bytes)\n  k (2 bytes)\n  \"b\\u0007\" (2 bytes)\n")), "{said}");
    assert!(said.contains(&format!(r#"rowver.drop_tool("{file}", "k", "b\u0007") in Python ("b\u0007" holds a character outside printable ASCII)"#)), "{said}");
    let (code, said) = rvr(&dir, &["tool", &file, "k", "--get"]);
    assert!(code == 1 && said.contains(&format!(r#"holds the tool key k 2 times, which FORMAT.md §1 refuses, so rvr tool cannot write out one: rowver.drop_tool("{file}", "k")"#)), "{said}");
    // Dropped alone, k leaves a header the bell still breaks; with it, every copy goes.
    let (code, said) = rvr(&dir, &["tool", &file, "k", "--drop", "-f"]);
    assert!(code == 1 && said.contains("b\\u{7}"), "{said}");
    let (code, said) = rvr(&dir, &["tool", &file, "k", "b\u{7}", "--drop", "-f"]);
    assert!(code == 0 && said.contains(&format!("Dropped the tool keys k (9 bytes), \"b\\u0007\" (2 bytes) of {file}.")), "{said}");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // So is one a terminal would show turned about, or past U+FFFF, as Python writes it.
    let bytes = std::fs::read(&path).unwrap();
    let (text, arrays) = parts(&bytes);
    let text = format!("{},\"z\u{202e}\u{1f600}\":{{}}}}", std::str::from_utf8(&text[..text.len() - 1]).unwrap());
    std::fs::write(&path, framed(text.as_bytes(), arrays)).unwrap();
    let (code, said) = rvr(&dir, &["tool", &file]);
    assert!(code == 0 && said.contains("  \"z\\u202e\\U0001f600\" (2 bytes)") && said.contains(&format!(r#"rowver.drop_tool("{file}", "z\u202e\U0001f600")"#)), "{said}");
}

/// rvr in `dir`, started with `args` and holding the first of its two locks while the file `hold`
/// is there (RVR_HOLD): the running rvr, once it holds it, and what it goes on to complain.
fn holding(dir: &Path, hold: &Path, args: &[&str]) -> (std::process::Child, std::sync::mpsc::Receiver<String>) {
    use std::io::BufRead;
    std::fs::write(hold, "").unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .current_dir(dir)
        .env("RVR_CONFIG", dir.join(".rvr-config"))
        .env("RVR_HOLD", hold)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (send, lines) = std::sync::mpsc::channel();
    let stderr = std::io::BufReader::new(child.stderr.take().unwrap());
    std::thread::spawn(move || stderr.lines().map_while(Result::ok).for_each(|line| drop(send.send(line))));
    loop {
        match lines.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(line) if line.contains("as RVR_HOLD asks") => return (child, lines),
            Ok(_) => {}
            Err(e) => panic!("{args:?} never held its first lock: {e}"),
        }
    }
}

// SPEC 7.5.2, 8.12
#[test]
fn a_press_and_an_empty_of_one_file_take_its_lock_first_and_the_one_second_is_refused_naming_it() {
    let (dir, file) = pressed("lock-order", &recipe("branches = [\"nJet\"]", ""));
    let hold = dir.join("hold");
    for (first, second) in [(vec!["press"], vec!["empty", &file, "-f"]), (vec!["empty", &file, "-f"], vec!["press"])] {
        let (mut child, lines) = holding(&dir, &hold, &first);
        let (code, said) = rvr(&dir, &second);
        assert!(code == 1 && said.contains(BUSY) && !said.contains("another rvr press"), "{second:?}: {said}");
        std::fs::remove_file(&hold).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success(), "{first:?}: {}", lines.try_iter().collect::<Vec<_>>().join("\n"));
    }
    assert!(rowver::rvr::Reader::open(dir.join(&file)).unwrap().empty());
}

// SPEC 8.12
#[test]
fn rvr_status_and_a_reader_read_a_file_a_press_holds_locked_with_no_journal_beside_it() {
    let (dir, file) = pressed("held-read", &recipe("branches = [\"nJet\"]", ""));
    let hold = dir.join("hold");
    let (mut child, lines) = holding(&dir, &hold, &["press"]);
    assert!(!journal(&dir.join(&file)).exists());
    let (code, said) = rvr(&dir, &["status"]);
    assert!(code == 0 && !said.contains("being changed"), "{said}");
    let (code, said) = rvr(&dir, &["validate", &file]);
    assert!(code == 0 && said.contains(": valid"), "{said}");
    let reader = rowver::rvr::Reader::open(dir.join(&file)).unwrap();
    assert!(reader.column("nJet").unwrap().is_some());
    std::fs::remove_file(&hold).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "{}", lines.try_iter().collect::<Vec<_>>().join("\n"));
}

// SPEC 8.12
#[test]
#[cfg(debug_assertions)]
fn a_press_never_writes_over_what_another_program_filled_an_empty_target_with_while_it_held_the_lock() {
    use std::io::Write;
    let (dir, file) = pressed("press-filled", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(&path, "").unwrap();
    let hold = dir.join("hold");
    let (mut child, lines) = holding(&dir, &hold, &["press"]);
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().write_all(b"another program's bytes").unwrap();
    std::fs::remove_file(&hold).unwrap();
    let status = child.wait().unwrap();
    let said = lines.iter().collect::<Vec<_>>().join("\n");
    assert!(!status.success() && said.contains("is not an .rvr file (it does not begin as one does), and rvr press writes only over an .rvr file"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), b"another program's bytes");
}

// SPEC 8.12
#[test]
#[cfg(debug_assertions)]
fn a_press_never_moves_its_file_over_one_another_program_moved_there_while_it_held_the_lock() {
    let (dir, file) = pressed("press-replaced", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    let hold = dir.join("hold");
    let (mut child, lines) = holding(&dir, &hold, &["press", "--full"]);
    std::fs::write(dir.join("other.rvr"), b"another program's file").unwrap();
    std::fs::rename(dir.join("other.rvr"), &path).unwrap();
    std::fs::remove_file(&hold).unwrap();
    let status = child.wait().unwrap();
    let said = lines.iter().collect::<Vec<_>>().join("\n");
    assert!(!status.success() && said.contains(&format!("{file} was replaced by another program while rvr held it: run the command again")), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), b"another program's file");
}

// SPEC 8.12
#[test]
fn a_press_in_place_of_derived_columns_alone_journals_the_length_it_leaves() {
    let (dir, file) = pressed("derived-only", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    // A longer copy of the file: its arrays, and a larger header.
    std::fs::write(dir.join("big.json"), format!(r#"{{"pad": "{}"}}"#, "x".repeat(100_000))).unwrap();
    std::fs::copy(&path, dir.join("copy.rvr")).unwrap();
    assert_eq!(rvr(&dir, &["tool", "copy.rvr", "pad", "--set", "big.json"]).0, 0);
    let longer = std::fs::read(dir.join("copy.rvr")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"nJet\"]", "twice = \"nJet * 2\"")).unwrap();
    let (code, said) = rvr_with(&dir, &[("RVR_STOP_CHANGE", "2")], &["press"]);
    assert_eq!(code, 99, "{said}");
    let ino = inode(&path);
    std::fs::write(&path, &longer).unwrap();
    assert_eq!(inode(&path), ino);
    let (code, said) = rvr(&dir, &["header", &file]);
    assert!(code == 1 && said.contains(&format!("{file}-journal is not of this {file}")), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), longer);
    std::fs::remove_file(journal(&path)).unwrap();
}

/// The corpus file `name` of tests/conformance/rvr/valid.
fn corpus(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/rvr/valid").join(name)).unwrap()
}

// SPEC 8.11, 8.12; FORMAT 1
#[test]
fn tool_sets_and_drops_a_key_of_an_empty_file_and_of_a_rowless_one() {
    let dir = Study::new("tool-empty");
    std::fs::write(dir.join("data.json"), r#"{"n": 1}"#).unwrap();
    for name in ["fixture-empty.rvr", "fixture-empty-derived.rvr", "fixture-rowless.rvr"] {
        let before = corpus(name);
        std::fs::write(dir.join(name), &before).unwrap();
        let (code, said) = rvr(&dir, &["tool", name, "k", "--set", "data.json"]);
        assert_eq!(code, 0, "{name}: {said}");
        let (code, said) = rvr(&dir, &["validate", name]);
        assert!(code == 0 && said.contains(": valid"), "{name}: {said}");
        assert_eq!(header(&dir, name)["k"], json!({"n": 1}), "{name}");
        assert_eq!(parts(&std::fs::read(dir.join(name)).unwrap()).1, parts(&before).1, "{name}");
        assert_eq!(rvr(&dir, &["tool", name, "k", "--drop", "-f"]).0, 0, "{name}");
        assert_eq!(std::fs::read(dir.join(name)).unwrap(), before, "{name}");
    }
}

/// The bytes ranges of `file`'s arrays a debug build read, as RVR_READ_LOG logged them.
#[cfg(debug_assertions)]
fn reads(log: &Path, file: &Path) -> Vec<(u64, u64)> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let file = std::fs::canonicalize(file).unwrap().to_string_lossy().into_owned();
    text.lines()
        .filter(|line| line.split('\t').next().and_then(|p| std::fs::canonicalize(p).ok()).is_some_and(|p| p.to_string_lossy() == file))
        .map(|line| {
            let mut words = line.split('\t').skip(1).map(|w| w.parse().unwrap());
            (words.next().unwrap(), words.next().unwrap())
        })
        .collect()
}

/// Where an array record lays its bytes in the file: from byte 8 + `at`, `size` of them.
#[cfg(debug_assertions)]
fn span(record: &Value) -> (u64, u64) {
    (8 + record["at"].as_u64().unwrap(), record["size"].as_u64().unwrap())
}

// SPEC 8.2, 8.3, 8.8, 8.12; FORMAT 7
#[test]
#[cfg(debug_assertions)]
fn a_column_added_to_the_recipe_is_written_in_place_reading_only_what_it_uses() {
    let first = recipe("branches = [\"weight\", \"nJet\"]", "");
    let (dir, file) = pressed("added-in-place", &first);
    let path = dir.join(&file);
    let (before, ino) = (std::fs::read(&path).unwrap(), inode(&path));
    let old = header(&dir, &file);
    let added = recipe("branches = [\"weight\", \"nJet\", \"Jet_pt\"]\nht = \"sum(Jet_pt)\"", "per_jet = \"ht / nJet\"\n");
    std::fs::write(dir.join("recipe.toml"), &added).unwrap();
    let log = dir.join("reads.log");
    let (code, said) = rvr_with(&dir, &[("RVR_READ_LOG", log.to_str().unwrap())], &["press"]);
    assert!(code == 0 && said.contains("Reading only Jet_pt, ht for those events") && said.contains("in place"), "{said}");
    let after = std::fs::read(&path).unwrap();
    assert_eq!((inode(&path), &after[..before_header(&before).len()]), (ino, &before_header(&before)[..]), "the arrays it had are where they were");
    // Of the arrays it had, it read only the entries (to know the events it kept) and nJet, which
    // the new derived column reads; then the new arrays, after them.
    let used: Vec<(u64, u64)> = [&old["data"]["sample"], &old["data"]["entry"], &old["data"]["cols"][1]].into_iter().map(span).collect();
    let new = 8 + parts(&before).1.len() as u64;
    let read = reads(&log, &path);
    assert!(read.contains(&used[1]) && read.iter().all(|&(at, n)| at >= new || used.iter().any(|&(from, size)| at >= from && at + n <= from + size)), "{read:?}");
    let now = header(&dir, &file);
    let names: Vec<&str> = now["data"]["cols"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["weight", "nJet", "Jet_pt", "ht"], "the records in the recipe's order, whatever the order of their bytes");
    assert!(now["data"]["cols"][2]["offsets"]["at"].as_u64().unwrap() >= parts(&before).1.len() as u64);
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // The same as a full press but for where its new arrays lie.
    let (code, said) = rvr(&dir, &["press", "--full", "--out", "full.rvr"]);
    assert_eq!(code, 0, "{said}");
    let full = header(&dir, "full.rvr");
    let unplaced = |data: &Value| {
        let mut data = data.clone();
        for record in data["cols"].as_array_mut().unwrap() {
            for part in [None, Some("offsets"), Some("content")] {
                let held = match part { None => &mut *record, Some(p) => &mut record[p] };
                if let Some(held) = held.as_object_mut() {
                    held.remove("at");
                }
            }
        }
        for key in ["sample", "entry"] {
            data[key].as_object_mut().unwrap().remove("at");
        }
        data
    };
    assert_eq!(unplaced(&now["data"]), unplaced(&full["data"]));
    assert_eq!(now["derived"], full["derived"]);
    assert_eq!(rvr(&dir, &["diff", &file, "full.rvr"]).0, 0);
    // rvr derive adds and drops a derived column in place too; a column taken out of the recipe
    // writes a new file.
    let (code, said) = rvr(&dir, &["derive", &file, "twice = ht * 2"]);
    assert!(code == 0 && said.contains("Added twice to"), "{said}");
    assert_eq!(rvr(&dir, &["derive", &file, "--drop", "twice"]).0, 0);
    assert_eq!(inode(&path), ino);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"Jet_pt\"]\nht = \"sum(Jet_pt)\"", "")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && !said.contains("in place"), "{said}");
    assert_ne!(inode(&path), ino, "a column dropped: a new file, moved over it");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
    // A column added while those kept change order is not only added: a new file again.
    let ino = inode(&path);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"Jet_pt\", \"weight\", \"nJet\"]\nht = \"sum(Jet_pt)\"", "")).unwrap();
    let (code, said) = rvr(&dir, &["press"]);
    assert!(code == 0 && !said.contains("in place") && said.contains("Reading only nJet for those events"), "{said}");
    assert_ne!(inode(&path), ino, "columns in another order: a new file, moved over it");
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
}

// SPEC 8.2, 8.12
#[test]
#[cfg(debug_assertions)]
fn a_press_in_place_reads_nothing_for_the_derived_columns_it_keeps() {
    let (dir, file) = pressed("kept-derived", &recipe("branches = [\"weight\", \"nJet\"]", "twice = \"nJet * 2\"\n"));
    let path = dir.join(&file);
    let old = header(&dir, &file);
    std::fs::write(dir.join("recipe.toml"), recipe("branches = [\"weight\", \"nJet\", \"Jet_pt\"]", "twice = \"nJet * 2\"\n")).unwrap();
    let log = dir.join("reads.log");
    let (code, said) = rvr_with(&dir, &[("RVR_READ_LOG", log.to_str().unwrap())], &["press"]);
    assert!(code == 0 && said.contains("in place"), "{said}");
    // nJet, which only the derived column it keeps reads, is not read; its record is kept.
    let (from, size) = span(&old["data"]["cols"][1]);
    let read = reads(&log, &path);
    // It reads the entries, to know the events the file kept: so an empty log fails.
    assert!(read.contains(&span(&old["data"]["entry"])) && read.iter().all(|&(at, n)| at + n <= from || at >= from + size), "{read:?}");
    assert_eq!(header(&dir, &file)["derived"], old["derived"]);
    assert_eq!(rvr(&dir, &["validate", &file]).0, 0);
}

/// `dir`'s file written again as if another build of rvr, with commit `commit`, made it.
fn made_by(dir: &Path, file: &str, commit: &str) {
    made_by_with(dir, file, |toolchain| toolchain["commit"] = commit.into());
}

/// `dir`'s file written again as if another build of rvr made it, its `made.toolchain` changed by `edit`.
fn made_by_with(dir: &Path, file: &str, edit: impl FnOnce(&mut Value)) {
    let path = dir.join(file);
    reheadered(&path, &path, |h| {
        edit(&mut h["made"]["toolchain"]);
        h["made"]["sha256"] = rowver::write::made_checksum(&h["made"]).unwrap().into();
        h.as_object_mut().unwrap().remove("signature");
    });
}

// SPEC 5.1.6, 5.1.7, 8.5, 8.6
#[test]
#[cfg(debug_assertions)]
fn a_file_made_by_another_build_is_pressed_again_whole_only_when_asked() {
    let (dir, file) = pressed("another-build", &recipe("branches = [\"nJet\"]", ""));
    let other = "0123456789abcdef0123456789abcdef01234567";
    made_by(&dir, &file, other);
    let before = std::fs::read(dir.join(&file)).unwrap();
    let this = rvr(&dir, &["--version"]).1;
    let commit = this.trim().rsplit("commit ").next().unwrap().trim_end_matches(')').to_owned();
    let said_both = format!("{file} was made by rvr {} (commit {other}); this is rvr {} (commit {commit}).", env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_VERSION"));
    // Away from a terminal it stops, saying to add --full, having read none of the file's arrays.
    let log = dir.join("reads.log");
    let (code, said) = rvr_with(&dir, &[("RVR_READ_LOG", log.to_str().unwrap())], &["press"]);
    assert!(code == 2 && said.contains(&said_both) && said.contains("--full"), "{said}");
    assert_eq!(std::fs::read(dir.join(&file)).unwrap(), before);
    assert_eq!(reads(&log, &dir.join(&file)), [], "the build is asked about before any array is read");
    // The log logs what reads the file's rows, so the empty one above checked something.
    assert_eq!(rvr_with(&dir, &[("RVR_READ_LOG", log.to_str().unwrap())], &["validate", &file]).0, 0);
    let rows = [&header(&dir, &file)["data"]["sample"], &header(&dir, &file)["data"]["entry"]].map(span);
    assert!(rows.iter().all(|r| reads(&log, &dir.join(&file)).contains(r)), "{rows:?}");
    // At one, it asks; a no presses nothing.
    let (code, said) = at_terminal(&dir, &["press"], "n");
    assert!(code == 1 && said.contains(&said_both) && said.contains("Press every column? [y/N] "), "{said}");
    assert_eq!(std::fs::read(dir.join(&file)).unwrap(), before);
    let (code, said) = at_terminal(&dir, &["press"], "y");
    assert!(code == 0 && said.contains("kept after each cut"), "{said}");
    assert_eq!(header(&dir, &file)["made"]["toolchain"]["commit"], commit.as_str());
    // --full presses every column without asking.
    made_by(&dir, &file, other);
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 0 && !said.contains("[y/N]"), "{said}");
    // The same version and commit with another compiler: each build's compiler and platform said.
    let this = header(&dir, &file)["made"]["toolchain"].clone();
    made_by_with(&dir, &file, |toolchain| toolchain["compiler"] = "rustc 1.0.0 (another)".into());
    let (code, said) = rvr(&dir, &["press"]);
    let named = |compiler: &str| format!("rvr {} (commit {commit}, {compiler}, {})", env!("CARGO_PKG_VERSION"), this["target"].as_str().unwrap());
    let said_both = format!("{file} was made by {}; this is {}.", named("rustc 1.0.0 (another)"), named(this["compiler"].as_str().unwrap()));
    assert!(code == 2 && said.contains(&said_both), "{said}");
}

// SPEC 9.6, 9.6.1
#[test]
fn reproduce_texts_writes_the_three_texts_and_presses_nothing() {
    let (dir, file) = pressed("texts", &recipe("branches = [\"nJet\"]", ""));
    let h = header(&dir, &file);
    let (code, said) = rvr(&dir, &["reproduce", &file, "again", "--texts"]);
    assert_eq!(code, 0, "{said}");
    let mut written: Vec<String> = std::fs::read_dir(dir.join("again")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    written.sort();
    assert_eq!(written, ["recipe.toml", "samples.lock", "samples.toml"]);
    for (name, key) in [("recipe.toml", "recipe"), ("samples.toml", "samples"), ("samples.lock", "lock")] {
        assert_eq!(std::fs::read_to_string(dir.join("again").join(name)).unwrap(), h["made"][key]["text"].as_str().unwrap());
    }
    // A folder holding other texts is refused, as a reproduce refuses it.
    std::fs::write(dir.join("again/recipe.toml"), "# another\n").unwrap();
    let (code, said) = rvr(&dir, &["reproduce", &file, "again", "--texts"]);
    assert!(code == 1 && said.contains("recipe.toml"), "{said}");
}

// SPEC 8.12; FORMAT 11
#[test]
fn a_press_stops_over_a_file_its_tool_keys_alone_keep_from_opening() {
    let (dir, file) = pressed("tools-shut", &recipe("branches = [\"nJet\"]", ""));
    let path = dir.join(&file);
    std::fs::write(dir.join("k.json"), "{\"a\":1}").unwrap();
    assert_eq!(rvr(&dir, &["tool", &file, "keep", "--set", "k.json"]).0, 0);
    reheadered(&path, &path, |header| header["bad"] = json!(3));
    let before = std::fs::read(&path).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains(&format!("{file} does not open: ")) && said.contains(&format!("A press carries over the tool keys of the file it replaces, so it stops: drop the tool keys that break it (rvr tool {file} names them), or remove the file, then press again.")), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(rvr(&dir, &["tool", &file, "bad", "--drop", "-f"]).0, 0);
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 0 && rvr(&dir, &["tool", &file]).1.contains("  keep (7 bytes)"), "{said}");
    // A file kept from opening by its arrays, not its tool keys, is pressed over, and its tool keys
    // kept; one its tool keys keep shut too gets the stop, and is left as it was.
    let damaged = |path: &std::path::Path| {
        let whole = std::fs::read(path).unwrap();
        let (text, arrays) = parts(&whole);
        std::fs::write(path, framed(text, &[arrays, &[0]].concat())).unwrap();
    };
    damaged(&path);
    assert_eq!(rvr(&dir, &["validate", &file]).0, 1);
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 0 && !said.contains("carries over the tool keys"), "{said}");
    assert!(rvr(&dir, &["tool", &file]).1.contains("  keep (7 bytes)"));
    // A key named twice is dropped once.
    let (code, said) = rvr(&dir, &["tool", &file, "keep", "keep", "--drop", "-f"]);
    assert!(code == 0 && said.contains("Dropped the tool key keep of"), "{said}");
    assert_eq!(rvr(&dir, &["tool", &file, "keep", "--set", "k.json"]).0, 0);
    reheadered(&path, &path, |header| header["bad"] = json!(3));
    damaged(&path);
    let before = std::fs::read(&path).unwrap();
    let (code, said) = rvr(&dir, &["press", "--full"]);
    assert!(code == 1 && said.contains("so it stops"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
