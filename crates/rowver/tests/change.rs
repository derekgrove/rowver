//! Changes made in a file itself (docs/SPEC.md §8.12): a tool key set or dropped, columns added
//! after the arrays, each journaled beside the file and put back when cut short, and a reader's
//! refusal of a file a change to which was cut short.

mod common;

use common::{golden, journal, parts, written};
use rowver::change::{drop_tool, restore, set_tool, tool_data, Change, Locked, Restored};
use rowver::rvr::{validate_file, Reader};
use rowver::write::{array, Source, Values};
use serde_json::{json, Value};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// Set the tool key `key` of the file at `path` to the object `text` holds, under its lock, as one writer does.
fn set(path: impl AsRef<Path>, key: &str, text: &[u8]) -> rowver::Result<()> {
    set_tool(&Locked::take(path)?, key, tool_data(key, text)?)
}

/// Drop the tool key `key` of the file at `path`, under its lock: its object's text.
fn dropped(path: impl AsRef<Path>, key: &str) -> rowver::Result<String> {
    drop_tool(&Locked::take(path)?, key)
}

/// The journal a change that adds arrays writes before it touches a file whose bytes were
/// `before`, its tail from `start` (SPEC 8.12): the magic, its old length, where its tail began and
/// 2^64 - 1 (no length after it), the SHA-256 of the up to 65,536 bytes before the tail, the tail,
/// then the SHA-256 of all before it.
fn journal_of(before: &[u8], start: usize) -> Vec<u8> {
    let sha = |bytes: &[u8]| sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(bytes));
    let mut kept = b"\x89RVJ\r\n\x1a\n".to_vec();
    for number in [before.len() as u64, start as u64, u64::MAX] {
        kept.extend(number.to_le_bytes());
    }
    kept.extend(sha(&before[start.saturating_sub(65_536)..start]));
    kept.extend(&before[start..]);
    kept.extend(sha(&kept));
    kept
}

/// The corpus file `name` of tests/conformance/rvr/valid.
fn valid(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/rvr/valid").join(name)).unwrap()
}

/// The file's inode, so a change is seen to be made in it and not in a file moved over it.
fn inode(path: impl AsRef<std::path::Path>) -> u64 {
    std::fs::metadata(path.as_ref()).unwrap().ino()
}

/// The header of the file at `path`, parsed.
fn header(path: impl AsRef<std::path::Path>) -> Value {
    serde_json::from_slice(parts(&std::fs::read(path).unwrap()).0).unwrap()
}

/// The names of the files a change written anew stages beside the file at `path` (`FILE.anew-PID-N`), left there.
fn staged_beside(path: impl AsRef<Path>) -> Vec<String> {
    let path = path.as_ref();
    let prefix = format!("{}.anew-", path.file_name().unwrap().to_string_lossy());
    let names = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned());
    names.filter(|name| name.starts_with(&prefix)).collect()
}

/// The bytes of the file before its header: the magic and the arrays.
fn arrays(bytes: &[u8]) -> Vec<u8> {
    bytes[..8 + parts(bytes).1.len()].to_vec()
}

