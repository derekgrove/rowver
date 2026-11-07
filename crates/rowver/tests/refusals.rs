//! One refusal per rule of docs/FORMAT.md:
//! the fixture, a valid file, changed in one way, and refused on one line that names the place.
//! The rules a reader checks on opening are refused by `Reader::open`; the rest only by full
//! validation, which opens the file first.

mod common;

use common::{rebuilt, sealed, to_bytes, written};
use rowver::rvr::{validate_file, Reader};
use rowver::study::lock::{Branches, Lock};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// How one row changes the fixture.
#[derive(Clone, Copy)]
enum Change {
    /// The header, sealed after: its sample columns and `made.sha256` made again.
    Header(fn(&mut Value)),
    /// The header, nothing made again: a checksum's own rule.
    Raw(fn(&mut Value)),
    /// The header, `made.sha256` made again but not the sample columns.
    Made(fn(&mut Value)),
    /// The header and the arrays; every `at` and `size`, and `data.sha256`, made again, then sealed.
    Arrays(fn(&mut Value, &mut Vec<Vec<u8>>)),
    /// The header's JSON text.
    Text(fn(&mut String)),
    /// The arrays, laid end to end in record order and `data.sha256` made again, then laid as `f`
    /// says: it sets their `at`s and gives the bytes from byte 8 to the header; then sealed.
    Placed(fn(&mut Value, &[Vec<u8>]) -> Vec<u8>),
    /// The fixture with a journal beside it (`FILE.rvr-journal`, FORMAT §12): a change cut short.
    Journaled,
    /// The file's bytes.
    Bytes(fn(&mut Vec<u8>)),
    /// The header, sealed, of the file emptied.
    Empty(fn(&mut Value)),
    /// The lock, written again.
    Lock(fn(&mut Lock)),
    /// The lock's text.
    LockText(fn(&mut String)),
    /// The samples.toml text.
    Samples(fn(&mut String)),
    /// The recipe's text.
    Recipe(fn(&mut String)),
    /// The fixture signed, then its header changed.
    Signed(fn(&mut Value)),
    /// The fixture with two derived columns, `twice = "MET * 2"` and `muons = "Muon_pt * 2"`, sealed after.
    Derived(fn(&mut Value)),
    /// As `Derived`, of the file emptied.
    EmptyDerived(fn(&mut Value)),
    /// Sample B's file split in two (`two_files`), the lock's two takes swapped; its own choice the
    /// file's (true), or another seed (false).
    SwappedTakes(bool),
    /// The fixture of no rows (`rowless`), its header changed and `data.sha256` made again, sealed.
    Rowless(fn(&mut Value)),
}

use Change::*;

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The rows of each of the fixture's chunks (FORMAT §7): its samples' rows cut every chunk_rows.
fn chunk_rows(header: &Value) -> Vec<(usize, usize, usize)> {
    let r = header["data"]["chunk_rows"].as_u64().unwrap() as usize;
    let (mut out, mut at) = (Vec::new(), 0);
    for (sample, record) in header["data"]["samples"].as_array().unwrap().iter().enumerate() {
        let end = at + record["rows"].as_u64().unwrap() as usize;
        while at < end {
            let to = (at + r).min(end);
            out.push((sample, at, to));
            at = to;
        }
    }
    out
}

/// One chunk as stored: each lane's frame, and the chunk's SHA-256.
type Stored = (Vec<Vec<u8>>, Vec<u8>);

/// A chunk's values `w` wide stored as FORMAT §7 says: each byte lane one zstd frame.
fn chunk(values: &[u8], w: usize) -> Stored {
    let lanes = (0..w).map(|j| ruzstd::encoding::compress_to_vec(&values.iter().skip(j).step_by(w).copied().collect::<Vec<u8>>()[..], ruzstd::encoding::CompressionLevel::Fastest)).collect();
    (lanes, Sha256::digest(values).to_vec())
}

/// An array laid from its stored chunks: their frames, then the chunk table of their sizes and SHA-256s.
fn laid(chunks: &[Stored]) -> Vec<u8> {
    let (mut out, mut table) = (Vec::new(), Vec::new());
    for (lanes, digest) in chunks {
        for frame in lanes {
            table.extend((frame.len() as u64).to_le_bytes());
            out.extend(frame);
        }
        table.extend(digest);
    }
    out.extend(table);
    out
}

/// An array's stored chunks, as its chunk table lays them out, for values `w` wide.
fn unlaid(array: &[u8], chunks: usize, w: usize) -> Vec<Stored> {
    let table = &array[array.len() - chunks * (8 * w + 32)..];
    let mut at = 0;
    table
        .chunks(8 * w + 32)
        .map(|entry| {
            let lanes = (0..w)
                .map(|j| {
                    let size = u64::from_le_bytes(entry[8 * j..8 * j + 8].try_into().unwrap()) as usize;
                    at += size;
                    array[at - size..at].to_vec()
                })
                .collect();
            (lanes, entry[8 * w..].to_vec())
        })
        .collect()
}

/// Change array `index`'s stored chunks (values `w` wide) as `f` says, and lay it again.
fn edit(header: &Value, arrays: &mut [Vec<u8>], index: usize, w: usize, f: impl FnOnce(&mut Vec<Stored>)) {
    let mut chunks = unlaid(&arrays[index], chunk_rows(header).len(), w);
    f(&mut chunks);
    arrays[index] = laid(&chunks);
}

/// The SHA-256 of these SHA-256s' hex texts joined (FORMAT §8).
fn joined(digests: &[&[u8]]) -> String {
    sha(digests.iter().map(|d| d.iter().map(|b| format!("{b:02x}")).collect::<String>()).collect::<String>().as_bytes())
}

/// The record of the fixture's array `index`, in file order.
fn record(header: &mut Value, index: usize) -> &mut Value {
    match index {
        0 => &mut header["data"]["sample"],
        1 => &mut header["data"]["entry"],
        2 | 3 => &mut header["data"]["cols"][index - 2],
        4 => &mut header["data"]["cols"][2]["offsets"],
        5 => &mut header["data"]["cols"][2]["content"],
        n => &mut header["data"]["cols"][n - 3],
    }
}

/// Array `index` holding these little-endian values `w` wide, cut at the fixture's chunks (each
/// chunk of a list's offsets counted from its first): its record's sha256 and by_sample theirs.
fn put(header: &mut Value, arrays: &mut [Vec<u8>], index: usize, raw: &[u8], w: usize) {
    let rows = chunk_rows(header);
    let at = |row: usize| (row * w).min(raw.len());
    let chunks: Vec<Stored> = rows
        .iter()
        .map(|&(_, a, b)| match index {
            4 => {
                let steps: Vec<i64> = raw[at(a)..at(b + 1)].chunks(8).map(|v| i64::from_le_bytes(v.try_into().unwrap())).collect();
                chunk(&steps.iter().flat_map(|v| (v - steps[0]).to_le_bytes()).collect::<Vec<u8>>(), w)
            }
            _ => chunk(&raw[at(a)..at(b)], w),
        })
        .collect();
    arrays[index] = laid(&chunks);
    let digests: Vec<&[u8]> = chunks.iter().map(|(_, d)| d.as_slice()).collect();
    let held = record(header, index);
    held["sha256"] = joined(&digests).into();
    if held.get("by_sample").is_some() {
        let samples = header["data"]["samples"].as_array().unwrap().len();
        let by: Vec<String> = (0..samples).map(|s| joined(&rows.iter().zip(&digests).filter(|(r, _)| r.0 == s).map(|(_, d)| *d).collect::<Vec<_>>())).collect();
        record(header, index)["by_sample"] = json!(by);
    }
}

/// Every array's `at` and `size` from the arrays, and `data.sha256`, made again.
fn relaid(header: &mut Value, arrays: &[Vec<u8>]) {
    let mut at = 0;
    for (index, array) in arrays.iter().enumerate() {
        let held = record(header, index);
        held["at"] = at.into();
        held["size"] = array.len().into();
        at += array.len();
    }
    let mut sums = String::new();
    for index in 0..arrays.len() {
        sums += record(header, index)["sha256"].as_str().unwrap();
    }
    header["data"]["sha256"] = sha(sums.as_bytes()).into();
}

/// A text of the study put in `made`, with its checksum.
fn set_text(header: &mut Value, name: &str, text: &str) {
    header["made"][name]["text"] = text.into();
    header["made"][name]["sha256"] = sha(text.as_bytes()).into();
}

/// The fixture with two derived columns, worked out from its arrays.
fn derived() -> (Value, Vec<Vec<u8>>) {
    derived_of(rebuilt(10.))
}

/// A file of the fixture's study with two derived columns, worked out from its arrays.
fn derived_of((mut header, arrays): (Value, Vec<Vec<u8>>)) -> (Value, Vec<Vec<u8>>) {
    let recipe = common::recipe() + "twice = \"MET * 2\"\nmuons = \"Muon_pt * 2\"\n";
    set_text(&mut header, "recipe", &recipe);
    let header = sealed(header);
    let path = written(&to_bytes(&header, &arrays), "derived");
    let reader = Reader::open(&path).unwrap();
    let wanted = [("twice".to_owned(), "MET * 2".to_owned()), ("muons".to_owned(), "Muon_pt * 2".to_owned())];
    let records = rowver::rvr::derived::records(&reader, &wanted, &[]).unwrap();
    let mut header = header;
    header["derived"] = json!({ "columns": records });
    (header, arrays)
}

/// The fixture with sample B's file split in two, of 11 and 9 entries: its four chosen entries
/// (1, 8, 10, 17) are 1, 8 and 10 of the first and 6 of the second, and its rows entries 1 and 6.
/// The lock chooses as the file did (`same`), or by another seed, one whose takes differ; with
/// `swapped`, the lock's two takes trade places, their sum unchanged.
fn two_files(header: &mut Value, arrays: &mut [Vec<u8>], same: bool, swapped: bool) {
    let entries = [11, 9];
    let seed = match same {
        true => 7,
        false => (8..).find(|&s| { let c = rowver::choice::per_file(&entries, 4, s); c[0].len() != c[1].len() }).unwrap(),
    };
    let samples = common::SAMPLES
        .replace("files = \"./fixture-b.root\"", "files = [\"./fixture-b.root\", \"./fixture-b2.root\"]")
        .replace("seed = 7", &format!("seed = {seed}"));
    let lock = common::lock_with(|lock| {
        lock.samples_toml_sha256 = rowver::write::text_checksum(&samples);
        let b = &mut lock.samples[1];
        b.asked = vec!["./fixture-b.root".into(), "./fixture-b2.root".into()];
        b.files[0].entries = entries[0];
        let mut second = b.files[0].clone();
        (second.at, second.bytes, second.adler32, second.uuid, second.entries) =
            ("./fixture-b2.root".into(), 5005, Some("0a1b2c05".into()), "31e851e4-b994-11f1-ba5c-000000000005".into(), entries[1]);
        b.files.push(second);
        b.events = rowver::choice::Events::Random { n: 4, seed };
        *b = b.planned(b.events);
        if swapped {
            let (first, second) = (b.files[0].take, b.files[1].take);
            (b.files[0].take, b.files[1].take) = (second, first);
        }
    });
    set_text(header, "samples", &samples);
    set_text(header, "lock", &lock);
    let taken: Vec<usize> = rowver::choice::per_file(&entries, 4, 7).iter().map(Vec::len).collect();
    let files = &mut header["made"]["files"]["B"]["files"];
    files["./fixture-b.root"]["entries"] = entries[0].into();
    files["./fixture-b.root"]["take"] = taken[0].into();
    files["./fixture-b2.root"] = json!({"tree": "Events", "uuid": "31e851e4-b994-11f1-ba5c-000000000005", "entries": entries[1], "take": taken[1]});
    let listed = header["data"]["files"].as_array_mut().unwrap();
    listed[3]["rows"] = 1.into();
    listed.push(json!({"sample": "B", "path": "./fixture-b2.root", "rows": 1}));
    let entry = |values: &[i64]| values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
    put(header, arrays, 1, &entry(&[0, 2, 4, 1, 6]), 8);
    relaid(header, arrays);
}

/// The fixture's bytes with `change` made to them.
fn bytes(change: Change) -> Vec<u8> {
    bytes_of(rebuilt(10.), change)
}

