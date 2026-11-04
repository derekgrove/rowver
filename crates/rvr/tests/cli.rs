//! The rvr command on files written here: validate, header, dump, and diff naming what changed.

use rowver::study::lock::{Branch, Branches, File, Lock, Sample, Shape};
use rowver::write::{array, made_checksum, sample_cols, stream, text_checksum, write_laid, Rows, Sample as Rowed, Source, Values};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

mod common;
use common::Study as Scratch;

/// A small pressed file: samples A (3 rows) and B (2 rows), a flat and a jagged column, and an
/// `xsec` sample column. `met_b` is MET's first value in sample B; `xsec_b` is B's cross-section.
fn pressed(path: &Path, met_b: f32, xsec_b: f64) {
    let sample = |name: &str, rows| Rowed { name: name.into(), label: format!("Sample {name}"), rows };
    let files = vec![
        json!({"sample": "A", "path": "./a.root", "rows": 3}),
        json!({"sample": "B", "path": "./b.root", "rows": 2}),
    ];
    let rows = Rows { samples: vec![sample("A", 3), sample("B", 2)], files, entry: Source::Values(Values::Flat(array(&[0_i64, 1, 2, 0, 1]))) };
    let columns = vec![
        ("MET".to_owned(), Source::Values(Values::Flat(array(&[10_f32, 20., 30., met_b, 50.])))),
        ("Jet_pt".to_owned(), Source::Values(Values::Jagged { counts: vec![1, 0, 2, 2, 1], content: array(&[40_f32, 50., 60., 70., 80., 90.]) })),
    ];
    let (mut data, laid) = stream(rows, columns, 1, path).unwrap();
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"Events\"\n\n[eras]\n\n\
        [[sample]]\nname = \"A\"\nlabel = \"Sample A\"\nfiles = \"./a.root\"\nmetadata = {{ xsec = 1.5 }}\n\n\
        [[sample]]\nname = \"B\"\nlabel = \"Sample B\"\nfiles = \"./b.root\"\nmetadata = {{ xsec = {xsec_b:?} }}\n");
    let list = Branches::new(vec![
        Branch { name: "MET".into(), kind: "f4".into(), shape: Shape::Flat, why: None },
        Branch { name: "Jet_pt".into(), kind: "f4".into(), shape: Shape::Jagged, why: None },
    ]);
    let made_samples = rowver::study::samples::read(&samples).unwrap();
    let locked: Vec<Sample> = made_samples.iter().zip([3, 2]).enumerate().map(|(n, (s, entries))| Sample {
        name: s.name.clone(), label: s.label.clone(), asked: s.files.clone(), tree: s.tree.clone(),
        branches: list.sha256.clone(), events: s.events, events_total: 0, events_read: 0, metadata: s.metadata.clone(),
        files: vec![File { at: s.files[0].clone(), bytes: 4000, adler32: Some("0a1b2c3d".into()), uuid: format!("31e851e4-b994-11f1-ba5c-{n:012x}"), entries, stop: None, take: None }],
    }.planned(s.events)).collect();
    let lock = Lock { resolved: "2026-10-16T00:00:00Z".into(), samples_toml_sha256: text_checksum(&samples), samples: locked, branches: vec![list] }.write().unwrap();
    let lock = Lock::read(&lock).map(|read| (lock, read)).unwrap();
    let text = |text: &str| json!({"sha256": text_checksum(text), "text": text});
    let mut recipe = text("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"MET\", \"Jet_pt\"]\n\n[totals]\n\n[derived]\n");
    recipe["format"] = "rvr-recipe".into();
    recipe["columns"] = json!(["MET", "Jet_pt"]);
    let mut files = serde_json::Map::new();
    for sample in &lock.1.samples {
        let file = &sample.files[0];
        let mut metadata = sample.metadata.clone();
        metadata.insert("events_read".into(), sample.events_read.into());
        metadata.insert("events_total".into(), sample.events_total.into());
        files.insert(sample.name.clone(), json!({"events": {"choose": "all"}, "files": {file.at.clone(): {"tree": "Events", "uuid": file.uuid, "entries": file.entries}}, "metadata": metadata}));
    }
    let mut made = json!({
        "study": "cli-test", "built": "2026-10-16T00:00:00Z", "by": "rvr test",
        "toolchain": {"rvr": "15.0.0", "rowver": "15", "commit": "unknown", "rustc": "rustc 1.91.0", "target": "test"},
        "recipe": recipe, "samples": text(&samples), "lock": text(&lock.0), "files": files,
    });
    made["sha256"] = made_checksum(&made).unwrap().into();
    data["sample_cols"] = sample_cols(&["A", "B"], &made["files"], &["MET", "Jet_pt"])
        .unwrap()
        .into();
    write_laid(path, &json!({"rowver": 15, "data": data, "made": made}), laid).unwrap();
}