// FORMAT 11; SPEC 8.11, 8.12
#[test]
fn a_tool_key_is_set_and_dropped_in_the_file_itself_its_arrays_untouched() {
    let before = golden();
    let path = written(&before, "tool");
    let ino = inode(&path);
    set(&path, "my-tool", br#"{"a": [1, 2.5, "x"], "b": {"c": null}}"#).unwrap();
    let after = std::fs::read(&path).unwrap();
    assert_eq!((inode(&path), arrays(&after)), (ino, arrays(&before)));
    assert_eq!(header(&path)["my-tool"], json!({"a": [1, 2.5, "x"], "b": {"c": null}}));
    assert_eq!(header(&path)["fixture_tool"], json!({"preserved": "μ"}), "the other tool keys stay");
    assert!(!journal(&path).exists(), "the journal goes once the change is made");
    validate_file(&path).unwrap();
    // Set again, it is replaced; dropped, it is gone and its object handed back.
    set(&path, "my-tool", b"{}").unwrap();
    assert_eq!(header(&path)["my-tool"], json!({}));
    assert_eq!(dropped(&path, "my-tool").unwrap(), "{}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "dropped, the file is as it was, byte for byte");
    let said = dropped(&path, "my-tool").unwrap_err().to_string();
    assert!(said.contains(&format!("has no tool key my-tool: rvr tool {} lists its tool keys", path.0.display())), "{said}");
}

// SPEC 1.5, 8.12
#[test]
fn a_header_read_after_a_change_in_place_says_the_file_changed() {
    let path = written(&golden(), "header-changed");
    let reader = Reader::open(&path).unwrap();
    assert_eq!(reader.header_text().unwrap(), parts(&golden()).0);
    set(&path, "my-tool", b"{\"a\": 1}").unwrap();
    let said = rowver::plain(&path, &reader.header_text().unwrap_err());
    assert_eq!(said, format!("{} changed while it was read (emptied, or written again): open it again", path.0.display()));
}

// FORMAT 9, 11; SPEC 8.12
#[test]
fn a_tool_key_change_leaves_the_signature_as_it_is() {
    let (mut signed, arrays) = common::split(&golden());
    rowver::sign::sign(&mut signed, &[3; 32]).unwrap();
    let path = written(&common::to_bytes(&signed, &[arrays]), "tool-signed");
    set(&path, "my-tool", br#"{"n": 1}"#).unwrap();
    assert_eq!(header(&path)["signature"], signed["signature"]);
    Reader::open(&path).unwrap();
}

// FORMAT 1, 11; SPEC 8.11
#[test]
fn a_tool_key_is_refused_for_each_rule_of_the_header_and_the_file_left_as_it_was() {
    let before = golden();
    let path = written(&before, "tool-refused");
    // Nested from its place: the header is 1 deep, the key's object 2, so 62 more arrays at most.
    set(&path, "deep", deep(62).as_bytes()).unwrap();
    dropped(&path, "deep").unwrap();
    for (_, key, text, said) in imports().into_iter().filter(|(_, _, _, said)| !said.is_empty()) {
        let refused = set(&path, key, text.as_bytes()).unwrap_err().to_string();
        assert!(refused.contains(said) && !refused.contains('\n'), "{key} {text:?}: {refused}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "{key} {text:?}");
        assert!(!journal(&path).exists());
    }
}

// FORMAT 1, 11; SPEC 8.11
#[test]
fn a_tool_key_that_breaks_the_header_rules_is_dropped_and_the_file_opens() {
    let before = golden();
    let (text, arrays) = parts(&before);
    let text = std::str::from_utf8(text).unwrap();
    let with = |text: &str, key: &str, value: &str| common::framed(format!("{},\"{key}\":{value}}}", &text[..text.len() - 1]).as_bytes(), arrays);
    // Lone surrogates and depth past what serde_json reads (128) too: the key's bytes are not parsed.
    for (key, value) in [("minus", r#"{"a":[-0]}"#.to_owned()), ("deep", deep(63)), ("lone", r#"{"s":"\ud800 } ] \" {"}"#.to_owned()), ("deeper", deep(198))] {
        let path = written(&with(text, key, &value), "drop-broken");
        assert!(Reader::open(&path).is_err(), "{key}");
        assert_eq!(dropped(&path, key).unwrap(), value);
        validate_file(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before, "{key}");
    }
    // Broken outside the key dropped: refused as before, the file as it was.
    let broken = with(&text.replacen("\"made\":{", "\"made\":{\"x\":-0,", 1), "sound", "{}");
    let path = written(&broken, "drop-broken-made");
    let said = dropped(&path, "sound").unwrap_err().to_string();
    assert!(said.contains("made.x is written -0") && said.contains("(line 1, column"), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (broken, false));
}

/// Nested arrays `n` deep in an object: a tool key's object, 2 deep in the header, with them inside.
fn deep(n: usize) -> String {
    format!("{{\"d\": {}{}}}", "[".repeat(n), "]".repeat(n))
}

/// Tool-key imports, each with the rule it shows and what its refusal says (nothing: it is taken).
fn imports() -> Vec<(&'static str, &'static str, String, &'static str)> {
    vec![
        ("FORMAT §11 a tool key's object", "my-tool", r#"{"a": [1, 2.5, "x"], "b": {"c": null}}"#.to_owned(), ""),
        ("FORMAT §1 §11 nesting to 64 deep, counted from its place", "deep", deep(62), ""),
        ("FORMAT §11 a name of letters, digits, _, - and .", "my.tool_2-b", "{}".to_owned(), ""),
        ("FORMAT §11 one JSON object", "my-tool", "[1]".to_owned(), "the tool key my-tool must be one JSON object"),
        ("FORMAT §1 JSON", "my-tool", r#"{"a": "#.to_owned(), "the tool key my-tool is not JSON as docs/FORMAT.md §1 reads it"),
        ("FORMAT §1 no byte-order mark", "my-tool", "\u{feff}{}".to_owned(), "the tool key my-tool is not JSON as docs/FORMAT.md §1 reads it"),
        ("FORMAT §1 no key twice", "my-tool", r#"{"a": 1, "a": 2}"#.to_owned(), "[\"my-tool\"].a is written twice"),
        ("FORMAT §1 no whole number written -0", "my-tool", r#"{"a": [-0]}"#.to_owned(), "[\"my-tool\"].a[0] is written -0"),
        ("FORMAT §1 no lone surrogate", "my-tool", r#"{"a": "\ud800"}"#.to_owned(), "the tool key my-tool is not JSON as docs/FORMAT.md §1 reads it"),
        ("FORMAT §1 §11 nesting at most 64, counted from its place", "my-tool", deep(63), "is nested 65 arrays and objects deep, more than 64"),
        ("FORMAT §11 a tool key's name", "my tool", "{}".to_owned(), "\"my tool\" is not a tool key's name"),
        ("FORMAT §11 a tool key's name: not empty", "", "{}".to_owned(), "\"\" is not a tool key's name"),
        ("FORMAT §11 not a slip of the format's own keys", "Data", "{}".to_owned(), "the top-level key \"Data\" is a slip of data"),
        ("FORMAT §2 §11 not the format's own key", "made", "{}".to_owned(), "made is the format's own key, not a tool key"),
    ]
}

// FORMAT 1, 2, 11
#[test]
fn the_conformance_corpus_holds_the_tool_key_imports_this_writer_takes_and_refuses() {
    let fixture = golden();
    let cases: Vec<Value> = imports().into_iter().map(|(rule, key, text, said)| {
        let path = written(&fixture, "tool-corpus");
        let taken = set(&path, key, text.as_bytes());
        assert_eq!(taken.is_ok(), said.is_empty(), "{rule}: {taken:?}");
        let mut case = json!({"rule": rule, "key": key, "text": text, "verdict": if said.is_empty() { "valid" } else { "refused" }});
        if let Err(e) = taken {
            case["rust_says"] = e.to_string().replace(&*path.0.to_string_lossy(), "FILE.rvr").into();
        }
        case
    }).collect();
    let manifest = json!({
        "about": "tool-key imports (rvr tool FILE KEY --set DATA.json, docs/SPEC.md §8.11): each KEY and the text of DATA.json, set on rvr/valid/fixture.rvr, with its verdict; see README.md",
        "cases": cases,
    });
    common::corpus::corpus("tool", &[], &manifest);
}

// FORMAT 12; SPEC 8.12
#[test]
fn a_change_cut_short_is_refused_by_a_reader_and_put_back_byte_for_byte() {
    let before = golden();
    let path = written(&before, "cut-short");
    let (text, arrays) = parts(&before);
    let start = 8 + arrays.len();
    let kept = journal_of(&before, start);
    std::fs::write(journal(&path), &kept).unwrap();
    // Cut short half way through writing the new header over the old.
    let mut half = before[..start].to_vec();
    half.extend(&text[..text.len() / 2]);
    std::fs::write(&path, &half).unwrap();
    let _gone = common::Written(journal(&path));
    let said = Reader::open(&path).unwrap_err().to_string();
    assert!(said.contains("-journal is beside it): any rvr command on the file puts it back as it was"), "{said}");
    assert!(validate_file(&path).unwrap_err().to_string().contains("was cut short"));
    assert_eq!(restore(&path).unwrap(), Restored::PutBack);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!journal(&path).exists());
    assert_eq!(restore(&path).unwrap(), Restored::Nothing, "nothing more to put back");
    // A journal cut short itself was written before the file was touched: it is let go.
    std::fs::write(journal(&path), &kept[..kept.len() - 1]).unwrap();
    assert_eq!(restore(&path).unwrap(), Restored::LetGo);
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before, false));
    // A journal whose file is not there is of no file that may be made there later: it is let go.
    std::fs::write(journal(&path), &kept).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!((restore(&path).unwrap(), journal(&path).exists()), (Restored::NoFile, false));
}

// SPEC 8.12
#[test]
fn a_second_writer_is_refused_while_one_holds_the_file() {
    let before = golden();
    let path = written(&before, "locked");
    let held = std::fs::File::open(&path).unwrap();
    held.lock().unwrap();
    let busy = "is being changed by another program (it holds the file's lock): wait for it to finish, then try again";
    let refused = [Locked::take(&path).err(), set(&path, "my-tool", b"{}").err(), dropped(&path, "fixture_tool").err()];
    for said in refused.map(|e| e.expect("refused").to_string()) {
        assert!(said.contains(busy), "{said}");
    }
    held.unlock().unwrap();
    // One writer holds it from its first read to its last write: another is refused meanwhile,
    // and the change it makes is made through the handle it locked.
    let locked = Locked::take(&path).unwrap();
    assert!(matches!(std::fs::File::open(&path).unwrap().try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    set_tool(&locked, "my-tool", json!({})).unwrap();
    assert_eq!(drop_tool(&locked, "my-tool").unwrap(), "{}");
    let freed = rowver::write::empty(locked).unwrap();
    assert_eq!(freed as usize, parts(&before).1.len());
    assert!(validate_file(&path).unwrap().empty);
    set(&path, "my-tool", b"{}").unwrap();
}

// SPEC 8.12
#[test]
fn a_file_being_changed_now_is_told_apart_from_one_a_change_to_which_was_cut_short() {
    let before = golden();
    let path = written(&before, "busy");
    let start = 8 + parts(&before).1.len();
    std::fs::write(journal(&path), journal_of(&before, start)).unwrap();
    let _gone = common::Written(journal(&path));
    // A writer holding the file's lock is changing it now: a reader is told to wait, not that it was cut short.
    let held = std::fs::File::open(&path).unwrap();
    held.lock().unwrap();
    let now = format!("{} is being changed by another program now: wait for it to finish, then open it again", path.0.display());
    for said in [Reader::open(&path).err(), validate_file(&path).err()].map(|e| e.expect("refused").to_string()) {
        assert!(said.contains(&now), "{said}");
    }
    held.unlock().unwrap();
    let said = Reader::open(&path).unwrap_err().to_string();
    assert!(said.contains("a change to it was cut short"), "{said}");
}

// SPEC 8.12
#[test]
fn the_journal_of_a_file_since_replaced_is_refused_and_neither_is_touched() {
    let before = golden();
    let path = written(&before, "replaced");
    let start = 8 + parts(&before).1.len();
    let kept = journal_of(&before, start);
    std::fs::write(journal(&path), &kept).unwrap();
    let _gone = common::Written(journal(&path));
    let (shown, beside) = (path.0.display(), journal(&path));
    let said = format!("{} is not of this {shown} ({shown} was replaced since a change to it was cut short): remove {} if {shown} is the file you want", beside.display(), beside.display());
    // Another file moved to its name since: one with other arrays and longer than the tail's
    // start, and one shorter.
    let others = [valid("generated-10.rvr"), before[..start / 2].to_vec()];
    assert!(others[0].len() > start && others[0][..start] != before[..start]);
    for other in others {
        let moved = path.0.with_extension("other");
        std::fs::write(&moved, &other).unwrap();
        std::fs::rename(&moved, &path).unwrap();
        for refused in [restore(&path).err(), set(&path, "k", b"{}").err(), Locked::take(&path).err()] {
            let refused = refused.expect("refused").to_string();
            assert!(refused.contains(&said), "{refused}");
        }
        assert_eq!((std::fs::read(&path).unwrap(), std::fs::read(&beside).unwrap()), (other, kept.clone()), "neither is touched");
    }
    // A journal that does not check out is let go, whatever file it names.
    std::fs::write(&beside, &kept[..kept.len() - 1]).unwrap();
    assert_eq!(restore(&path).unwrap(), Restored::LetGo);
    assert!(!beside.exists());
}

// SPEC 8.12
#[test]
fn a_file_copied_with_its_journal_is_put_back_on_the_copy() {
    // A file whose change was cut short 3 bytes into its new tail, its tail from `start`, and its
    // journal; then checked to be put back as it was.
    let cut = |name: &str, bytes: &[u8], start: usize| {
        let path = written(&bytes[..start + 3], name);
        std::fs::write(journal(&path), journal_of(bytes, start)).unwrap();
        (common::Written(journal(&path)), path)
    };
    let put_back = |path: &common::Written, bytes: &[u8]| {
        assert_eq!(restore(path).unwrap(), Restored::PutBack);
        assert_eq!((std::fs::read(path).unwrap(), journal(path).exists()), (bytes.to_vec(), false));
    };
    let before = golden();
    let start = 8 + parts(&before).1.len();
    let (_beside, original) = cut("copied-from", &before, start);
    // The file and its journal copied together: the copy is put back, and the original where it is.
    let (_copy_beside, copy) = cut("copied-to", &before, start);
    put_back(&copy, &before);
    put_back(&original, &before);
    // An empty file's hashed bytes are only the magic, which every file begins with: accepted.
    let shell = written(&before, "copied-empty");
    rowver::write::empty(Locked::take(&shell).unwrap()).unwrap();
    let emptied = std::fs::read(&shell).unwrap();
    let (_shell_beside, shell) = cut("copied-empty-cut", &emptied, 8);
    put_back(&shell, &emptied);
    // Only the 65,536 bytes before the tail are hashed: a byte before them may differ, one in them not.
    let mut long = before[..8].to_vec();
    long.extend((0..70_000_u32).map(|n| (n % 251) as u8));
    let start = long.len();
    long.extend(&before[8 + parts(&before).1.len()..]);
    let (_long_beside, path) = cut("copied-long", &long, start);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[start - 65_537] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    long[start - 65_537] ^= 1;
    put_back(&path, &long);
    let (_long_beside, path) = cut("copied-long-in", &long, start);
    bytes = std::fs::read(&path).unwrap();
    bytes[start - 65_536] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert!(restore(&path).unwrap_err().to_string().contains("was replaced since a change to it was cut short"));
}

// SPEC 8.12
#[test]
fn a_refused_change_touches_nothing_on_disk() {
    let folder = std::env::temp_dir().join(format!("rowver-test-{}-untouched", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = common::Written(folder.join("untouched.rvr"));
    std::fs::write(&path, golden()).unwrap();
    let stamps = || {
        let (file, dir) = (std::fs::metadata(&path).unwrap(), std::fs::metadata(&folder).unwrap());
        (file.mtime(), file.mtime_nsec(), file.ctime_nsec(), dir.mtime_nsec(), dir.ctime_nsec())
    };
    let was = stamps();
    std::thread::sleep(std::time::Duration::from_millis(20));
    for (key, text) in [("made", "{}"), ("my-tool", "[1]"), ("my-tool", "{\"a\": -0}")] {
        assert!(set(&path, key, text.as_bytes()).is_err());
    }
    // Refused once the change began: a key it does not have, or a header that would not open.
    assert!(dropped(&path, "no-such-key").is_err());
    assert_eq!(stamps(), was, "no journal was written, nor the file");
    let (mut header, arrays) = common::split(&golden());
    header["made"]["sha256"] = "0".repeat(64).into();
    std::fs::write(&path, common::to_bytes(&header, &[arrays])).unwrap();
    let was = stamps();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let said = set(&path, "my-tool", b"{}").unwrap_err().to_string();
    assert!(said.contains("made.sha256"), "{said}");
    assert_eq!(stamps(), was, "no journal was written, nor the file");
    drop(path);
    std::fs::remove_dir(&folder).unwrap();
}

// FORMAT 1, 11; SPEC 8.11, 8.12
#[test]
fn a_tool_key_is_set_and_dropped_on_an_empty_file_and_a_rowless_one() {
    for name in ["fixture-empty.rvr", "fixture-empty-derived.rvr", "fixture-rowless.rvr", "fixture-rowless-derived.rvr"] {
        let before = valid(name);
        let path = written(&before, name);
        set(&path, "my-tool", br#"{"n": 1}"#).unwrap();
        let checked = validate_file(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(checked.empty, name.contains("empty"), "{name}");
        assert_eq!(Reader::open(&path).unwrap().header()["my-tool"], json!({"n": 1}), "{name}");
        assert_eq!(parts(&std::fs::read(&path).unwrap()).1, parts(&before).1, "{name}: the arrays as they were");
        dropped(&path, "my-tool").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before, "{name}");
    }
}

/// The permission bits of the file at `path`.
fn mode(path: impl AsRef<Path>) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// SPEC 8.12
#[test]
fn a_file_that_may_not_be_written_is_changed_anew_and_moved_over_its_permissions_kept() {
    let before = golden();
    let path = written(&before, "read-only");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let ino = inode(&path);
    let locked = Locked::take(&path).unwrap();
    assert_eq!(locked.anew().as_deref(), Some("you may not write it"));
    set_tool(&locked, "my-tool", json!({})).unwrap();
    assert_ne!(inode(&path), ino, "a new file, moved over it");
    assert_eq!((mode(&path), staged_beside(&path), journal(&path).exists()), (0o444, vec![], false));
    assert_eq!(header(&path)["my-tool"], json!({}));
    assert_eq!(arrays(&std::fs::read(&path).unwrap()), arrays(&before));
    // The lock is of the file moved away: it is taken again for another change.
    let said = set_tool(&locked, "other", json!({})).unwrap_err().to_string();
    assert!(said.contains("was written anew and moved over since it was locked: lock it again"), "{said}");
    drop(locked);
    assert_eq!(dropped(&path, "my-tool").unwrap(), "{}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Columns added anew: the arrays copied as they are, the new ones after them.
    let locked = Locked::take(&path).unwrap();
    let mut change = Change::begin(&locked).unwrap();
    let rows = change.header()["data"]["rows"].as_u64().unwrap() as usize;
    let values: Vec<f64> = (0..rows).map(|n| n as f64).collect();
    let records = change.pack(vec![("extra".to_owned(), Source::Values(Values::Flat(array(&values))))], 2).unwrap();
    let mut header = change.header().clone();
    header["data"]["cols"].as_array_mut().unwrap().extend(records);
    rowver::write::data_sealed(&mut header["data"]).unwrap();
    header["made"]["recipe"]["columns"].as_array_mut().unwrap().push("extra".into());
    change.finish(&common::sealed(header)).unwrap();
    let after = std::fs::read(&path).unwrap();
    assert_eq!(&after[..arrays(&before).len()], &arrays(&before)[..]);
    assert_eq!((mode(&path), staged_beside(&path), journal(&path).exists()), (0o444, vec![], false));
    let extra = Reader::open(&path).unwrap().column("extra").unwrap().unwrap();
    assert_eq!(extra.bytes, values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
}

// SPEC 8.9, 8.12
#[test]
fn an_emptied_file_keeps_its_permissions() {
    let path = written(&golden(), "permissions");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    rowver::write::empty(Locked::take(&path).unwrap()).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
}

// SPEC 8.12
#[test]
fn a_file_rewritten_or_refilled_is_put_back_first_when_a_change_to_it_was_cut_short() {
    let before = golden();
    let start = 8 + parts(&before).1.len();
    // A file whose change was cut short 3 bytes into its new tail, and its journal.
    let cut = |name: &str, bytes: &[u8], start: usize| {
        let path = written(bytes, name);
        std::fs::write(journal(&path), journal_of(bytes, start)).unwrap();
        std::fs::write(&path, &bytes[..start + 3]).unwrap();
        let beside = common::Written(journal(&path));
        (path, beside)
    };
    let (from, _beside) = cut("rewrite-from", &before, start);
    let to = from.0.with_extension("to");
    let _to = common::Written(to.clone());
    let header: Value = serde_json::from_slice(parts(&before).0).unwrap();
    rowver::write::rewrite(&from, &to, &header).unwrap();
    assert_eq!((std::fs::read(&from).unwrap(), journal(&from).exists()), (before.clone(), false));
    validate_file(&to).unwrap();
    // An empty shell cut short, and the file it is refilled from.
    let shell = written(&before, "refill-shell");
    rowver::write::empty(Locked::take(&shell).unwrap()).unwrap();
    let emptied = std::fs::read(&shell).unwrap();
    let (shell, _shell_beside) = cut("refill-shell-cut", &emptied, 8);
    let (full, _full_beside) = cut("refill-from", &before, start);
    let out = shell.0.with_extension("out");
    let _out = common::Written(out.clone());
    rowver::write::refilled(&Locked::take(&shell).unwrap(), &full, &out).unwrap();
    assert_eq!((std::fs::read(&shell).unwrap(), std::fs::read(&full).unwrap()), (emptied, before.clone()));
    assert_eq!(std::fs::read(&out).unwrap(), before);
}

/// Held by each test that sets `RVR_READ_LOG`, which the whole process shares.
#[cfg(debug_assertions)]
static READ_LOG: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every read of the arrays of the file at `path` a debug build logged to `RVR_READ_LOG` (SPEC
/// 8.12): each `(at, length)` in the file.
#[cfg(debug_assertions)]
fn reads(log: &Path, path: &Path) -> Vec<(u64, u64)> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let path = std::fs::canonicalize(path).unwrap();
    text.lines().filter(|line| line.split('\t').next().map(Path::new) == Some(&path)).map(|line| {
        let mut words = line.split('\t').skip(1).map(|w| w.parse().unwrap());
        (words.next().unwrap(), words.next().unwrap())
    }).collect()
}

/// The rows of the file at `path` read, `sample` and `entry`, while `RVR_READ_LOG` names `log`:
/// what an empty log of a change must be told apart from, a log that logs nothing. (Looked inside,
/// as a file made for a test may have no `made`.)
#[cfg(debug_assertions)]
fn rows_are_logged(log: &Path, path: &Path) {
    let (reader, _) = Reader::inspect(path).unwrap();
    let (sample, entry) = (reader.sample_slice(0, 1).unwrap(), reader.entry_slice(0, 1).unwrap());
    assert_eq!((sample.len(), entry.len()), (1, 1));
    let spans: Vec<(u64, u64)> = ["sample", "entry"].map(|key| &reader.header()["data"][key]).iter().map(|r| (8 + r["at"].as_u64().unwrap(), r["size"].as_u64().unwrap())).collect();
    let read = reads(log, path);
    assert!(spans.iter().all(|span| read.contains(span)), "the log holds the reads of sample and entry {spans:?}: {read:?}");
}

// FORMAT 1, 7; SPEC 8.12
#[test]
#[cfg(debug_assertions)]
fn columns_are_added_after_the_arrays_in_place_reading_none_of_them() {
    let _serial = READ_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let before = golden();
    let path = written(&before, "added");
    let ino = inode(&path);
    let log = path.0.with_extension("reads");
    let _log = common::Written(log.clone());
    std::env::set_var("RVR_READ_LOG", &log);
    let locked = Locked::take(&path).unwrap();
    let mut change = Change::begin(&locked).unwrap();
    assert!(!journal(&path).exists(), "no journal until the file is about to change");
    let rows = change.header()["data"]["rows"].as_u64().unwrap() as usize;
    let values: Vec<f64> = (0..rows).map(|n| n as f64 / 4.).collect();
    let records = change.pack(vec![("extra".to_owned(), Source::Values(Values::Flat(array(&values))))], 2).unwrap();
    assert!(journal(&path).exists(), "journaled before its first byte changed");
    let mut header = change.header().clone();
    header["data"]["cols"].as_array_mut().unwrap().extend(records);
    rowver::write::data_sealed(&mut header["data"]).unwrap();
    header["made"]["recipe"]["columns"].as_array_mut().unwrap().push("extra".into());
    let header = common::sealed(header);
    change.finish(&header).unwrap();
    drop(locked);
    assert_eq!(reads(&log, &path.0), [], "no array of the file was read");
    rows_are_logged(&log, &path.0);
    std::env::remove_var("RVR_READ_LOG");
    let after = std::fs::read(&path).unwrap();
    let old = 8 + parts(&before).1.len();
    assert_eq!((inode(&path), &after[..old]), (ino, &before[..old]), "the arrays it had are where they were");
    assert_eq!(header["data"]["cols"].as_array().unwrap().last().unwrap()["at"], json!(old - 8), "the new one after them");
    assert!(!journal(&path).exists());
    let reader = Reader::open(&path).unwrap();
    let extra = reader.column("extra").unwrap().unwrap();
    assert_eq!(extra.bytes, values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
}

// SPEC 8.12; FORMAT 7, 8
#[test]
fn the_rows_a_press_in_place_read_are_checked_to_be_the_files_in_order() {
    use rowver::write::{rows_agree, Sample};
    let path = written(&golden(), "rows-agree");
    let reader = Reader::open(&path).unwrap();
    let data = &reader.header()["data"];
    let samples: Vec<Sample> = data["samples"].as_array().unwrap().iter().map(|s| Sample { name: s["name"].as_str().unwrap().into(), label: s["label"].as_str().unwrap().into(), rows: s["rows"].as_u64().unwrap() as usize }).collect();
    let entry = reader.entry().unwrap();
    assert!(samples.len() >= 2 && samples[0].rows >= 2);
    // The file's entries as read, file by file.
    let mut at = 0;
    let read: Vec<Vec<u64>> = data["files"].as_array().unwrap().iter().map(|file| {
        let rows = file["rows"].as_u64().unwrap() as usize;
        at += rows;
        entry[at - rows..at].iter().map(|e| *e as u64).collect()
    }).collect();
    let read: Vec<&[u64]> = read.iter().map(Vec::as_slice).collect();
    let given = |samples: &[Sample], entry: &[i64]| rows_agree(samples, Source::Values(Values::Flat(array(entry))), data, &read).map_err(|e| e.to_string());
    given(&samples, &entry).unwrap();
    // An entry other, two rows in another order, a row moved to the next sample, a row more.
    let mut other = entry.clone();
    other[0] += 1;
    let mut swapped = entry.clone();
    swapped.swap(0, 1);
    let mut moved = samples.clone();
    (moved[0].rows, moved[1].rows) = (moved[0].rows - 1, moved[1].rows + 1);
    let mut more = samples.clone();
    more[0].rows += 1;
    let longer: Vec<i64> = entry.iter().copied().chain([99]).collect();
    let said = [given(&samples, &other), given(&samples, &swapped), given(&moved, &entry), given(&more, &longer)].map(Result::unwrap_err);
    let first = data["files"][0]["path"].as_str().unwrap();
    assert!(said[0].contains(&format!("row 0's entry is {}, and the file's is {} (a row of {first} of ", entry[0] + 1, entry[0])), "{}", said[0]);
    assert!(said[1].contains(&format!("row 0's entry is {}, and the file's is {}", entry[1], entry[0])), "{}", said[1]);
    assert!(said[2].contains("their sample array's sha256"), "{}", said[2]);
    assert!(said[3].contains(&format!("they are {} rows, and the file holds {}", entry.len() + 1, entry.len())), "{}", said[3]);
    // The file's entries given of fewer files than it lists.
    let said = rows_agree(&samples, Source::Values(Values::Flat(array(&entry))), data, &read[1..]).unwrap_err().to_string();
    assert!(said.contains("give the entries of each of its files, in order"), "{said}");
}

// SPEC 8.12
#[test]
fn a_change_dropped_unfinished_is_put_back() {
    let before = golden();
    let path = written(&before, "dropped");
    let locked = Locked::take(&path).unwrap();
    let mut change = Change::begin(&locked).unwrap();
    let rows = change.header()["data"]["rows"].as_u64().unwrap() as usize;
    change.pack(vec![("extra".to_owned(), Source::Values(Values::Flat(array(&vec![1_i32; rows]))))], 1).unwrap();
    drop(change);
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before, false));
}

// SPEC 8.12
#[test]
fn a_column_added_to_a_file_of_no_rows_takes_no_byte_and_is_journaled_as_it_finishes() {
    let path = written(&valid("fixture-rowless.rvr"), "rowless-column");
    let before = std::fs::read(&path).unwrap();
    let locked = Locked::take(&path).unwrap();
    let mut change = Change::begin(&locked).unwrap();
    let records = change.pack(vec![("extra".to_owned(), Source::Values(Values::Flat(array::<i32>(&[]))))], 1).unwrap();
    // Its change writes only a header: nothing written yet, and its journal, with the new tail, still to come.
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before.clone(), false));
    let mut header = change.header().clone();
    header["data"]["cols"].as_array_mut().unwrap().extend(records);
    rowver::write::data_sealed(&mut header["data"]).unwrap();
    header["made"]["recipe"]["columns"].as_array_mut().unwrap().push("extra".into());
    change.finish(&common::sealed(header)).unwrap();
    drop(locked);
    let after = std::fs::read(&path).unwrap();
    assert_eq!((&after[..8], journal(&path).exists()), (&before[..8], false));
    assert!(Reader::open(&path).unwrap().column("extra").unwrap().is_some());
}

// SPEC 8.12; FORMAT 1, 7
#[test]
fn a_header_that_lays_out_other_bytes_than_a_change_wrote_is_refused_and_the_file_put_back() {
    let before = golden();
    let path = written(&before, "misplaced");
    let locked = Locked::take(&path).unwrap();
    let mut change = Change::begin(&locked).unwrap();
    let rows = change.header()["data"]["rows"].as_u64().unwrap() as usize;
    change.pack(vec![("extra".to_owned(), Source::Values(Values::Flat(array(&vec![1_i32; rows]))))], 1).unwrap();
    // The header as it was, whose arrays end before the one just packed.
    let header = change.header().clone();
    let said = change.finish(&header).unwrap_err().to_string();
    assert!(said.contains("a bug in rowver") && said.contains("please report it"), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before, false));
}

// SPEC 8.12; FORMAT 8, 12
#[test]
fn a_header_that_would_not_open_is_refused_by_finish_and_the_file_put_back() {
    let before = golden();
    let path = written(&before, "unsealed");
    let locked = Locked::take(&path).unwrap();
    // A column packed and its record added, but data.sha256 not made again.
    let mut change = Change::begin(&locked).unwrap();
    let rows = change.header()["data"]["rows"].as_u64().unwrap() as usize;
    let records = change.pack(vec![("extra".to_owned(), Source::Values(Values::Flat(array(&vec![1_i32; rows]))))], 1).unwrap();
    let mut header = change.header().clone();
    header["data"]["cols"].as_array_mut().unwrap().extend(records);
    header["made"]["recipe"]["columns"].as_array_mut().unwrap().push("extra".into());
    let said = change.finish(&common::sealed(header)).unwrap_err().to_string();
    assert!(said.contains("is as it was: the header the change would write would not open (") && said.contains("data.sha256 is not the SHA-256"), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before.clone(), false));
    // Its header only: a tool key named as no tool key may be.
    let change = Change::begin(&locked).unwrap();
    let mut header = change.header().clone();
    header["Data"] = json!({});
    let said = change.finish(&header).unwrap_err().to_string();
    assert!(said.contains("is as it was: the header the change would write would not open ("), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before, false));
}

// SPEC 8.12; FORMAT 1, 7
#[test]
fn a_header_of_a_rowless_or_empty_file_laying_out_other_bytes_is_refused() {
    // Its last array 100 bytes longer: in a rowless file, bytes no change wrote; in an empty one,
    // other bytes than its records laid out before.
    for name in ["fixture-rowless.rvr", "fixture-empty.rvr"] {
        let before = valid(name);
        let path = written(&before, name);
        let locked = Locked::take(&path).unwrap();
        let change = Change::begin(&locked).unwrap();
        let mut header = change.header().clone();
        let last = header["data"]["cols"].as_array_mut().unwrap().last_mut().unwrap();
        last["size"] = (last["size"].as_u64().unwrap() + 100).into();
        let said = change.finish(&header).unwrap_err().to_string();
        assert!(said.contains("a bug in rowver") && said.contains("please report it"), "{name}: {said}");
        assert_eq!((std::fs::read(&path).unwrap(), journal(&path).exists()), (before, false), "{name}");
    }
}

// SPEC 8.12
#[test]
#[cfg(all(debug_assertions, unix))]
#[should_panic(expected = "was opened again while this program holds it locked; please report it")]
fn a_debug_build_stops_at_a_file_opened_again_while_it_is_locked() {
    let path = written(&golden(), "opened-again");
    let _locked = Locked::take(&path).unwrap();
    let _ = Reader::open(&path);
}

// SPEC 8.12
#[test]
fn a_change_begun_from_what_inspect_read_reads_nothing_again_while_the_lock_has_made_no_change() {
    let path = written(&golden(), "begun-from-inspect");
    let locked = Locked::take(&path).unwrap();
    let (reader, checked) = locked.inspect().unwrap();
    checked.unwrap();
    // Its header changed behind the lock (as no writer may), the same length: a change begun from
    // what inspect read holds the header it read, not read again.
    let at = golden().windows(16).position(|bytes| bytes == "\"preserved\":\"μ\"".as_bytes()).unwrap();
    let mut other = golden();
    other[at + 13..at + 15].copy_from_slice(b"mu");
    std::fs::write(&path, &other).unwrap();
    let change = Change::of(&locked, &reader).unwrap();
    assert_eq!(change.header()["fixture_tool"]["preserved"], "μ");
    // Once the lock has begun a change (that one, dropped), the file is read again.
    drop(change);
    assert_eq!(Change::of(&locked, &reader).unwrap().header()["fixture_tool"]["preserved"], "mu");
}

// SPEC 8.12
#[test]
fn a_reader_takes_no_lock_and_reads_a_file_another_program_holds_locked_with_no_journal_beside_it() {
    let path = written(&golden(), "reader-no-lock");
    // Another program holds the file's lock, at a question or planning, and no journal is there.
    let other = std::fs::File::open(&path).unwrap();
    other.lock().unwrap();
    let reader = Reader::open(&path).unwrap();
    let nlep = reader.column("nlep").unwrap().unwrap();
    assert_eq!(validate_file(&path).unwrap().rows, 5);
    other.unlock().unwrap();
    assert_eq!(Reader::open(&path).unwrap().column("nlep").unwrap().unwrap(), nlep);
    // A writer started while rvr status holds a shared lock for its moment waits it out, and takes the file.
    let (taken, held) = std::sync::mpsc::channel();
    let status = std::thread::spawn({
        let path = path.0.clone();
        move || {
            let file = std::fs::File::open(&path).unwrap();
            file.lock_shared().unwrap();
            taken.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(30));
            file.unlock().unwrap();
        }
    });
    held.recv().unwrap();
    let locked = Locked::take(&path).unwrap();
    status.join().unwrap();
    assert!(locked.reader().is_ok());
}

// SPEC 8.12
#[test]
fn a_file_another_program_moved_a_new_file_over_while_it_was_locked_is_never_replaced_by_the_lock() {
    let other = b"another program's file".to_vec();
    let moved_over = |path: &Path| {
        let newer = path.with_extension("newer");
        std::fs::write(&newer, &other).unwrap();
        std::fs::rename(&newer, path).unwrap();
    };
    let replaced = |path: &Path| format!("{} was replaced by another program while rvr held it: run the command again", path.display());
    // A file moved over it by the lock (as rvr empty and rvr refill do): refused.
    let path = written(&golden(), "replaced-under-lock");
    let locked = Locked::take(&path).unwrap();
    moved_over(&path.0);
    let staged = written(&golden(), "replaced-staged");
    let said = locked.replace(&staged.0).unwrap_err().to_string();
    assert!(said.contains(&replaced(&path.0)), "{said}");
    assert_eq!((std::fs::read(&path).unwrap(), std::fs::read(&staged).unwrap()), (other.clone(), golden()));
    // A change written anew, as for a file it may not write: refused, nothing left beside it.
    let path = written(&golden(), "replaced-under-change");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let locked = Locked::take(&path).unwrap();
    assert!(locked.anew().is_some());
    moved_over(&path.0);
    let said = set_tool(&locked, "k", json!({})).unwrap_err().to_string();
    assert!(said.contains(&replaced(&path.0)), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), other);
    assert!(staged_beside(&path).is_empty() && !journal(&path).exists());
}

// SPEC 8.12
#[test]
fn refused_opens_of_a_file_another_thread_holds_leave_no_descriptor_behind() {
    let path = written(&golden(), "no-leak");
    let _locked = Locked::take(&path).unwrap();
    let open = || std::fs::read_dir("/dev/fd").unwrap().count();
    let before = open();
    std::thread::scope(|threads| {
        threads.spawn(|| {
            for _ in 0..1000 {
                assert!(Reader::open(&path).unwrap_err().to_string().contains("another of its threads holds its lock"));
                assert!(validate_file(&path).unwrap_err().to_string().contains("another of its threads holds its lock"));
            }
        });
    });
    // Other tests open and close files as this runs: a leak would be a thousand more, or two.
    let after = open();
    assert!(after < before + 50, "{before} descriptors open before, {after} after");
}

// SPEC 8.12
#[test]
#[cfg(all(not(debug_assertions), unix))]
fn a_release_build_refuses_a_file_opened_again_while_it_is_locked_as_a_bug() {
    let path = written(&golden(), "opened-again-release");
    let _locked = Locked::take(&path).unwrap();
    let said = Reader::open(&path).unwrap_err().to_string();
    assert_eq!(said, format!("a bug in rowver: {} was opened again while this program holds it locked; please report it", path.0.display()));
}

// SPEC 8.12
#[test]
fn a_file_one_thread_of_a_program_holds_locked_is_refused_to_its_others_before_the_system_is_asked() {
    let path = written(&golden(), "two-threads");
    let locked = Locked::take(&path).unwrap();
    let told = format!("{} is being changed by this program already (another of its threads holds its lock, or is waiting for it): wait for that change to finish", path.0.display());
    // Where the system would give the lock to both (POSIX locks), the second is refused all the
    // same; opened to be read, too, which would let the lock go when it closed.
    std::thread::scope(|threads| {
        threads.spawn(|| {
            assert_eq!(Locked::take(&path).unwrap_err().to_string(), told);
            assert_eq!(Reader::open(&path).unwrap_err().to_string(), told);
            assert_eq!(set(&path, "k", b"{}").unwrap_err().to_string(), told);
        });
    });
    assert!(locked.reader().is_ok(), "the first holds it still");
    drop(locked);
    // Let go, it is taken by any thread.
    std::thread::scope(|threads| {
        threads.spawn(|| set(&path, "k", b"{}").unwrap());
    });
    assert_eq!(header(&path)["k"], json!({}));
}

// FORMAT 1, 7; SPEC 8.12
#[test]
#[cfg(debug_assertions)]
#[ignore = "writes 5 GB: RVR_BIG_DIR=FOLDER cargo test -p rowver --test change -- --ignored"]
fn a_column_is_added_in_place_to_a_2_gb_file_reading_none_of_its_arrays() {
    let _serial = READ_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::path::PathBuf::from(std::env::var("RVR_BIG_DIR").expect("RVR_BIG_DIR names a folder with 5 GB free"));
    let path = common::Written(dir.join("big.rvr"));
    let spooled = common::Written(dir.join("big.spool"));
    // One f8 column of values that do not compress, 2^28 of them, and its entries, spooled a
    // piece at a time so that nothing whole is held: 2 GiB of arrays and more.
    let (rows, piece) = (1_usize << 28, 1_usize << 22);
    let spool = rowver::write::Spool::create(&spooled.0).unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for at in (0..rows).step_by(piece) {
        let noise: Vec<u64> = (0..piece).map(|_| { state ^= state << 13; state ^= state >> 7; state ^= state << 17; state & 0x7fef_ffff_ffff_ffff }).collect();
        let noise: Vec<f64> = noise.into_iter().map(f64::from_bits).collect();
        spool.add(0, (0, at / piece), Values::Flat(array(&noise))).unwrap();
        spool.add(1, (0, at / piece), Values::Flat(array(&(at as i64..(at + piece) as i64).collect::<Vec<_>>()))).unwrap();
    }
    let source = |n: usize| Source::Spooled(spool.take(n).unwrap().unwrap());
    let written = rowver::write::Rows {
        samples: vec![rowver::write::Sample { name: "A".into(), label: "A".into(), rows }],
        files: vec![json!({"sample": "A", "path": "./a.root", "rows": rows})],
        entry: source(1),
        chunk_rows: 1 << 24,
    };
    let (data, laid) = rowver::write::stream(written, vec![("noise".into(), source(0))], 4, &path.0).unwrap();
    rowver::write::write_laid(&json!({"rowver": 15, "data": data}), laid).unwrap();
    drop(spool);
    drop(spooled);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > 2_000_000_000, "{size}");
    let log = dir.join("big.reads");
    let _log = common::Written(log.clone());
    std::env::set_var("RVR_READ_LOG", &log);
    let started = std::time::Instant::now();
    let locked = Locked::take(&path).unwrap();
    let mut change = Change::begin(&locked).unwrap();
    let records = change.pack(vec![("small".to_owned(), Source::Values(Values::Flat(array(&vec![1_u8; rows]))))], 4).unwrap();
    let mut header = change.header().clone();
    header["data"]["cols"].as_array_mut().unwrap().extend(records);
    rowver::write::data_sealed(&mut header["data"]).unwrap();
    change.finish(&header).unwrap();
    drop(locked);
    let took = started.elapsed();
    assert_eq!(reads(&log, &path.0), [], "bytes of the arrays it left were read");
    rows_are_logged(&log, &path.0);
    std::env::remove_var("RVR_READ_LOG");
    let after = std::fs::metadata(&path).unwrap().len();
    eprintln!("a column added in place to a {:.2} GB file ({:.2} GB after) in {:.2} s", size as f64 / 1e9, after as f64 / 1e9, took.as_secs_f64());
}

/// The header length a file's last 12 bytes state (FORMAT §1), read without reading the file.
fn length_of(path: impl AsRef<Path>) -> u64 {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).unwrap();
    file.seek(SeekFrom::End(-12)).unwrap();
    let mut four = [0; 4];
    file.read_exact(&mut four).unwrap();
    u32::from_le_bytes(four).into()
}

// FORMAT 1, 11; SPEC 8.11, 8.12
#[test]
#[ignore = "writes a 4 GiB header and holds about 16 GB while it does: cargo test --release -p rowver --test change -- --ignored two_to_the_32"]
fn a_tool_key_that_makes_the_header_two_to_the_32_bytes_is_refused_and_one_byte_fewer_is_set() {
    let path = written(&golden(), "huge-tool");
    let _journal = common::Written(journal(&path));
    // The key's object replaced where it lies: the header grows by the text's bytes alone.
    set(&path, "huge", br#"{"s": ""}"#).unwrap();
    let most = (1_u64 << 32) - 1;
    let fill = (most - length_of(&path)) as usize;
    set_tool(&Locked::take(&path).unwrap(), "huge", json!({"s": "a".repeat(fill)})).unwrap();
    assert_eq!(length_of(&path), most);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(Reader::open(&path).is_ok_and(|reader| reader.header()["huge"]["s"].as_str().map(str::len) == Some(fill)));
    let said = set_tool(&Locked::take(&path).unwrap(), "huge", json!({"s": "a".repeat(fill + 1)})).unwrap_err().to_string();
    assert!(said.ends_with("the header would be 4,294,967,296 bytes, more than the 4,294,967,295 (2^32 − 1) its length's four bytes hold (docs/FORMAT.md §1): give the tool key less to hold"), "{said}");
    assert_eq!((std::fs::metadata(&path).unwrap().len(), length_of(&path)), (size, most), "the file as it was");
}