/// The bytes of `base` (the fixture, or a variant of its values) with `change` made to them.
fn bytes_of(base: (Value, Vec<Vec<u8>>), change: Change) -> Vec<u8> {
    let (mut header, mut arrays) = base;
    let lock = |header: &mut Value, text: String| set_text(header, "lock", &text);
    match change {
        Header(f) => {
            f(&mut header);
            header = sealed(header);
        }
        Raw(f) => f(&mut header),
        Made(f) => {
            f(&mut header);
            header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
        }
        Arrays(f) => {
            f(&mut header, &mut arrays);
            relaid(&mut header, &arrays);
            header = sealed(header);
        }
        Text(f) => {
            let mut text = serde_json::to_string(&header).unwrap();
            f(&mut text);
            return common::framed(text.as_bytes(), &arrays.concat());
        }
        Placed(f) => {
            relaid(&mut header, &arrays);
            let region = f(&mut header, &arrays);
            return common::framed(serde_json::to_string(&sealed(header)).unwrap().as_bytes(), &region);
        }
        Journaled => {}
        Bytes(f) => {
            let mut out = to_bytes(&header, &arrays);
            f(&mut out);
            return out;
        }
        Empty(f) => {
            f(&mut header);
            return to_bytes(&sealed(header), &[]);
        }
        Lock(f) => {
            lock(&mut header, common::lock_with(f));
            header = sealed(header);
        }
        LockText(f) => {
            let mut text = common::lock();
            f(&mut text);
            lock(&mut header, text);
            header = sealed(header);
        }
        Samples(f) => {
            let mut text = common::SAMPLES.to_owned();
            f(&mut text);
            set_text(&mut header, "samples", &text);
            header = sealed(header);
        }
        Recipe(f) => {
            let mut text = common::recipe();
            f(&mut text);
            set_text(&mut header, "recipe", &text);
            header = sealed(header);
        }
        Signed(f) => {
            rowver::sign::sign(&mut header, &[3; 32]).unwrap();
            f(&mut header);
        }
        SwappedTakes(same) => {
            two_files(&mut header, &mut arrays, same, true);
            header = sealed(header);
        }
        Rowless(f) => {
            let (mut none, arrays) = common::rowless();
            f(&mut none);
            relaid(&mut none, &arrays);
            return to_bytes(&sealed(none), &arrays);
        }
        Derived(f) | EmptyDerived(f) => {
            let (mut with, arrays) = derived();
            f(&mut with);
            let arrays = if matches!(change, EmptyDerived(_)) { Vec::new() } else { arrays };
            return to_bytes(&sealed(with), &arrays);
        }
    }
    to_bytes(&header, &arrays)
}

/// Whether the file of these bytes opens, and whether it validates in full: each refusal said.
fn checked(bytes: &[u8]) -> (Result<(), String>, Result<(), String>) {
    checked_with(bytes, false)
}

/// The same, with a journal beside the file when `journal`: a change to it cut short (FORMAT §12).
fn checked_with(bytes: &[u8], journal: bool) -> (Result<(), String>, Result<(), String>) {
    let path = written(bytes, "refusal");
    let beside = common::journal(&path);
    if journal {
        std::fs::write(&beside, b"a change cut short").unwrap();
    }
    let _gone = common::Written(beside);
    // A refusal that names the file names it as FILE.rvr, the same wherever the test runs.
    let said = |e: rowver::Error| e.to_string().replace(&*path.0.to_string_lossy(), "FILE.rvr");
    let opened = Reader::open(&path).map(drop).map_err(said);
    let validated = validate_file(&path).map(drop).map_err(said);
    (opened, validated)
}