/// The bytes of a file of this header and arrays, whatever the header holds.
fn framed(header: &Value, arrays: &[u8]) -> Vec<u8> {
    let text = serde_json::to_string(header).unwrap();
    let mut bytes = b"\x89RVR\r\n\x1a\n".to_vec();
    bytes.extend((text.len() as u32).to_le_bytes());
    bytes.extend(text.as_bytes());
    bytes.extend(arrays);
    bytes
}

/// Run rvr; its exit status and standard output.
fn rvr(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .env("RVR_CONFIG", std::env::temp_dir().join("rvr-cli-no-config"))
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn diff_names_the_column_and_sample_that_changed() {
    let dir = Scratch::new("diff");
    let (a, b) = (dir.0.join("a.rvr"), dir.0.join("b.rvr"));
    pressed(&a, 40., 2.5);
    pressed(&b, 41., 3.5);
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
    assert_eq!(rvr(&["diff", a, a]).0, 0);
    let (code, said) = rvr(&["diff", a, b]);
    assert_eq!(code, 1);
    assert_eq!(
        said,
        format!("column MET: differs in sample B\nsample column xsec: differs in sample B: 2.5 in {a}, 3.5 in {b}\n")
    );
}

#[test]
fn validate_header_and_dump_read_a_pressed_file() {
    let dir = Scratch::new("read");
    let path = dir.0.join("one.rvr");
    pressed(&path, 40., 2.5);
    let file = path.to_str().unwrap();
    let (code, said) = rvr(&["validate", file]);
    assert_eq!(
        (code, said),
        (0, format!("{file}: valid, 5 rows, 2 columns, read all 5 entries, not signed\n"))
    );
    // A flag it does not know is refused (on stderr), not read past.
    assert_eq!(rvr(&["validate", file, "--parent", file]), (2, String::new()));
    let (code, said) = rvr(&["header", file]);
    let header: Value = serde_json::from_str(&said).unwrap();
    assert_eq!((code, header["data"]["rows"].as_u64()), (0, Some(5)));
    let (code, said) = rvr(&["dump", file, "--columns"]);
    assert_eq!(code, 0);
    assert!(
        said.contains("MET            f4[5]  [10, 20, 30, 40, 50]"),
        "{said}"
    );
    assert!(
        said.contains("Jet_pt         jagged f4[5]  [[40], [], [50, 60], ...]"),
        "{said}"
    );
    assert!(
        said.contains("xsec           f8 by sample[2]  A: 1.5, B: 2.5"),
        "{said}"
    );
    // A damaged header is still shown, after a line saying what is wrong.
    let bytes = std::fs::read(&path).unwrap();
    let start = 12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut header: Value = serde_json::from_slice(&bytes[12..start]).unwrap();
    header["derived"] = 5.into();
    header["made"]["recipe"] = "not a record".into();
    let damaged = dir.0.join("damaged.rvr");
    std::fs::write(&damaged, framed(&header, &bytes[start..])).unwrap();
    let (code, said) = rvr(&["dump", damaged.to_str().unwrap()]);
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains("is not valid") && said.contains("\"derived\": 5"),
        "{said}"
    );
    std::fs::write(&path, b"not an rvr file").unwrap();
    let (code, said) = rvr(&["validate", file]);
    assert_eq!(code, 1);
    assert!(said.starts_with(&format!("{file}: INVALID: ")), "{said}");
}