/// A rule, by its section of docs/FORMAT.md; the change that breaks it; what its refusal says (of
/// the fixture: a generated file's says what comes before a `|`, its values its own).
type Row = (&'static str, &'static str, Change, &'static str);

/// Every row refused as `opening` says (by `Reader::open` too, or only by full validation), with
/// a message on one line holding its `said`.
fn refused(rows: Vec<Row>, opening: bool) {
    refused_of(|| rebuilt(10.), rows, opening, "")
}

/// The same, of `base`'s file, each failure said with `case`.
fn refused_of(base: impl Fn() -> (Value, Vec<Vec<u8>>), rows: Vec<Row>, opening: bool, case: &str) {
    let mut wrong = Vec::new();
    for (section, rule, change, said) in rows {
        let said = match case.is_empty() {
            true => said.replace('|', ""),
            false => said.split('|').next().unwrap_or(said).to_owned(),
        };
        let said = said.as_str();
        let (opened, validated) = checked_with(&bytes_of(base(), change), matches!(change, Journaled));
        let problem = match (&opened, &validated) {
            (_, Ok(())) => Some("accepted".to_owned()),
            (Ok(()), _) if opening => Some("opened".to_owned()),
            (Err(e), _) if !opening => Some(format!("refused on opening: {e}")),
            (_, Err(e)) if !e.contains(said) || e.contains('\n') => Some(format!("said: {e}")),
            _ => None,
        };
        if let Some(problem) = problem {
            wrong.push(format!("{case}{section} {rule}: {problem}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Where a file's header starts, as its last 12 bytes say (FORMAT §1).
fn header_start(bytes: &[u8]) -> usize {
    let n = bytes.len();
    n - 12 - u32::from_le_bytes(bytes[n - 12..n - 8].try_into().unwrap()) as usize
}

/// The arrays laid in `order` (indices in record order), each record's `at` where it then lies.
fn reordered(header: &mut Value, arrays: &[Vec<u8>], order: &[usize]) -> Vec<u8> {
    assert_eq!(order.len(), arrays.len());
    let mut region = Vec::new();
    for &index in order {
        record(header, index)["at"] = region.len().into();
        region.extend(&arrays[index]);
    }
    region
}

/// Nested arrays `depth` deep.
fn nested(depth: usize) -> Value {
    (1..depth).fold(json!([]), |inner, _| json!([inner]))
}

/// A hole of the fixture's records.
fn hole(first: u64, last: u64) -> Value {
    json!({"entries": [first, last], "kind": "damaged", "why": "basket 2 does not decompress"})
}

/// The file of no rows with a total, sumw, and every entry it chose in a hole, so no file holds
/// the total: each sample's sumw `a` and `b`.
fn holed_with_total(h: &mut Value, a: Value, b: Value) {
    let text = common::recipe().replace("[totals]\n", "[totals]\nsumw = \"MET\"\n");
    set_text(h, "recipe", &text);
    for (sample, path, last) in [("A", "./fixture-a.root", 10), ("A", "./fixture-empty.root", 10), ("A", "./fixture-a2.root", 5), ("B", "./fixture-b.root", 20)] {
        h["made"]["files"][sample]["files"][path]["holes"] = json!([hole(0, last)]);
    }
    for (sample, missing, total) in [("A", 25, a), ("B", 4, b)] {
        let metadata = &mut h["made"]["files"][sample]["metadata"];
        (metadata["events_read"], metadata["events_missing"], metadata["sumw"]) = (0.into(), missing.into(), total);
    }
}

/// A frame made a single segment that declares a content size of `declared` bytes.
fn sized(frame: &[u8], declared: u8) -> Vec<u8> {
    let mut out = frame[..5].to_vec();
    out[4] = (out[4] & !0xc0) | 0x20;
    out.push(declared);
    out.extend(&frame[6..]);
    out
}

/// The branch list of the lock with MET's kind changed, every sample naming it.
fn met_as(lock: &mut Lock, kind: &str) {
    let mut list = lock.branches[0].list.clone();
    list.iter_mut().filter(|b| b.name == "MET").for_each(|b| b.kind = kind.into());
    let list = Branches::new(list);
    for sample in &mut lock.samples {
        sample.branches = list.sha256.clone();
    }
    lock.branches = vec![list];
}

const ZEROS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

// FORMAT 1, 10, 12
#[test]
fn the_fixture_itself_is_valid() {
    assert_eq!(checked(&common::golden()), (Ok(()), Ok(())));
    let (header, arrays) = derived();
    assert_eq!(checked(&to_bytes(&header, &arrays)), (Ok(()), Ok(())));
    assert_eq!(checked(&to_bytes(&header, &[])), (Ok(()), Ok(())));
    assert_eq!(checked(&to_bytes(&rebuilt(10.).0, &[])), (Ok(()), Ok(())));
}

// FORMAT 1, 4, 7, 8, 9, 11
#[test]
fn what_the_format_allows_is_valid() {
    let wrong: Vec<String> = allowed().into_iter().filter_map(|(rule, change)| match checked(&bytes(change)) {
        (Ok(()), Ok(())) => None,
        said => Some(format!("{rule}: {said:?}")),
    }).collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Files the format allows, each the fixture changed one way.
fn allowed() -> Vec<(&'static str, Change)> {
    vec![
        ("§1 nesting to 64 deep", Raw(|h| h["fixture_tool"]["deep"] = nested(62))),
        ("§7 arrays in any order: the last record's first", Placed(|h, a| reordered(h, a, &(0..a.len()).rev().collect::<Vec<_>>()))),
        ("§7 arrays in any order: a list column's content before its offsets", Placed(|h, a| reordered(h, a, &(0..a.len()).map(|i| match i { 4 => 5, 5 => 4, i => i }).collect::<Vec<_>>()))),
        ("§5 a sample column's numbers compared as numbers: in an f8 one, as the doubles they read as", Raw(|h| {
            let cols = h["data"]["sample_cols"].as_array_mut().unwrap();
            let seed = cols.iter_mut().find(|c| c["name"] == "seed_like").unwrap();
            assert_eq!(seed["kind"], "f8");
            seed["values"][1] = json!(9.223372036854776e18);
        })),
        ("§4 whole facts in another spelling", Header(|h| h["data"]["cols"][0]["lim"] = json!([0.0, 3e0]))),
        ("§4 -0.0 is 0.0", Header(|h| h["data"]["cols"][1]["lim"][0] = json!(-0.0))),
        ("§4 a flat float column of NaN and infinities", Arrays(|h, a| {
            let raw: Vec<u8> = [f32::NAN, 0., f32::INFINITY, 40., f32::NEG_INFINITY].iter().flat_map(|v| v.to_le_bytes()).collect();
            put(h, a, 3, &raw, 4);
        })),
        ("§3 §7 a file of no rows, its arrays of no bytes", Rowless(|_| {})),
        ("§6 a total no file holds, 0.0 on every sample", Rowless(|h| holed_with_total(h, json!(0.0), json!(0.0)))),
        ("§7 a frame declaring its content size", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0] = sized(&c[0].0[0], 3)))),
        ("§3 §7 a chunk_rows the recipe gives", Header(|h| {
            h["data"]["chunk_rows"] = 1024.into();
            let text = h["made"]["recipe"]["text"].as_str().unwrap().replacen("version = 1\n", "version = 1\nchunk_rows = 1024\n", 1);
            set_text(h, "recipe", &text);
        })),
        ("§3 §7 a chunk_rows of 16,777,216, the most", Header(|h| {
            h["data"]["chunk_rows"] = 16_777_216.into();
            let text = h["made"]["recipe"]["text"].as_str().unwrap().replacen("version = 1\n", "version = 1\nchunk_rows = 16777216\n", 1);
            set_text(h, "recipe", &text);
        })),
        ("§11 tool keys of every allowed character", Raw(|h| h["A_b-9.z"] = json!({"x": [null, {}]}))),
        ("§11 a tool key two edits from data", Raw(|h| h["dash"] = json!({}))),
        ("§11 a tool key two edits from made", Raw(|h| h["mile"] = json!({}))),
        ("§11 a tool key two edits from rowver", Raw(|h| h["router"] = json!({}))),
        ("§11 a tool key two edits from derived", Raw(|h| h["driver"] = json!({}))),
        ("§11 a tool key two edits from signature", Raw(|h| h["signatory"] = json!({}))),
        ("§11 a tool key far from every format key", Raw(|h| h["view"] = json!({}))),
        ("§11 a tool key four edits from signature", Raw(|h| h["signal"] = json!({}))),
        ("§9 a signed file", Signed(|_| {})),
    ]
}

// FORMAT 1, 2, 3, 4, 5, 6, 6.1, 6.2, 7, 8, 9, 10, 11, 12
#[test]
fn a_reader_refuses_on_opening_each_rule_it_checks_there() {
    refused(opening(), true);
}

/// The rules a reader checks on opening, each with a change that breaks it alone.
fn opening() -> Vec<Row> {
    vec![
        // §1 The file and the header's JSON
        ("§1", "the magic", Bytes(|b| b[0] ^= 1), "it is not an .rvr file"),
        // A whole number written -0, refused wherever it is (the header's text written so).
        ("§1", "no whole number written -0: a count", Text(|t| *t = t.replacen("\"path\":\"./fixture-empty.root\",\"rows\":0", "\"path\":\"./fixture-empty.root\",\"rows\":-0", 1)), "the header breaks docs/FORMAT.md §1: data.files[1].rows is written -0 (line 1, column "),
        ("§1 §6.1", "no whole number written -0: a file's stop", Text(|t| *t = t.replacen("\"entries\":10}", "\"entries\":10,\"stop\":-0}", 1)), "made.files[\"A\"].files[\"./fixture-a.root\"].stop is written -0"),
        ("§1 §6.1", "no whole number written -0: a seed", Text(|t| *t = t.replacen("\"seed\":7", "\"seed\":-0", 1)), "made.files[\"B\"].events.seed is written -0"),
        ("§1 §6", "no whole number written -0: metadata", Text(|t| *t = t.replacen("\"seed_like\":9223372036854775807", "\"seed_like\":-0", 1)), "made.files[\"B\"].metadata.seed_like is written -0"),
        ("§1 §11", "no whole number written -0: a tool key's object", Text(|t| *t = t.replacen("\"fixture_tool\":{\"preserved\":\"μ\"}", "\"fixture_tool\":{\"preserved\":\"μ\",\"x\":[1,-0]}", 1)), "fixture_tool.x[1] is written -0"),
        ("§5 §6", "a metadata whole number at most 2^63 − 1", Header(|h| h["made"]["files"]["A"]["metadata"]["big"] = serde_json::from_str("9223372036854775808").unwrap()), "made.files[\"A\"].metadata.big is 9223372036854775808, and a whole number of metadata is from −2^63 to 2^63 − 1"),
        ("§5 §6", "a metadata whole number at least −2^63", Header(|h| h["made"]["files"]["B"]["metadata"]["small"] = json!([serde_json::from_str::<Value>("-9223372036854775809").unwrap()])), "made.files[\"B\"].metadata.small[0] is -9223372036854775809, and a whole number of metadata is from −2^63 to 2^63 − 1"),
        // §3 and §6: what the header alone shows of the files read and the totals
        ("§3 R17", "the lock's files, leaving out only those never reached", Header(|h| {
            h["data"]["files"].as_array_mut().unwrap().insert(3, json!({"sample": "A", "path": "./fixture-a3.root", "rows": 0}));
            h["made"]["files"]["A"]["files"]["./fixture-a3.root"] = json!({"tree": "Events", "uuid": "31e851e4-b994-11f1-ba5c-000000000009", "entries": 10, "stop": 0});
        }), "made.files[\"A\"].events: the file ./fixture-a3.root is listed, and the first N never reach it"),
        ("§6 R16", "totals only when the recipe has one", Header(|h| { h["made"]["files"]["A"]["files"]["./fixture-a.root"]["totals"] = json!({"sumw": 1.0}); h["made"]["files"]["A"]["metadata"]["sumw"] = json!(1.0) }), "made.files[\"B\"].metadata has no total sumw, and every sample's metadata holds each total a file record holds"),
        ("§6", "a sample's total of no file is 0.0, written as a double", Header(|h| {
            let text = common::recipe().replace("[totals]\n", "[totals]\nsumw = \"MET\"\n");
            set_text(h, "recipe", &text);
            for path in ["./fixture-a.root", "./fixture-empty.root", "./fixture-a2.root"] {
                h["made"]["files"]["A"]["files"][path]["totals"] = json!({"sumw": 1.0});
            }
            h["made"]["files"]["A"]["metadata"]["sumw"] = json!(3.0);
            h["made"]["files"]["B"]["files"]["./fixture-b.root"]["holes"] = json!([hole(0, 20)]);
            h["made"]["files"]["B"]["metadata"]["sumw"] = json!(0);
            h["made"]["files"]["B"]["metadata"]["events_read"] = 0.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 4.into();
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 0.into();
        }), "made.files[\"B\"].metadata.sumw is 0, and a total is a finite double, written with a . or an exponent"),
        ("§6 R16", "no totals on a file read none of", Header(|h| {
            h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([hole(0, 10)]);
            h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["totals"] = json!({"sumw": 0.0});
            h["made"]["files"]["A"]["metadata"]["sumw"] = json!(0.0);
            h["made"]["files"]["A"]["metadata"]["events_read"] = 15.into();
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 10.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into();
        }), "made.files[\"B\"].metadata has no total sumw, and every sample's metadata holds each total a file record holds"),
        // §3 and §7: chunks
        ("§3", "chunk_rows there", Header(|h| { h["data"].as_object_mut().unwrap().remove("chunk_rows"); }), "data.chunk_rows is missing"),
        ("§3", "chunk_rows within its bounds", Header(|h| h["data"]["chunk_rows"] = 1023.into()), "data.chunk_rows is 1023, and a chunk holds from 1,024 to 16,777,216 rows"),
        ("§3", "chunk_rows within its bounds: 16,777,217 is one too many", Header(|h| h["data"]["chunk_rows"] = 16_777_217.into()), "data.chunk_rows is 16777217, and a chunk holds from 1,024 to 16,777,216 rows"),
        ("§7 §12", "an array at least its table and a frame a lane", Header(|h| h["data"]["cols"][1]["size"] = 231.into()), "data.cols[\"MET\"].size is 231, and its 2 chunks take at least 232 bytes"),
        ("§7 §8", "no chunk spanning two samples", Arrays(|h, a| {
            let values: Vec<u8> = [20_f32, 0., 40., 30., 10.].iter().flat_map(|v| v.to_le_bytes()).collect();
            a[3] = laid(&[chunk(&values, 4)]);
            h["data"]["cols"][1]["sha256"] = joined(&[Sha256::digest(&values).as_slice()]).into();
        }), "data.cols[\"MET\"].size is 136, and its 2 chunks take at least 232 bytes"),
        ("§1", "the header's length within the file", Bytes(|b| { let n = b.len(); b[n - 12..n - 8].copy_from_slice(&u32::MAX.to_le_bytes()) }), "the header's length (the 4 bytes before the last 8) is more than the file holds"),
        ("§1", "the file ends in the magic", Bytes(|b| { let n = b.len(); b[n - 1] ^= 1 }), "it does not end in the .rvr magic (its last 8 bytes): the file was cut short, or a change to it was"),
        ("§1", "the file ends in the magic: nothing after it", Bytes(|b| b.push(0)), "it does not end in the .rvr magic (its last 8 bytes)"),
        ("§1", "the file at least the magic twice and a length", Bytes(|b| b.truncate(19)), "it is not an .rvr file: it holds 19 bytes, and an .rvr file holds at least 20"),
        ("§12", "no journal beside the file", Journaled, "FILE.rvr: a change to it was cut short (FILE.rvr-journal is beside it): any rvr command on the file puts it back as it was"),
        ("§1", "rowver is 15", Raw(|h| h["rowver"] = 14.into()), "This file is rowver 14"),
        ("§1", "rowver is the whole number 15", Raw(|h| h["rowver"] = json!(15.0)), "rowver must be the whole number 15"),
        ("§1 §7", "the arrays fill the space before the header: no byte left", Bytes(|b| { let at = header_start(b); b.splice(at..at, [0, 0, 0]); }), "the arrays' records end at byte"),
        ("§1 §7", "the arrays fill the space before the header: none missing", Bytes(|b| { let at = header_start(b); b.remove(at - 1); }), "the arrays' records end at byte"),
        ("§7", "no gap between arrays", Placed(|h, a| {
            // One byte more after the sample array, every array after it a byte further on.
            let mut region = a.concat();
            region.insert(a[0].len(), 0);
            for index in 1..a.len() {
                let held = record(h, index);
                held["at"] = (held["at"].as_u64().unwrap() + 1).into();
            }
            region
        }), "data.entry.at is 241, but the arrays before it end at 240: the arrays fill the bytes from 8 to the header with no gap and no overlap"),
        ("§7", "no arrays overlapping", Placed(|h, a| {
            // The sample array's last byte gone, every array after it a byte nearer: it overlaps the next.
            let mut region = a.concat();
            region.remove(a[0].len() - 1);
            for index in 1..a.len() {
                let held = record(h, index);
                held["at"] = (held["at"].as_u64().unwrap() - 1).into();
            }
            region
        }), "data.entry.at is 239, but the arrays before it end at 240: the arrays fill the bytes from 8 to the header with no gap and no overlap"),
        ("§1", "no byte-order mark", Text(|t| t.insert(0, '\u{feff}')), "the header is not JSON as docs/FORMAT.md §1 reads it"),
        ("§1", "one JSON object", Text(|t| *t = format!("[{t}]")), "the header must be one JSON object"),
        ("§1", "no key twice", Text(|t| *t = t.replacen("{\"rows\":", "{\"rows\":5,\"rows\":", 1)), "the header breaks docs/FORMAT.md §1: data.rows is written twice (line 1, column "),
        ("§1 §11", "no key twice in a tool key", Text(|t| *t = t.replace("\"preserved\":\"μ\"", "\"preserved\":\"μ\",\"preserved\":1")), "fixture_tool.preserved is written twice"),
        ("§1 §11", "nesting at most 64", Raw(|h| h["fixture_tool"]["deep"] = nested(63)), "is nested 65 arrays and objects deep, more than 64 (line 1, column "),
        ("§1", "no lone surrogate", Text(|t| *t = t.replace("\"preserved\":\"μ\"", "\"preserved\":\"\\ud800\"")), "the header is not JSON as docs/FORMAT.md §1 reads it"),
        ("§1", "a count has no fraction", Raw(|h| h["data"]["rows"] = json!(5.0)), "data.rows must be a whole number from 0 to 2^63 − 1"),
        ("§1", "a count to 2^63 − 1", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["entries"] = json!(9223372036854775808_u64)), "made.files[\"A\"].files[\"./fixture-a.root\"].entries must be a whole number"),
        ("§1", "a hole's entries are counts", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([{"entries": [1.5, 3], "kind": "damaged", "why": "x"}])), "holes[0].entries must be [first, last]"),
        ("§1 §6.1", "seed from 0 to 2^64 − 1", Header(|h| h["made"]["files"]["B"]["events"]["seed"] = json!(-1)), "made.files[\"B\"].events: events.seed must be a whole number"),
        ("§1 §6.1", "seed from 0 to 2^64 − 1: 2^64 refused", Text(|t| {
            // made.sha256 made again of made as a reader parses it, so that the seed is what is refused.
            *t = t.replacen("\"seed\":7", "\"seed\":18446744073709551616", 1);
            let made = &serde_json::from_str::<Value>(t).unwrap()["made"];
            *t = t.replacen(made["sha256"].as_str().unwrap(), &rowver::write::made_checksum(made).unwrap(), 1);
        }), "made.files[\"B\"].events: events.seed must be a whole number from 0 to 2^64 − 1"),
        // §2 The top level
        ("§2", "rowver, data, made", Raw(|h| drop(h.as_object_mut().unwrap().shift_remove("made"))), "made is missing"),
        ("§2 §11", "each but rowver an object", Raw(|h| h["fixture_tool"] = 5.into()), "fixture_tool must be an object"),
        // §3 Rows
        ("§3", "data has exactly its fields", Header(|h| h["data"]["extra"] = 1.into()), "data has a field \"extra\""),
        ("§3", "rows is the samples' rows summed", Header(|h| h["data"]["rows"] = 6.into()), "data.rows is 6, and the samples' rows add up to 5"),
        ("§3", "a sample is exactly name, label, rows", Header(|h| h["data"]["samples"][0]["x"] = 1.into()), "data.samples[\"A\"] has a field \"x\""),
        ("§3 §3.5.3", "a label is nonempty", Header(|h| h["data"]["samples"][0]["label"] = "".into()), "data.samples[\"A\"].label must be nonempty text"),
        ("§3", "sample names unique", Header(|h| h["data"]["samples"][1]["name"] = "A".into()), "data.samples[\"A\"]: two samples have this name"),
        ("§3", "a data.files record is exactly sample, path, rows", Header(|h| h["data"]["files"][0]["x"] = 1.into()), "data.files[0] has a field \"x\""),
        ("§3", "no file twice", Header(|h| h["data"]["files"][1]["path"] = "./fixture-a.root".into()), "data.files[1]: data.files names ./fixture-a.root of A twice"),
        ("§3", "a path is a key of its made.files", Header(|h| h["data"]["files"][1]["path"] = "./other.root".into()), "made.files[\"A\"].files holds"),
        ("§3", "files in sample order", Header(|h| h["data"]["files"].as_array_mut().unwrap().swap(2, 3)), "data.files[3]: data.files lists each sample's files in the order of data.samples"),
        ("§3", "a sample's files' rows are its rows", Header(|h| h["data"]["files"][0]["rows"] = 1.into()), "data.samples[\"A\"].rows is 3, and its files' rows in data.files add up to 2"),
        ("§1 §3", "counts add up without overflow", Header(|h| {
            h["data"]["files"][0]["rows"] = i64::MAX.into();
            h["data"]["files"][1]["rows"] = i64::MAX.into();
            h["data"]["files"][2]["rows"] = 5.into();
        }), "the rows of made.files[\"A\"]'s files in data.files add up past 2^64"),
        ("§3", "sample_cols always present", Raw(|h| drop(h["data"].as_object_mut().unwrap().shift_remove("sample_cols"))), "data.sample_cols is missing"),
        ("§3 §6", "cols in made.recipe.columns order", Header(|h| h["made"]["recipe"]["columns"].as_array_mut().unwrap().swap(0, 1)), "data.cols holds the columns"),
        // §4 Columns
        ("§4", "a kind of the list", Header(|h| h["data"]["cols"][1]["kind"] = "f16".into()), "data.cols[\"MET\"].kind is \"f16\""),
        ("§4", "jagged content of a flat kind", Header(|h| h["data"]["cols"][2]["content"]["kind"] = "jagged".into()), "data.cols[\"Muon_pt\"].content.kind is \"jagged\""),
        ("§4", "a name is nonempty", Header(|h| h["data"]["cols"][1]["name"] = "".into()), "data.cols[1].name must be nonempty text"),
        ("§4", "no two columns alike", Header(|h| h["data"]["cols"][1]["name"] = "nlep".into()), "data.cols[\"nlep\"]: two columns have this name"),
        ("§4", "no column named sample, entry or file", Header(|h| { h["data"]["cols"][0]["name"] = "entry".into(); h["made"]["recipe"]["columns"][0] = "entry".into() }), "data.cols[\"entry\"]: no column is named sample, entry or file"),
        ("§4", "a flat record is exactly its fields", Header(|h| h["data"]["cols"][1]["x"] = 1.into()), "data.cols[\"MET\"] has a field \"x\""),
        ("§4 F4", "no names fact", Header(|h| h["data"]["cols"][10]["names"] = json!({"0": "off"})), "data.cols[\"flag\"] has a field \"names\""),
        ("§4", "a jagged record is exactly its fields", Header(|h| h["data"]["cols"][2]["x"] = 1.into()), "data.cols[\"Muon_pt\"] has a field \"x\""),
        ("§4", "offsets are exactly their fields", Header(|h| h["data"]["cols"][2]["offsets"]["x"] = 1.into()), "data.cols[\"Muon_pt\"].offsets has a field \"x\""),
        ("§4", "content is exactly its fields", Header(|h| drop(h["data"]["cols"][2]["content"].as_object_mut().unwrap().shift_remove("count"))), "data.cols[\"Muon_pt\"].content.count is missing"),
        ("§4 F3", "nice by the exact decimal rule", Header(|h| h["data"]["cols"][1]["nice"] = json!([0, 100])), "data.cols[\"MET\"].nice is [0,100], and the rule of docs/FORMAT.md §4 makes |[0.0,40.0]"),
        ("§4 F9", "no finite value gives nice [0, 1]", Header(|h| { h["data"]["cols"][1]["lim"] = json!([0.0, 0.0]); h["data"]["cols"][1]["nice"] = json!([0.0, 2.0]) }), "makes [0.0,1.0]"),
        ("§4", "lim in order", Header(|h| h["data"]["cols"][1]["lim"] = json!([40.0, 0.0])), "data.cols[\"MET\"].lim is [40.0,0.0], and its first end is its least value"),
        ("§4", "vals ascending, from lim to lim", Header(|h| h["data"]["cols"][0]["vals"] = json!([0, 1, 2])), "data.cols[\"nlep\"].vals must be every distinct value"),
        ("§4", "vals required on a few values", Header(|h| drop(h["data"]["cols"][0].as_object_mut().unwrap().shift_remove("vals"))), "data.cols[\"nlep\"] has no vals"),
        ("§4", "vals there whenever lim spans 64 values", Header(|h| {
            let nlep = &mut h["data"]["cols"][0];
            (nlep["lim"], nlep["nice"]) = (json!([0, 63]), json!([0, 63]));
            nlep.as_object_mut().unwrap().shift_remove("vals");
        }), "data.cols[\"nlep\"] has no vals, and a whole-number column whose lim [0,63] spans 64 values or fewer lists them"),
        ("§4", "vals at most 64 of them", Header(|h| {
            let nlep = &mut h["data"]["cols"][0];
            (nlep["lim"], nlep["nice"], nlep["vals"]) = (json!([0, 64]), json!([0, 64]), json!((0..=64).collect::<Vec<u64>>()));
        }), "data.cols[\"nlep\"].vals must be every distinct value, ascending, from lim's first end to its last, at most 64 of them"),
        ("§4 R5", "vals on no float column", Header(|h| h["data"]["cols"][1]["vals"] = json!([0])), "data.cols[\"MET\"] has vals, which a float column never has"),
        ("§4 F6", "whole facts are whole numbers", Header(|h| h["data"]["cols"][0]["lim"] = json!([0, 3.5])), "data.cols[\"nlep\"].lim must be two whole numbers"),
        ("§4 §8", "by_sample one per sample", Header(|h| drop(h["data"]["cols"][1]["by_sample"].as_array_mut().unwrap().pop())), "data.cols[\"MET\"].by_sample must hold one SHA-256 per sample"),
        // §5 Sample columns
        ("§5", "a record is exactly name, kind, values, sha256", Raw(|h| h["data"]["sample_cols"][0]["x"] = 1.into()), "data.sample_cols[\"xsec\"] has a field \"x\""),
        ("§5", "a kind of b1, i8, f8, text", Raw(|h| h["data"]["sample_cols"][0]["kind"] = "f4".into()), "data.sample_cols[\"xsec\"].kind is \"f4\""),
        ("§5", "one value per sample", Raw(|h| drop(h["data"]["sample_cols"][0]["values"].as_array_mut().unwrap().pop())), "data.sample_cols[\"xsec\"].values holds 1 values"),
        ("§5", "the keys that make sample columns", Raw(|h| drop(h["data"]["sample_cols"].as_array_mut().unwrap().remove(0))), "data.sample_cols has no column \"xsec\""),
        ("§5", "not a pressed column's name", Raw(|h| h["data"]["sample_cols"].as_array_mut().unwrap().push(json!({"name": "MET", "kind": "f8", "values": [1.0, 2.0], "sha256": ZEROS}))), "data.sample_cols[\"MET\"]: a sample column is never named"),
        ("§5", "the metadata's values", Made(|h| h["made"]["files"]["A"]["metadata"]["xsec"] = json!(88.3)), "data.sample_cols has no column \"xsec\" of kind f8 holding each sample's metadata value"),
        ("§5 §8", "its sha256 of the values as bytes", Raw(|h| h["data"]["sample_cols"][0]["sha256"] = ZEROS.into()), "data.sample_cols[\"xsec\"].sha256 is not the SHA-256 of its values"),
        ("§5 F7", "events_read and events_total on every sample", Header(|h| drop(h["made"]["files"]["A"]["metadata"].as_object_mut().unwrap().shift_remove("events_total"))), "made.files[\"A\"].metadata.events_total must be a whole number"),
        ("§5", "events_missing only with holes", Header(|h| { h["made"]["files"]["A"]["metadata"]["events_missing"] = 0.into(); h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into() }), "made.files[\"A\"].metadata has events_missing, and only a file with holes has it"),
        ("§5", "events_missing on every sample of a file with holes", Header(|h| { h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([hole(3, 7)]); h["made"]["files"]["A"]["metadata"]["events_missing"] = 4.into() }), "made.files[\"B\"].metadata.events_missing must be a whole number"),
        // §6 The study
        ("§6", "made has exactly its fields", Header(|h| h["made"]["x"] = 1.into()), "made has a field \"x\""),
        ("§6", "study: no / or control character", Header(|h| h["made"]["study"] = "a/b".into()), "made.study is \"a/b\""),
        ("§6", "built: UTC to the second", Header(|h| h["made"]["built"] = "2026-10-18 00:00:00".into()), "made.built is \"2026-10-18 00:00:00\""),
        ("§6", "by: nonempty", Header(|h| h["made"]["by"] = "".into()), "made.by must be nonempty text"),
        ("§6", "toolchain is exactly its fields", Header(|h| h["made"]["toolchain"]["x"] = 1.into()), "made.toolchain has a field \"x\""),
        ("§6", "toolchain.version: 15.MINOR.PATCH", Header(|h| h["made"]["toolchain"]["version"] = "0.1.0".into()), "made.toolchain.version is \"0.1.0\""),
        ("§6", "toolchain.program: nonempty", Header(|h| h["made"]["toolchain"]["program"] = "".into()), "made.toolchain.program must be nonempty text"),
        ("§6 §14", "toolchain's old name rvr refused", Header(|h| h["made"]["toolchain"]["rvr"] = "15.0.0".into()), "made.toolchain has a field \"rvr\""),
        ("§6 §14", "toolchain's old name rustc refused", Header(|h| h["made"]["toolchain"]["rustc"] = "rustc 1.91.0".into()), "made.toolchain has a field \"rustc\""),
        ("§6", "toolchain.rowver: \"15\"", Header(|h| h["made"]["toolchain"]["rowver"] = "14".into()), "made.toolchain.rowver is \"14\""),
        ("§6", "toolchain.commit: 40 hex or unknown", Header(|h| h["made"]["toolchain"]["commit"] = "fixture".into()), "made.toolchain.commit is \"fixture\""),
        ("§6", "toolchain.commit: -dirty- and 12 hex", Header(|h| h["made"]["toolchain"]["commit"] = "0123456789abcdef0123456789abcdef01234567-dirty-x".into()), "made.toolchain.commit is"),
        ("§6", "toolchain.commit: refused, not split, at a character of two bytes", Header(|h| h["made"]["toolchain"]["commit"] = "0123456789abcdef0123456789abcdef0123456é-dirty".into()), "made.toolchain.commit is \"0123456789abcdef0123456789abcdef0123456é-dirty\""),
        ("§6", "toolchain.compiler: nonempty", Header(|h| h["made"]["toolchain"]["compiler"] = "".into()), "made.toolchain.compiler must be nonempty text"),
        ("§6", "a text is exactly text and sha256", Header(|h| h["made"]["lock"]["x"] = 1.into()), "made.lock has a field \"x\""),
        ("§6 §8", "each text's sha256", Header(|h| h["made"]["lock"]["text"] = "# another lock\n".into()), "made.lock.sha256 is not the SHA-256 of made.lock.text"),
        ("§6", "recipe.format is rvr-recipe", Header(|h| h["made"]["recipe"]["format"] = "toml".into()), "made.recipe.format is \"toml\""),
        ("§6", "recipe.columns each once", Header(|h| h["made"]["recipe"]["columns"][1] = "nlep".into()), "made.recipe.columns must name each column the recipe stores once"),
        ("§6 §8", "made.sha256", Raw(|h| h["made"]["study"] = "another".into()), "made.sha256 is not the SHA-256 of made's canonical JSON"),
        ("§6", "made.files holds every sample", Header(|h| drop(h["made"]["files"].as_object_mut().unwrap().shift_remove("B"))), "made.files[\"B\"] is missing"),
        ("§6", "made.files holds no other", Header(|h| h["made"]["files"]["C"] = h["made"]["files"]["B"].clone()), "made.files[\"C\"] is no sample of data.samples"),
        ("§6", "made.files[s] is exactly events, files, metadata", Header(|h| h["made"]["files"]["A"]["x"] = 1.into()), "made.files[\"A\"] has a field \"x\""),
        ("§6", "metadata holds no null", Header(|h| h["made"]["files"]["A"]["metadata"]["note"] = Value::Null), "made.files[\"A\"].metadata.note is null"),
        ("§6", "a file record is exactly its fields", Header(|h| h["made"]["files"]["B"]["files"]["./fixture-b.root"]["served"] = json!({})), "made.files[\"B\"].files[\"./fixture-b.root\"] has a field \"served\""),
        ("§6", "a tree is nonempty", Header(|h| h["made"]["files"]["B"]["files"]["./fixture-b.root"]["tree"] = "".into()), "made.files[\"B\"].files[\"./fixture-b.root\"].tree must be nonempty text"),
        ("§6 R14", "a uuid is 8-4-4-4-12 lowercase hex", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["uuid"] = "fixture-a-uuid".into()), "made.files[\"A\"].files[\"./fixture-a.root\"].uuid is \"fixture-a-uuid\""),
        ("§6 R14", "a uuid is never null", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["uuid"] = Value::Null), "made.files[\"A\"].files[\"./fixture-a.root\"].uuid must be text"),
        ("§6 R16", "totals: {} refused", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["totals"] = json!({})), "made.files[\"A\"].files[\"./fixture-a.root\"].totals must be an object of one total or more"),
        ("§6", "a total is a finite double", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["totals"] = json!({"sumw": "x"})), "made.files[\"A\"].files[\"./fixture-a.root\"].totals.sumw is \"x\""),
        ("§6 R16", "a total is written as a double", Header(|h| { h["made"]["files"]["A"]["files"]["./fixture-a.root"]["totals"] = json!({"sumw": 1}); h["made"]["files"]["A"]["metadata"]["sumw"] = json!(1.0) }), "totals.sumw is 1, and a total is a finite double, written with a . or an exponent"),
        ("§6", "a sample's total is its files' sum", Header(|h| { h["made"]["files"]["A"]["files"]["./fixture-a.root"]["totals"] = json!({"sumw": 1.5}); h["made"]["files"]["A"]["metadata"]["sumw"] = json!(2.0) }), "made.files[\"A\"].metadata.sumw is 2.0, and its files' totals of sumw"),
        // §6.1 The event choice
        ("§6.1", "events is exactly its row's fields", Header(|h| h["made"]["files"]["A"]["events"]["seed"] = 1.into()), "made.files[\"A\"].events: events holds fields a \"first\" choice does not have"),
        ("§6.1", "the algorithm named", Header(|h| h["made"]["files"]["B"]["events"]["algorithm"] = "another".into()), "made.files[\"B\"].events: events.algorithm is not"),
        ("§6.1", "choose all, first or random", Header(|h| h["made"]["files"]["A"]["events"]["choose"] = "last".into()), "made.files[\"A\"].events: events.choose is not all, first or random"),
        ("§6.1 R10", "N from 1", Header(|h| h["made"]["files"]["A"]["events"]["n"] = 0.into()), "made.files[\"A\"].events: events.n must be a whole number from 1"),
        ("§1 §6.1", "N to 2^63 − 1: 2^63 refused", Header(|h| h["made"]["files"]["A"]["events"]["n"] = json!(9223372036854775808_u64)), "made.files[\"A\"].events: events.n must be a whole number from 1 to 2^63 − 1"),
        ("§6.1", "all: no stop", Header(|h| h["made"]["files"]["A"]["events"] = json!({"choose": "all"})), "made.files[\"A\"].events: the file ./fixture-a2.root's stop or take is not what"),
        ("§6.1", "first: stop when it read fewer", Header(|h| drop(h["made"]["files"]["A"]["files"]["./fixture-a2.root"].as_object_mut().unwrap().shift_remove("stop"))), "made.files[\"A\"].events: the file ./fixture-a2.root's stop is not what"),
        ("§6.1", "random: take on every file", Header(|h| drop(h["made"]["files"]["B"]["files"]["./fixture-b.root"].as_object_mut().unwrap().shift_remove("take"))), "made.files[\"B\"].events: the file ./fixture-b.root's take is not what"),
        ("§6.1", "random: takes add up to min(N, total)", Header(|h| h["made"]["files"]["B"]["files"]["./fixture-b.root"]["take"] = 3.into()), "made.files[\"B\"].events: the files' take adds up to 3, and a random choice of 4 takes 4"),
        ("§6.1 F8", "at most 2^40 entries a sample", Header(|h| h["made"]["files"]["B"]["files"]["./fixture-b.root"]["entries"] = json!(1_u64 << 41)), "made.files[\"B\"].events: the sample's files hold more than 2^40 entries in all"),
        ("§6.1 F8", "at most 2^40 entries a sample: 2^40 + 1 refused", Header(|h| h["made"]["files"]["B"]["files"]["./fixture-b.root"]["entries"] = json!((1_u64 << 40) + 1)), "made.files[\"B\"].events: the sample's files hold more than 2^40 entries in all"),
        // §6.2 Holes
        ("§6.2", "a hole is exactly entries, kind, why", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([{"entries": [3, 7], "kind": "damaged", "why": "x", "more": 1}])), "made.files[\"A\"].files[\"./fixture-empty.root\"].holes[0] has a field \"more\""),
        ("§6.2", "first < last", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([hole(5, 5)])), "holes[0].entries is [5, 5], and holes are ranges first < last"),
        ("§6.2", "a kind of the list", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([{"entries": [3, 7], "kind": "lost", "why": "x"}])), "holes[0].kind is \"lost\""),
        ("§6.2", "why: text that is not blank", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([{"entries": [3, 7], "kind": "gone", "why": "  "}])), "holes[0].why must say what failed"),
        ("§6.2", "in entry order and apart", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([hole(3, 7), hole(6, 8)])), "holes[1].entries is [6, 8], and holes are ranges"),
        ("§6.2", "within what the choice read (below stop)", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a2.root"]["holes"] = json!([hole(5, 8)])), "made.files[\"A\"].files[\"./fixture-a2.root\"].holes[0].entries is [5, 8]"),
        ("§6.2", "no empty list", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([])), "made.files[\"A\"].files[\"./fixture-empty.root\"].holes must be a list of one hole or more"),
        // §7 Arrays
        ("§7", "sample is exactly at, size, sha256", Header(|h| h["data"]["sample"]["by_sample"] = json!([ZEROS, ZEROS])), "data.sample has a field \"by_sample\""),
        ("§7", "entry has by_sample", Header(|h| drop(h["data"]["entry"].as_object_mut().unwrap().shift_remove("by_sample"))), "data.entry.by_sample is missing"),
        ("§7", "at from 0, end to end", Raw(|h| h["data"]["entry"]["at"] = (h["data"]["entry"]["at"].as_u64().unwrap() + 1).into()), "data.entry.at is"),
        ("§7", "a size from 1", Raw(|h| h["data"]["sample"]["size"] = 0.into()), "data.sample.size is 0"),
        // §8 Checksums
        ("§8", "data.sha256", Raw(|h| h["data"]["sha256"] = ZEROS.into()), "data.sha256 is not the SHA-256 of the arrays' sha256s"),
        ("§8", "a checksum is 64 lowercase hex", Arrays(|h, _| h["data"]["cols"][1]["sha256"] = h["data"]["cols"][1]["sha256"].as_str().unwrap().to_uppercase().into()), "data.cols[\"MET\"].sha256 must be 64 lowercase hexadecimal characters"),
        ("§8", "canonical JSON: no number past the largest double", Text(|t| *t = t.replacen("\"xsec\":87.3", "\"xsec\":87.3,\"big\":1e400", 1)), "is past the largest double"),
        // §9 The signature
        ("§9", "exactly sha256, ed25519, key", Signed(|h| h["signature"]["x"] = 1.into()), "signature must hold exactly ed25519, key and sha256"),
        ("§9", "sha256 of the header", Signed(|h| {
            h["made"]["by"] = "someone else".into();
            h["made"]["sha256"] = rowver::write::made_checksum(&h["made"]).unwrap().into();
        }), "the header changed after it was signed"),
        ("§9", "key: 64 lowercase hex", Signed(|h| h["signature"]["key"] = h["signature"]["key"].as_str().unwrap().to_uppercase().into()), "signature.key must be 64 lowercase hexadecimal characters"),
        ("§9", "the signature holds for key", Signed(|h| h["signature"]["key"] = rowver::sign::public_key(&[4; 32]).into()), "signature.ed25519 does not hold for signature.key"),
        ("§9 R11", "checked strictly: a key of small order refused", Signed(|h| {
            h["signature"]["key"] = format!("01{}", "0".repeat(62)).into();
            h["signature"]["ed25519"] = format!("01{}", "0".repeat(126)).into();
        }), "signature.key is an ed25519 public key of small order"),
        ("§9", "the key a canonical encoding, its y below p", Signed(|h| h["signature"]["key"] = format!("ee{}7f", "f".repeat(60)).into()), "signature.key is not the canonical encoding of an ed25519 public key (its y is p or more)"),
        ("§9", "R a canonical encoding, its y below p", Signed(|h| {
            let s = h["signature"]["ed25519"].as_str().unwrap()[64..].to_owned();
            h["signature"]["ed25519"] = format!("ee{}7f{s}", "f".repeat(60)).into();
        }), "signature.ed25519's R is not a canonical encoding (its y is p or more)"),
        ("§9", "R not of small order", Signed(|h| {
            let s = h["signature"]["ed25519"].as_str().unwrap()[64..].to_owned();
            h["signature"]["ed25519"] = format!("01{}{s}", "0".repeat(62)).into();
        }), "signature.ed25519 does not hold for signature.key"),
        ("§9", "S below L", Signed(|h| {
            let r = h["signature"]["ed25519"].as_str().unwrap()[..64].to_owned();
            h["signature"]["ed25519"] = format!("{r}edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010").into();
        }), "signature.ed25519's S is not below L"),
        // §10 Derived columns
        ("§10 R10", "derived present only with a derived column", Header(|h| h["derived"] = json!({"columns": []})), "derived.columns is empty"),
        ("§10", "derived is exactly columns", Derived(|h| h["derived"]["x"] = 1.into()), "derived has a field \"x\""),
        ("§10", "a name no other column has", Derived(|h| h["derived"]["columns"][0]["name"] = "xsec".into()), "derived.columns[\"xsec\"]: a derived column's name is one no stored, sample or other derived column has"),
        ("§10", "an expression that is not blank", Derived(|h| h["derived"]["columns"][0]["expression"] = " ".into()), "derived.columns[\"twice\"].expression is blank"),
        ("§10 F4", "a flat record without names", Derived(|h| h["derived"]["columns"][0]["names"] = json!({})), "derived.columns[\"twice\"] has a field \"names\""),
        ("§10", "a jagged record's content of a flat kind", Derived(|h| h["derived"]["columns"][1]["content"]["kind"] = "jagged".into()), "derived.columns[\"muons\"].content.kind is \"jagged\""),
        ("§10", "by_sample one per sample", Derived(|h| drop(h["derived"]["columns"][0]["by_sample"].as_array_mut().unwrap().pop())), "derived.columns[\"twice\"].by_sample must hold one SHA-256 per sample"),
        ("§10 §4", "facts checked on opening", Derived(|h| h["derived"]["columns"][0]["nice"] = json!([0, 1])), "derived.columns[\"twice\"].nice is [0,1], and the rule"),
        // §11 Tool keys
        ("§11", "a tool key's name", Raw(|h| h["a b"] = json!({})), "the top-level key \"a b\" is no field of the format and not a tool key's name"),
        ("§11", "a tool key's name is nonempty", Raw(|h| h[""] = json!({})), "the top-level key \"\" is no field of the format"),
        ("§11", "a slip of data in its case", Raw(|h| h["Data"] = json!({})), "the top-level key \"Data\" is a slip of data"),
        ("§11", "a slip of data, a character replaced", Raw(|h| h["dota"] = json!({})), "is a slip of data"),
        ("§11", "a slip of made, a character deleted", Raw(|h| h["mde"] = json!({})), "the top-level key \"mde\" is a slip of made"),
        ("§11", "a slip of made, a character inserted", Raw(|h| h["madde"] = json!({})), "is a slip of made"),
        ("§11", "a slip of signature, a character deleted", Raw(|h| h["signatur"] = json!({})), "the top-level key \"signatur\" is a slip of signature"),
        ("§11", "a slip of derived, two neighbours swapped", Raw(|h| h["dervied"] = json!({})), "is a slip of derived"),
        ("§11", "a slip of rowver, in its case and a character deleted", Raw(|h| h["ROWVR"] = json!({})), "is a slip of rowver"),
        // §12 What a reader checks: an empty file's header, checked as any file's
        ("§12 R18", "an empty file: rows add up", Empty(|h| h["data"]["rows"] = 6.into()), "data.rows is 6"),
        ("§12 R18", "an empty file: kinds", Empty(|h| h["data"]["cols"][1]["kind"] = "f16".into()), "data.cols[\"MET\"].kind is \"f16\""),
        ("§12 R18", "an empty file: facts", Empty(|h| h["data"]["cols"][0]["lim"] = json!([5, 1])), "data.cols[\"nlep\"].lim is [5,1], and its first end is its least value"),
        ("§12 R18", "an empty file: the arrays' records laid out", Empty(|h| h["data"]["sample"]["size"] = 0.into()), "data.sample.size is 0"),
    ]
}

// SPEC 2.3, 3.1.3, 3.5.10, 4.1.6, 4.4.1, 4.6.2; FORMAT 3, 4, 5, 6, 6.1, 6.2, 6.3, 7, 8, 10, 12
#[test]
fn full_validation_refuses_each_rule_it_alone_checks() {
    refused(in_full(), false);
}

/// The rules only full validation checks, each with a change that breaks it alone.
fn in_full() -> Vec<Row> {
    vec![
        // §3 Rows, against the arrays and the lock
        ("§3 §6.3", "samples are the lock's, with its labels", Header(|h| h["data"]["samples"][1]["label"] = "Bee".into()), "data.samples holds A (Sample A, π⁰), B (Bee), and it holds the lock's samples"),
        ("§3", "the sample index runs through the samples", Arrays(|h, a| put(h, a, 0, &[0_i32, 0, 1, 1, 1].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(), 4)), "data.sample: row 2 holds 1"),
        ("§3 R15", "entries strictly increasing within a file", Arrays(|h, a| put(h, a, 1, &[2_i64, 0, 4, 1, 17].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(), 8)), "data.entry: row 1 holds entry 0 of data.files[0] after entry 2"),
        ("§3 §6.1", "each entry chosen", Arrays(|h, a| put(h, a, 1, &[0_i64, 2, 6, 1, 17].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(), 8)), "data.entry: row 2 holds entry 6 of data.files[2] (./fixture-a2.root), which the event choice did not choose"),
        ("§3 §6.1", "a random choice made again", Header(|h| h["made"]["files"]["B"]["events"]["seed"] = 8.into()), "data.entry: row 3 holds entry 1 of data.files[3] (./fixture-b.root), which the event choice did not choose"),
        ("§6.3", "each file's random take its share, the choice the file made again", SwappedTakes(true), "made.lock.text: sample B's file ./fixture-b.root has take 1, and its random choice takes 3 of it"),
        ("§6.3", "each file's random take its share, the lock's own choice made again", SwappedTakes(false), "made.lock.text: sample B's file ./fixture-b.root has take"),
        ("§3 §7", "entry is i8[rows]", Arrays(|h, a| put(h, a, 1, &[0_i64, 2, 4, 1].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(), 8)), "data.entry: its zstd frame decodes to 1 bytes, and its chunk's lane holds 2"),
        // §4 Columns, against their values and the lock
        ("§4", "a copied column keeps its branch's kind", Lock(|l| met_as(l, "f8")), "data.cols[\"MET\"] is f4, and the recipe makes it f8"),
        ("§4", "offsets never decrease", Arrays(|h, a| put(h, a, 4, &[0_i64, 2, 1, 3, 6, 6].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(), 8)), "data.cols[\"Muon_pt\"].offsets: a chunk's offsets must start at 0 and never decrease"),
        ("§4 F9", "lim made again from the values", Header(|h| { h["data"]["cols"][1]["lim"] = json!([0, 50]); h["data"]["cols"][1]["nice"] = json!([0, 50]) }), "data.cols[\"MET\"].lim is [0,50], and its values give |[0.0,40.0]"),
        ("§4", "vals made again from the values", Header(|h| h["data"]["cols"][0]["vals"] = json!([0, 2, 3])), "data.cols[\"nlep\"].vals is [0,2,3], and its values give [0,1,2,3]"),
        ("§4", "a lim of 38 digits, wider than a span holds", Header(|h| {
            let ends: Value = serde_json::from_str("[-150000000000000000000000000000000000000, 150000000000000000000000000000000000000]").unwrap();
            h["data"]["cols"][0]["lim"] = ends.clone();
            h["data"]["cols"][0]["nice"] = ends;
            drop(h["data"]["cols"][0].as_object_mut().unwrap().shift_remove("vals"));
        }), "data.cols[\"nlep\"].lim is [-150000000000000000000000000000000000000,150000000000000000000000000000000000000], and its values give [0,3]"),
        ("§4", "a b1 value is 0 or 1", Arrays(|h, a| put(h, a, 13, &[0, 1, 2, 1, 1], 1)), "data.cols[\"flag\"]: a b1 value is 2"),
        // §5 Sample columns: the counts
        ("§5 F7", "events_read exact", Header(|h| h["made"]["files"]["A"]["metadata"]["events_read"] = 24.into()), "made.files[\"A\"].metadata.events_read is 24, and the sample's event choice chose 25 entries"),
        ("§5 F7", "events_total exact", Header(|h| h["made"]["files"]["A"]["metadata"]["events_total"] = 31.into()), "made.files[\"A\"].metadata.events_total is 31, and it is the entries of all the sample's files in the lock, 30"),
        ("§5 §6.2", "events_missing exact", Header(|h| {
            h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([hole(3, 7)]);
            h["made"]["files"]["A"]["metadata"]["events_read"] = 21.into();
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 3.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into();
        }), "made.files[\"A\"].metadata.events_missing is 3, and 4 entries"),
        ("§5 §3.5.10", "no samples.toml metadata key is one rvr gives", Samples(|t| *t = t.replace("seed_like = ", "events_read = 1, seed_like = ")), "made.samples.text is not a samples.toml docs/SPEC.md §3 accepts"),
        ("§5", "no samples.toml metadata key is a total's name", Recipe(|t| *t = t.replace("[totals]\n", "[totals]\nxsec = \"MET\"\n")), "the recipe's total xsec has the name of a key of sample A's metadata"),
        // §6 The study, its three texts against each other and the file
        ("§6", "title present exactly when the recipe sets one", Header(|h| drop(h["made"].as_object_mut().unwrap().shift_remove("title"))), "made.title is null, and it is the recipe's title"),
        ("§6", "the recipe is one the reference accepts", Recipe(|t| t.push_str("x = ")), "made.recipe.text cannot be read as a recipe"),
        ("§6", "the recipe has all five tables", Recipe(|t| *t = t.replace("[define]\n", "")), "A recipe has no [define] table"),
        ("§6", "its [columns] stores recipe.columns", Recipe(|t| *t = t.replace(", \"double\"", "")), "made.recipe.columns is"),
        ("§6 §10", "its [derived] lists the derived columns", Recipe(|t| t.push_str("twice = \"MET * 2\"\n")), "does not list the derived columns its header records"),
        ("§6", "samples.toml accepted", Samples(|t| *t = t.replace("version = 1", "version = 2")), "made.samples.text is not a samples.toml docs/SPEC.md §3 accepts"),
        ("§6 §6.3", "the lock resolved from that samples.toml", Samples(|t| *t = t.replace("xsec = 87.3", "xsec = 87.4")), "made.lock.text was not resolved from made.samples.text"),
        ("§6", "tree: the lock's", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["tree"] = "Other".into()), "made.files[\"A\"].files[\"./fixture-a.root\"].tree is \"Other\", and it is the lock's tree"),
        ("§6", "uuid: the lock's", Header(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["uuid"] = "31e851e4-b994-11f1-ba5c-0000000000ff".into()), "made.files[\"A\"].files[\"./fixture-a.root\"].uuid is \"31e851e4-b994-11f1-ba5c-0000000000ff\", and it is the UUID the lock gives"),
        ("§6", "entries: the lock's", Header(|h| h["made"]["files"]["B"]["files"]["./fixture-b.root"]["entries"] = 21.into()), "made.files[\"B\"].files[\"./fixture-b.root\"].entries is 21"),
        ("§6", "metadata: the lock's values", Header(|h| h["made"]["files"]["A"]["metadata"]["xsec"] = json!(87.4)), "made.files[\"A\"].metadata.xsec is 87.4, and it is the lock's value"),
        ("§6", "metadata: the lock's keys and rvr's", Header(|h| h["made"]["files"]["A"]["metadata"]["note"] = "x".into()), "made.files[\"A\"].metadata holds the keys"),
        ("§6", "a total no file holds is 0.0 on every sample", Rowless(|h| holed_with_total(h, json!(5.0), json!(0.0))), "made.files[\"A\"].metadata.sumw is 5.0, and no file of the sample holds that total"),
        ("§6 R16", "a total no file holds is written as a double", Rowless(|h| holed_with_total(h, json!(0.0), json!(0))), "made.files[\"B\"].metadata.sumw is 0, and a total is a finite double"),
        ("§6 R16", "totals on every file read", Recipe(|t| *t = t.replace("[totals]\n", "[totals]\nsumw = \"MET\"\n")), "made.files[\"A\"].files[\"./fixture-a.root\"].totals holds [], and a file the press read holds each total"),
        ("§6 R16", "totals only when the recipe has one, every sample holding them", Header(|h| { h["made"]["files"]["A"]["files"]["./fixture-a.root"]["totals"] = json!({"sumw": 1.0}); h["made"]["files"]["A"]["metadata"]["sumw"] = json!(1.0); h["made"]["files"]["B"]["metadata"]["sumw"] = json!(0.0) }), "made.files[\"A\"].files[\"./fixture-a.root\"].totals holds [\"sumw\"], and a file the press read holds each total"),
        ("§6 R16", "no totals on a file read none of, every sample holding them", Header(|h| {
            h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["holes"] = json!([hole(0, 10)]);
            h["made"]["files"]["A"]["files"]["./fixture-empty.root"]["totals"] = json!({"sumw": 0.0});
            h["made"]["files"]["A"]["metadata"]["sumw"] = json!(0.0);
            h["made"]["files"]["A"]["metadata"]["events_read"] = 15.into();
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 10.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into();
            h["made"]["files"]["B"]["metadata"]["sumw"] = json!(0.0);
        }), "made.files[\"A\"].files[\"./fixture-empty.root\"] has totals, and the press did not read the file"),
        // §6.2 Holes, against the choice and the rows
        ("§6.2", "a hole holds a chosen entry", Header(|h| {
            h["made"]["files"]["B"]["files"]["./fixture-b.root"]["holes"] = json!([hole(0, 1)]);
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 0.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into();
        }), "made.files[\"B\"].files[\"./fixture-b.root\"].holes[0] holds entry 0, none of which the event choice chose"),
        ("§6.2", "no row's entry in a hole", Header(|h| {
            h["made"]["files"]["A"]["files"]["./fixture-a.root"]["holes"] = json!([hole(0, 1)]);
            h["made"]["files"]["A"]["metadata"]["events_read"] = 24.into();
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 1.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into();
        }), "data.entry: row 0 holds entry 0 of data.files[0] (./fixture-a.root), which the event choice did not choose or a hole holds"),
        // §6.3 The carried lock
        ("§6.3 L1", "TOML", LockText(|t| t.push_str("[[[")), "made.lock.text is not a samples.lock docs/FORMAT.md §6.3 accepts: samples.lock is not TOML"),
        ("§6.3 L2", "format and version", LockText(|t| *t = t.replace("version = 1", "version = 2")), "samples.lock's version is not 1"),
        ("§6.3 L3", "resolved: YYYY-MM-DDTHH:MM:SSZ", Lock(|l| l.resolved = "yesterday".into()), "samples.lock's resolved \"yesterday\" is not YYYY-MM-DDTHH:MM:SSZ"),
        ("§6.3 L4", "one sample or more", Lock(|l| l.samples.clear()), "samples.lock has no sample"),
        ("§6.3 L5", "a sample exactly its fields", LockText(|t| *t = t.replacen("tree = \"Events\"\n", "tree = \"Events\"\nx = 1\n", 1)), "samples.lock sample 1 has no key \"x\""),
        ("§6.3 L6", "the samples samples.toml makes", Lock(|l| l.samples[1].label = "Bee".into()), "samples.lock sample \"B\" is not the sample samples.toml makes"),
        ("§6.3 L7", "events_total the sum of entries", Lock(|l| l.samples[0].events_total = 31), "samples.lock sample \"A\": events_total is not the sum of its files' entries"),
        ("§6.3 L8", "branches names one of the lists", Lock(|l| l.samples[0].branches = ZEROS.into()), "samples.lock sample \"A\": its branches names no branch list of the lock"),
        ("§6.3 L9", "a file exactly its fields", LockText(|t| *t = t.replacen("entries = 10\n", "entries = 10\nmodified = 1\n", 1)), "samples.lock sample 1 file 1 has no key \"modified\""),
        ("§6.3 L10", "stop exactly when first N reads fewer", Lock(|l| l.samples[0].files[2].stop = None), "samples.lock sample \"A\": ./fixture-a2.root's stop and take are not what its event choice reads"),
        ("§6.3 L11", "files in SPEC §3.8's order", Lock(|l| l.samples[0].files.swap(0, 1)), "samples.lock sample \"A\": ./fixture-a.root is not where the lock lists it"),
        ("§6.3 L12", "no two files with the same at", Lock(|l| l.samples[0].files[1].at = "./fixture-a.root".into()), "samples.lock sample \"A\" names ./fixture-a.root twice"),
        ("§6.3 L13", "at: a bare name as ./NAME", Lock(|l| l.samples[1].files[0].at = "fixture-b.root".into()), "samples.lock sample \"B\": fixture-b.root is not where the lock lists it"),
        ("§6.3 L14", "bytes from 1", Lock(|l| l.samples[0].files[0].bytes = 0), "./fixture-a.root has bytes 0"),
        ("§6.3 L15", "adler32 for a local file", Lock(|l| l.samples[0].files[0].adler32 = None), "./fixture-a.root has no adler32"),
        ("§6.3 L16", "uuid 8-4-4-4-12 lowercase hex", Lock(|l| l.samples[0].files[0].uuid = "X".into()), "./fixture-a.root's uuid is not a ROOT UUID"),
        ("§6.3 L17", "a branch's kind", Lock(|l| met_as(l, "f16")), "kind is a .rvr kind"),
        ("§6.3 L18", "no list twice", Lock(|l| l.branches.push(l.branches[0].clone())), "samples.lock writes one branch list twice"),
        ("§6.3 L19", "2^40 entries a sample", Lock(|l| {
            let mut more = l.samples[1].files[0].clone();
            (more.at, more.uuid, more.entries) = ("./fixture-b9.root".into(), "31e851e4-b994-11f1-ba5c-000000000009".into(), 1);
            l.samples[1].files[0].entries = 1 << 40;
            l.samples[1].files.push(more);
        }), "samples.lock sample \"B\"'s files hold more than 2^40 entries in all, more than a sample of a .rvr file may: split them among samples of fewer files"),
        ("§6.3 L19", "2^40 entries a sample: 2^40 + 1 in one file refused", Lock(|l| l.samples[1].files[0].entries = (1 << 40) + 1), "samples.lock sample \"B\": one of its files holds more than 2^40 entries, more than a sample of a .rvr file may: write that file again as smaller ones"),
        // §7 Arrays: chunks, lanes, frames and tables
        ("§3 §7", "chunk_rows the recipe's", Header(|h| h["data"]["chunk_rows"] = 1024.into()), "data.chunk_rows is 1024, and it is the recipe's chunk_rows, 65,536"),
        ("§7", "lane sizes adding up to the frames", Arrays(|_, a| { let n = a[3].len(); a[3][n - 128] += 1; }), "data.cols[\"MET\"]: its chunk table gives its frames"),
        ("§7", "byte lanes in order", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0.swap(2, 3))), "data.cols[\"MET\"]: a chunk's values are not the SHA-256 its chunk table states"),
        ("§7 §8", "a chunk's SHA-256 its values'", Arrays(|h, a| {
            edit(h, a, 3, 4, |c| c[0].1[0] ^= 1);
            let digests: Vec<Vec<u8>> = unlaid(&a[3], 2, 4).into_iter().map(|(_, d)| d).collect();
            h["data"]["cols"][1]["sha256"] = joined(&[&digests[0][..], &digests[1][..]]).into();
            h["data"]["cols"][1]["by_sample"] = json!([joined(&[&digests[0][..]]), joined(&[&digests[1][..]])]);
        }), "data.cols[\"MET\"]: a chunk's values are not the SHA-256 its chunk table states"),
        ("§7", "a lane decodes to the chunk's values", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0] = ruzstd::encoding::compress_to_vec(&[0_u8; 4][..], ruzstd::encoding::CompressionLevel::Fastest))), "data.cols[\"MET\"]: its zstd frame decodes to 4 bytes, and its chunk's lane holds 3"),
        ("§7 F5", "exactly one frame a lane", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0] = [c[0].0[0].clone(), c[0].0[0].clone()].concat())), "data.cols[\"MET\"]: its zstd frame ends"),
        ("§7 F5", "no skippable frame", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0] = [vec![0x50, 0x2a, 0x4d, 0x18, 0, 0, 0, 0], c[0].0[0].clone()].concat())), "data.cols[\"MET\"]: its zstd frame does not begin with the zstd magic"),
        ("§7 F5", "no dictionary", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0][4] |= 1)), "data.cols[\"MET\"]: its zstd frame has a dictionary ID"),
        ("§7 F5", "a content checksum", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0][4] &= !4)), "data.cols[\"MET\"]: its zstd frame has no content checksum"),
        ("§7 F5", "the content checksum right", Arrays(|h, a| edit(h, a, 3, 4, |c| *c[0].0[0].last_mut().unwrap() ^= 1)), "data.cols[\"MET\"]: its zstd frame has a content checksum that is not that of its content"),
        ("§7 F5", "a declared content size exact", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0] = sized(&c[0].0[0], 4))), "data.cols[\"MET\"]: its zstd frame declares 4 bytes of content"),
        ("§7", "a window of at most 2^27", Arrays(|h, a| edit(h, a, 3, 4, |c| c[0].0[0][5] = 18 << 3)), "data.cols[\"MET\"]: its zstd frame asks for a window of 268435456 bytes"),
        ("§7 §8", "a file of no rows: each array's sha256 that of no chunks", Rowless(|h| h["data"]["entry"]["sha256"] = sha(b"x").into()), "data.entry: its chunk table does not give the array's sha256"),
        ("§7 §8", "a file of no rows: each array's by_sample that of no chunks", Rowless(|h| h["data"]["cols"][1]["by_sample"][1] = sha(b"x").into()), "data.cols[\"MET\"]: its chunk table does not give by_sample[1]"),
        // §8 Checksums, against the chunk tables
        ("§8", "an array's sha256", Arrays(|h, _| h["data"]["cols"][1]["sha256"] = ZEROS.into()), "data.cols[\"MET\"]: its chunk table does not give the array's sha256"),
        ("§8", "by_sample", Header(|h| h["data"]["cols"][1]["by_sample"][0] = ZEROS.into()), "data.cols[\"MET\"]: its chunk table does not give by_sample[0]"),
        ("§8", "jagged content's by_sample", Header(|h| h["data"]["cols"][2]["content"]["by_sample"][1] = ZEROS.into()), "data.cols[\"Muon_pt\"].content: its chunk table does not give by_sample[1]"),
        // §10 Derived columns, worked out
        ("§10 F6", "facts made again, compared as numbers", Derived(|h| { h["derived"]["columns"][0]["lim"] = json!([0, 90]); h["derived"]["columns"][0]["nice"] = json!([0, 90]) }), "derived.columns[\"twice\"], worked out here, is not what the file records"),
        ("§10 §8", "checksums made again", Derived(|h| h["derived"]["columns"][1]["content"]["by_sample"][0] = ZEROS.into()), "derived.columns[\"muons\"], worked out here, is not what the file records"),
        ("§10", "an expression of the file's own columns", Derived(|h| {
            h["derived"]["columns"][0]["expression"] = "nothing * 2".into();
            let text = h["made"]["recipe"]["text"].as_str().unwrap().replace("twice = \"MET * 2\"", "twice = \"nothing * 2\"");
            set_text(h, "recipe", &text);
        }), "[derived] twice uses nothing, but the file does not store nothing"),
        // §12 Full validation of an empty file: every check but the arrays'
        ("§12 R18", "an empty file: events_read exact", Empty(|h| h["made"]["files"]["A"]["metadata"]["events_read"] = 24.into()), "made.files[\"A\"].metadata.events_read is 24"),
        ("§12 R18", "an empty file: the lock", Empty(|h| h["made"]["files"]["A"]["files"]["./fixture-a.root"]["tree"] = "Other".into()), "tree is \"Other\", and it is the lock's tree"),
        ("§12 R18", "an empty file: holes hold chosen entries", Empty(|h| {
            h["made"]["files"]["B"]["files"]["./fixture-b.root"]["holes"] = json!([hole(0, 1)]);
            h["made"]["files"]["A"]["metadata"]["events_missing"] = 0.into();
            h["made"]["files"]["B"]["metadata"]["events_missing"] = 0.into();
        }), "none of which the event choice chose"),
        ("§12 R18", "an empty file: derived columns worked out over no rows", EmptyDerived(|h| h["derived"]["columns"][0]["kind"] = "f8".into()), "derived.columns[\"twice\"] is f8, and its expression, worked out over no rows, gives f4"),
    ]
}