#[test]
fn new_makes_a_study_that_expands_and_never_overwrites() {
    let dir = Scratch::new("new");
    let study = dir.0.join("higgs_tautau");
    let at = study.to_str().unwrap();
    let (code, said) = rvr(&["new", at]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("rvr samples resolve samples.toml"), "{said}");
    for name in ["samples.toml", "recipe.toml", ".gitignore"] {
        assert!(study.join(name).is_file(), "{name}");
    }
    let recipe = std::fs::read_to_string(study.join("recipe.toml")).unwrap();
    assert!(recipe.contains("title = \"higgs_tautau\""));
    // The .gitignore leaves out exactly what SPEC 5.3.3 names.
    let ignored = std::fs::read_to_string(study.join(".gitignore")).unwrap();
    let lines: Vec<&str> = ignored.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines, ["*.rvr", "*.rvr.partial/", "*.rvr.derive", "*.rvr.refill", "*.rvr.refilled", "*.rvr-new", "*.reproduced/"]);
    let (code, said) = rvr(&[
        "samples",
        "expand",
        study.join("samples.toml").to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains("describes 2 samples")
            && said.contains("GluGluToHToTauTau")
            && said.contains("VBF_HToTauTau"),
        "{said}"
    );
    std::fs::write(study.join("recipe.toml"), "mine").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(["new", at])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("writes nothing over a file"));
    assert_eq!(
        std::fs::read_to_string(study.join("recipe.toml")).unwrap(),
        "mine"
    );
}

#[test]
fn samples_alone_says_its_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(["samples"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("rvr samples resolve samples.toml"));
}