/// Whether a row's change is made to the file as it is: not one that rebuilds the fixture itself
/// (its derived columns, or sample B split in two).
fn of_any_file(row: &Row) -> bool {
    !matches!(row.2, Derived(_) | EmptyDerived(_) | SwappedTakes(_))
}

// FORMAT 12; SPEC 1.5
#[test]
fn every_rule_refuses_alone_in_generated_files() {
    // The seeds shared out over every core, each seed's file checked against every row.
    let (cases, next) = (common::gen::cases(4), std::sync::atomic::AtomicU64::new(0));
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism().map_or(1, |n| n.get()) {
            scope.spawn(|| loop {
                let seed = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if seed >= cases {
                    return;
                }
                let base = || common::variant(seed);
                let (opened, validated) = checked(&to_bytes(&base().0, &base().1));
                assert!(opened.is_ok() && validated.is_ok(), "seed {seed}: the variant is not valid: {opened:?} {validated:?}");
                refused_of(base, opening().into_iter().filter(of_any_file).collect(), true, &format!("seed {seed}: "));
                refused_of(base, in_full().into_iter().filter(of_any_file).collect(), false, &format!("seed {seed}: "));
            });
        }
    });
}

// FORMAT 3, 6, 6.1
#[test]
fn a_file_of_no_entries_is_listed_and_read_and_one_never_reached_is_not() {
    // ./fixture-empty.root with no entries: the first 25 read 10, 0 and 10 entries.
    let lock = common::lock_with(|l| {
        l.samples[0].files[1].entries = 0;
        l.samples[0] = l.samples[0].planned(l.samples[0].events);
    });
    let recipe = common::recipe().replace("[totals]\n", "[totals]\nsumw = \"MET\"\n");
    let (mut header, arrays) = rebuilt(10.);
    set_text(&mut header, "lock", &lock);
    set_text(&mut header, "recipe", &recipe);
    let files = &mut header["made"]["files"]["A"];
    files["files"]["./fixture-empty.root"]["entries"] = 0.into();
    drop(files["files"]["./fixture-a2.root"].as_object_mut().unwrap().shift_remove("stop"));
    files["metadata"]["events_read"] = 20.into();
    files["metadata"]["events_total"] = 20.into();
    for (sample, path) in [("A", "./fixture-a.root"), ("A", "./fixture-empty.root"), ("A", "./fixture-a2.root"), ("B", "./fixture-b.root")] {
        header["made"]["files"][sample]["files"][path]["totals"] = json!({"sumw": 1.0});
    }
    header["made"]["files"]["A"]["metadata"]["sumw"] = json!(3.0);
    header["made"]["files"]["B"]["metadata"]["sumw"] = json!(1.0);
    let read = sealed(header.clone());
    assert_eq!(checked(&to_bytes(&read, &arrays)), (Ok(()), Ok(())));
    // A file of no entries that the press opened holds the totals.
    let mut without = header.clone();
    drop(without["made"]["files"]["A"]["files"]["./fixture-empty.root"].as_object_mut().unwrap().shift_remove("totals"));
    without["made"]["files"]["A"]["metadata"]["sumw"] = json!(2.0);
    let said = checked(&to_bytes(&sealed(without), &arrays)).1.unwrap_err();
    assert!(said.contains("./fixture-empty.root\"].totals holds [], and a file the press read holds each total"), "{said}");
    // Left out of the file, it is refused: only a file with entries the first N never reached is.
    let mut left = header;
    left["data"]["files"].as_array_mut().unwrap().remove(1);
    drop(left["made"]["files"]["A"]["files"].as_object_mut().unwrap().shift_remove("./fixture-empty.root"));
    left["made"]["files"]["A"]["metadata"]["sumw"] = json!(2.0);
    let said = checked(&to_bytes(&sealed(left), &arrays)).1.unwrap_err();
    assert!(said.contains("data.files lists of sample A [\"./fixture-a.root\", \"./fixture-a2.root\"]"), "{said}");
}

// FORMAT 1, 3, 10, 12
#[test]
fn a_file_that_validates_whole_validates_emptied() {
    // B keeps none of its rows, and has no xsec, which a derived column reads.
    let (mut header, _) = rebuilt(10.);
    let piece = |name: &str, from, to, entry: Vec<i64>| common::Piece { name: name.into(), label: header["data"]["samples"][usize::from(name == "B")]["label"].as_str().unwrap().into(), columns: common::columns(from, to, 10.), entry };
    let pieces = vec![piece("A", 0, 3, vec![0, 2, 4]), piece("B", 3, 3, Vec::new())];
    let mut files = header["data"]["files"].as_array().unwrap().clone();
    files[3]["rows"] = 0.into();
    let (mut data, arrays) = common::pack(pieces, files).unwrap();
    data["sample_cols"] = header["data"]["sample_cols"].clone();
    header["data"] = data;
    set_text(&mut header, "recipe", &(common::recipe() + "w = \"MET * sample.xsec\"\n"));
    let header = sealed(header);
    let path = written(&to_bytes(&header, &arrays), "plain");
    let wanted = [("w".to_owned(), "MET * sample.xsec".to_owned())];
    let records = rowver::rvr::derived::records(&Reader::open(&path).unwrap(), &wanted, &[]).unwrap();
    let mut header = header;
    header["derived"] = json!({ "columns": records });
    assert_eq!(checked(&to_bytes(&header, &arrays)), (Ok(()), Ok(())));
    assert_eq!(checked(&to_bytes(&header, &[])), (Ok(()), Ok(())));
}