/// Run rvr with these variables set and those removed: its exit status, standard output and standard error.
fn run(args: &[&std::ffi::OsStr], set: &[(&str, &Path)], removed: &[&str]) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rvr"));
    command.args(args).env("RVR_CONFIG", std::env::temp_dir().join("rvr-cli-no-config"));
    for (name, value) in set {
        command.env(name, value);
    }
    for name in removed {
        command.env_remove(name);
    }
    let out = command.output().unwrap();
    (out.status.code().unwrap(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// `run` of text arguments, with nothing else set.
fn said(args: &[&str]) -> (i32, String, String) {
    let args: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    run(&args, &[], &[])
}

#[test]
fn dump_all_prints_every_value_one_row_a_line() {
    let dir = Scratch::new("dump-all");
    let path = dir.0.join("one.rvr");
    pressed(&path, 40., 2.5);
    let file = path.to_str().unwrap();
    let (code, out, _) = said(&["dump", file, "--columns", "--all"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("sample\tentry\tMET\tJet_pt\nA\t0\t10\t[40]\nA\t1\t20\t[]\nA\t2\t30\t[50, 60]\nB\t0\t40\t[70, 80]\nB\t1\t50\t[90]\nxsec "), "{out}");
    let (code, out, _) = said(&["dump", file, "--all"]);
    assert!(code == 0 && out.contains("##### end samples.lock #####") && out.contains("B\t1\t50\t[90]\n"), "{out}");
    let (code, out, err) = said(&["dump", file, "--files", "--all"]);
    assert!(code == 2 && out.is_empty() && err.contains("--files prints no column: give --all alone or with --columns"), "{err}");
}

#[test]
fn diff_compares_labels_and_reading_failures_are_said_plainly() {
    let dir = Scratch::new("labels");
    let (a, b) = (dir.0.join("a.rvr"), dir.0.join("b.rvr"));
    pressed(&a, 40., 2.5);
    let bytes = std::fs::read(&a).unwrap();
    let start = 12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut header: Value = serde_json::from_slice(&bytes[12..start]).unwrap();
    header["data"]["samples"][1]["label"] = "Sample B, again".into();
    std::fs::write(&b, framed(&header, &bytes[start..])).unwrap();
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
    let (code, out, _) = said(&["diff", a, b]);
    assert_eq!((code, out), (1, format!("sample B: its label is \"Sample B\" in {a}, \"Sample B, again\" in {b}\n")));
    // Full validation ties a label to the lock the file carries.
    let (code, out, _) = said(&["validate", b]);
    assert!(code == 1 && out.contains("with their names and labels"), "{out}");
    let missing = dir.0.join("missing.rvr");
    let missing = missing.to_str().unwrap();
    let (code, out, _) = said(&["validate", missing]);
    assert_eq!((code, out), (1, format!("{missing}: INVALID: it is not there\n")));
    let folder = dir.0.to_str().unwrap();
    let (code, _, err) = said(&["dump", folder]);
    assert!(code == 1 && err.contains(&format!("{folder} is a folder, not a file")), "{err}");
    let (code, out, err) = said(&["diff", a, missing]);
    assert_eq!((code, out.as_str(), err), (2, "", format!("rvr diff: {missing} is not there\n")));
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_wrong_use() {
    use std::os::unix::ffi::OsStrExt;
    let (code, _, err) = run(&[std::ffi::OsStr::new("validate"), std::ffi::OsStr::from_bytes(b"caf\xe9.rvr")], &[], &[]);
    assert!(code == 2 && err.contains("the argument \"caf\u{fffd}.rvr\" is not UTF-8 text"), "{err}");
}

#[test]
fn keys_are_made_once_and_trusted_once_under_one_name() {
    let dir = Scratch::new("keys");
    let config = dir.0.join("config");
    let key = |args: &[&str]| {
        let args: Vec<&std::ffi::OsStr> = ["key"].iter().chain(args).map(std::ffi::OsStr::new).collect();
        run(&args, &[("RVR_CONFIG", &config)], &[])
    };
    let (code, out, _) = key(&["new", "Tester"]);
    assert!(code == 0 && out.contains("rvr key trust ") && out.ends_with(" Tester\n"), "{out}");
    let text = std::fs::read_to_string(config.join("key")).unwrap();
    assert!(text.len() == 65 && text.ends_with('\n'));
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(config.join("key")).unwrap().permissions()) & 0o777, 0o600);
    let (code, _, err) = key(&["new"]);
    assert!(code == 1 && err.contains("You already have a key"), "{err}");
    assert_eq!(std::fs::read_to_string(config.join("key")).unwrap(), text);
    let left: Vec<_> = std::fs::read_dir(&config).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, ["key"]);
    assert_eq!(key(&["new", "two", "words"]).0, 2);
    let other = "ab".repeat(32);
    let (code, out, _) = key(&["trust", &other, "Someone", "Else"]);
    assert!(code == 0 && out.contains("now say they are Someone Else's"), "{out}");
    let (code, out, _) = key(&["trust", &other, "Someone", "Else"]);
    assert!(code == 0 && out.contains("trusted already, as Someone Else's: nothing to change"), "{out}");
    let (code, _, err) = key(&["trust", &other, "Another"]);
    assert!(code == 1 && err.contains("trusted already, as Someone Else's, and is left so: to trust it as Another's"), "{err}");
    assert_eq!(std::fs::read_to_string(config.join("trusted")).unwrap(), format!("{other} Someone Else\n"));
    let (code, _, err) = key(&["trust", &"AB".repeat(32), "Upper"]);
    assert!(code == 1 && err.contains("one is 64 lowercase hexadecimal characters"), "{err}");
}

#[test]
fn proxy_says_what_is_wrong_with_a_proxy_and_a_key_without_the_network() {
    let dir = Scratch::new("proxy");
    let proxy = |args: &[&str], set: &[(&str, &Path)]| {
        let args: Vec<&std::ffi::OsStr> = ["proxy"].iter().chain(args).map(std::ffi::OsStr::new).collect();
        run(&args, set, &["X509_USER_CERT", "X509_USER_KEY"])
    };
    let missing = dir.0.join("x509up_missing");
    let (code, out, _) = proxy(&[], &[("X509_USER_PROXY", &missing)]);
    assert!(code == 1 && out.contains(&format!("The grid proxy at {} does not exist. Run rvr proxy init to make one.", missing.display())), "{out}");
    let made_up = dir.0.join("x509up_made_up");
    std::fs::write(&made_up, "-----BEGIN CERTIFICATE-----\nbm90IGEgY2VydGlmaWNhdGU=\n-----END CERTIFICATE-----\n").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&made_up, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let (code, out, _) = proxy(&[], &[("X509_USER_PROXY", &made_up)]);
    assert!(code == 1 && out.starts_with(&format!("proxy     {}\n", made_up.display())) && out.contains("rvr proxy init"), "{out}");
    for wrong in [&["init", "--hours", "0"][..], &["init", "--hours", "721"], &["init", "--vo", "cms"], &["extra"]] {
        assert_eq!(proxy(wrong, &[]).0, 2, "{wrong:?}");
    }
    let globus = dir.0.join(".globus");
    std::fs::create_dir_all(&globus).unwrap();
    for name in ["usercert.pem", "userkey.pem"] {
        std::fs::write(globus.join(name), "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n").unwrap();
    }
    #[cfg(unix)]
    {
        std::fs::set_permissions(globus.join("userkey.pem"), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let (code, _, err) = proxy(&["init"], &[("HOME", &dir.0), ("X509_USER_PROXY", &missing)]);
        assert!(code == 1 && err.contains(&format!("chmod 600 {}", globus.join("userkey.pem").display())), "{err}");
        assert!(!missing.exists());
    }
}