// SPEC 1.5; FORMAT 1
#[test]
fn reading_a_column_of_an_empty_file_says_it_is_empty() {
    let path = written(&to_bytes(&rebuilt(10.).0, &[]), "empty");
    let derived = written(&to_bytes(&sealed(derived().0), &[]), "empty-derived");
    let reader = Reader::open(&derived).unwrap();
    let asked = [reader.column("twice").map(drop), reader.slice("muons", 0, 2).map(drop), rowver::rvr::derived::checked(&reader).map(drop)];
    for refused in [Reader::open(&path).unwrap().column("MET").map(drop), Reader::open(&path).unwrap().entry().map(drop)].into_iter().chain(asked) {
        let said = refused.unwrap_err().to_string();
        assert!(said.contains("the file is empty") && said.contains("rvr refill"), "{said}");
    }
    // Asking for no columns gives none.
    assert_eq!(reader.stored_columns(&[]).unwrap(), vec![]);
    // Over no rows an empty file's derived columns still say what kind each is.
    assert_eq!(reader.slice("twice", 0, 0).unwrap().unwrap().kind, "f4");
}

// SPEC 11.8; FORMAT 10
#[test]
fn a_slice_of_a_derived_column_is_checked_against_its_record() {
    let (mut header, arrays) = derived();
    header["derived"]["columns"][0]["sha256"] = sha(b"x").into();
    let path = written(&to_bytes(&sealed(header), &arrays), "derived-lying");
    let reader = Reader::open(&path).unwrap();
    for refused in [reader.slice("twice", 1, 3).map(drop), reader.column("twice").map(drop)] {
        let said = refused.unwrap_err().to_string();
        assert!(said.contains("derived.columns[\"twice\"], worked out here, is not what the file records of it"), "{said}");
    }
    assert_eq!(reader.slice("muons", 1, 3).unwrap().unwrap().offsets.map(|o| o.len()), Some(3));
}

// SPEC 11.8; FORMAT 10
#[test]
fn a_derived_column_reads_only_the_columns_it_uses() {
    // Muon_pt's content damaged: muons, which reads it, is refused; twice, which reads MET alone, reads.
    let (mut header, mut arrays) = derived();
    edit(&header, &mut arrays, 5, 4, |c| c[0].0[0] = [c[0].0[0].clone(), c[0].0[0].clone()].concat());
    relaid(&mut header, &arrays);
    let path = written(&to_bytes(&sealed(header), &arrays), "derived-damaged");
    let reader = Reader::open(&path).unwrap();
    assert!(reader.column("muons").unwrap_err().to_string().contains("it is exactly one frame"));
    assert!(reader.column("twice").unwrap().is_some() && reader.slice("twice", 1, 3).unwrap().is_some());
    // Asked together, the refusal is Muon_pt's; twice alone reads whole.
    let both = rowver::rvr::derived::columns(&reader, &["twice".into(), "muons".into()]).unwrap_err().to_string();
    assert!(both.contains("it is exactly one frame"), "{both}");
    assert_eq!(rowver::rvr::derived::columns(&reader, &["twice".into()]).unwrap().len(), 1);
}

// FORMAT 7, 8, 12
#[test]
fn reading_a_column_checks_its_frame_length_and_sha256() {
    let (mut header, mut arrays) = rebuilt(10.);
    let raw: Vec<u8> = [20_f32, 0., 40., 30., 10.].iter().flat_map(|v| v.to_le_bytes()).collect();
    edit(&header, &mut arrays, 3, 4, |c| c[1].0[2] = [c[1].0[2].clone(), c[1].0[2].clone()].concat());
    relaid(&mut header, &arrays);
    let path = written(&to_bytes(&header, &arrays), "column");
    let reader = Reader::open(&path).unwrap();
    assert!(reader.column("MET").unwrap_err().to_string().contains("before its chunk's lane does: it is exactly one frame"));
    assert!(reader.column("nlep").is_ok());
    let (mut header, mut arrays) = rebuilt(10.);
    put(&mut header, &mut arrays, 3, &raw[..16], 4);
    relaid(&mut header, &arrays);
    let path = written(&to_bytes(&header, &arrays), "column");
    assert!(Reader::open(&path).unwrap().column("MET").unwrap_err().to_string().contains("decodes to 1 bytes, and its chunk's lane holds 2"));
    let (mut header, arrays) = rebuilt(10.);
    header["data"]["cols"][1]["sha256"] = ZEROS.into();
    relaid(&mut header, &arrays);
    let path = written(&to_bytes(&header, &arrays), "column");
    assert!(Reader::open(&path).unwrap().column("MET").unwrap_err().to_string().contains("its chunk table does not give the array's sha256"));
}

// FORMAT 6, 6.1, 6.3
#[test]
fn a_two_file_random_sample_whose_takes_are_the_choices_shares_is_valid() {
    for same in [true, false] {
        let (mut header, mut arrays) = rebuilt(10.);
        two_files(&mut header, &mut arrays, same, false);
        let path = written(&to_bytes(&sealed(header), &arrays), "two-files");
        validate_file(&path).unwrap_or_else(|e| panic!("same choice {same}: {e}"));
    }
}


/// A valid file's contents as the corpus states them: rows, each row's sample and entry, and every
/// column and derived column (floats as the hex of their bits, at their own width).
fn contents(bytes: &[u8]) -> Value {
    let path = written(bytes, "corpus");
    let reader = Reader::open(&path).unwrap();
    let mut columns = serde_json::Map::new();
    if !reader.empty() {
        let names: Vec<String> = reader.header()["made"]["recipe"]["columns"].as_array().unwrap().iter().chain(reader.header()["derived"]["columns"].as_array().into_iter().flatten().map(|d| &d["name"])).map(|n| n.as_str().unwrap().to_owned()).collect();
        for name in names {
            let held = reader.column(&name).unwrap().unwrap();
            let width = rowver::rvr::width(&held.kind).unwrap();
            let values: Vec<Value> = held.bytes.chunks(width).map(|b| {
                let mut wide = [0u8; 8];
                wide[..width].copy_from_slice(b);
                let raw = u64::from_le_bytes(wide);
                let shift = 64 - 8 * width as u32;
                match held.kind.as_str() {
                    "f8" | "f4" => json!(format!("{raw:0w$x}", w = 2 * width)),
                    "b1" => json!(raw != 0),
                    k if k.starts_with('i') => json!(((raw << shift) as i64) >> shift),
                    _ => json!(raw),
                }
            }).collect();
            let mut column = json!({"kind": held.kind, "values": values});
            if let Some(offsets) = held.offsets {
                column["offsets"] = json!(offsets);
            }
            columns.insert(name, column);
        }
    }
    let empty = reader.empty();
    let (sample, entry) = match empty { true => (json!(null), json!(null)), false => (json!(reader.sample().unwrap()), json!(reader.entry().unwrap())) };
    json!({"rows": reader.rows().unwrap(), "empty": empty, "sample": sample, "entry": entry, "columns": columns})
}

// FORMAT 12, example; SPEC 9.1
#[test]
fn the_conformance_corpus_holds_what_this_reader_says() {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut valid = Vec::new();
    let (header, arrays) = rebuilt(10.);
    let mut signed = header.clone();
    rowver::sign::sign(&mut signed, &[3; 32]).unwrap();
    let (with_derived, derived_arrays) = derived();
    let (mut none, none_arrays) = common::rowless();
    relaid(&mut none, &none_arrays);
    let (none_derived, _) = derived_of((sealed(none.clone()), none_arrays.clone()));
    let mut named: Vec<(String, Vec<u8>)> = vec![
        ("fixture".into(), to_bytes(&header, &arrays)),
        ("fixture-empty".into(), to_bytes(&header, &[])),
        ("fixture-signed".into(), to_bytes(&signed, &arrays)),
        ("fixture-derived".into(), to_bytes(&sealed(with_derived.clone()), &derived_arrays)),
        ("fixture-empty-derived".into(), to_bytes(&sealed(with_derived), &[])),
        ("fixture-rowless".into(), to_bytes(&sealed(none), &none_arrays)),
        ("fixture-rowless-derived".into(), to_bytes(&sealed(none_derived), &none_arrays)),
    ];
    for seed in 0..12 {
        let mut rng = common::gen::Rng::new(seed);
        let table = common::gen::table(&mut rng, 8);
        let values = common::gen::filled(&mut rng, &table);
        let (header, arrays) = common::gen::file(&table, values);
        named.push((format!("generated-{seed}"), to_bytes(&header, &arrays)));
    }
    for (name, bytes) in named {
        let path = format!("valid/{name}.rvr");
        valid.push(json!({"file": path, "verdict": "valid", "signed": name.ends_with("signed"), "contents": contents(&bytes)}));
        files.push((path, bytes));
    }
    // Every case, one file after another in cases.bin: the files the format allows, and each rule broken alone.
    let (mut cases, mut packed) = (Vec::new(), Vec::new());
    let mut case = |rule: String, verdict: &str, bytes: Vec<u8>, journal: bool| {
        let (opened, validated) = checked_with(&bytes, journal);
        let said = opened.clone().err().or(validated.clone().err());
        let when = match (&opened, &validated) {
            (Ok(()), Ok(())) => "",
            (Err(_), _) => "opening",
            _ => "full",
        };
        let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        let mut entry = json!({"rule": rule, "verdict": verdict, "refused": when, "at": packed.len(), "size": bytes.len(), "sha256": sha, "rust_says": said});
        if journal {
            entry["journal"] = true.into();
        }
        cases.push(entry);
        packed.extend(bytes);
    };
    for (rule, change) in allowed() {
        case(format!("FORMAT {rule}"), "valid", bytes(change), false);
    }
    for (rows, when) in [(opening(), "opening"), (in_full(), "full")] {
        for (section, rule, change, _) in rows {
            case(format!("FORMAT {section} {rule}"), when, bytes(change), matches!(change, Journaled));
        }
    }
    for entry in &cases {
        let verdict = entry["verdict"].as_str().unwrap();
        let refused = entry["refused"].as_str().unwrap();
        assert!((verdict == "valid" && refused.is_empty()) || verdict == refused, "{entry}");
    }
    files.push(("cases.zst".into(), ruzstd::encoding::compress_to_vec(&packed[..], ruzstd::encoding::CompressionLevel::Fastest)));
    let manifest = json!({
        "about": "rvr files: each valid one with its contents, and every case of cases.zst (one zstd frame of the cases end to end, each at its at and size) with its verdict; see README.md",
        "valid": valid,
        "cases": cases,
    });
    common::corpus::corpus("rvr", &files, &manifest);
}
