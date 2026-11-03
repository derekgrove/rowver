//! Reading the committed fixture, and what readers refuse: a damaged header, another version,
//! a broken `made`, an entry the event choice did not read, holes that do not hold, and a header
//! changed after signing.

mod common;

use common::{golden, rebuilt, sealed, split, to_bytes, written};
use rowver::rvr::{validate_file, Reader};
use rowver::write::Values;
use serde_json::{json, Value};

fn names(header: &Value) -> Vec<String> {
    header["data"]["cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_format_version_is_the_crate_major_version() {
    assert_eq!(rowver::VERSION.to_string(), env!("CARGO_PKG_VERSION_MAJOR"));
}

#[test]
fn reader_gives_the_columns_the_fixture_holds() {
    let path = written(&golden(), "fixture");
    let reader = Reader::open(&path).unwrap();
    for (name, values) in common::columns(0, 5, 10.) {
        let held = reader.column(&name).unwrap().unwrap();
        match values {
            Values::Flat(flat) => assert_eq!((&held.kind, &held.bytes, &held.offsets), (&flat.kind, &flat.bytes, &None), "{name}"),
            Values::Jagged { counts, content } => {
                let offsets: Vec<usize> = std::iter::once(0).chain(counts.iter().scan(0, |at, n| { *at += n; Some(*at) })).collect();
                assert_eq!((&held.kind, &held.bytes, held.offsets.as_ref()), (&content.kind, &content.bytes, Some(&offsets)), "{name}");
            }
        }
    }
    assert_eq!(reader.entry().unwrap(), [0, 2, 4, 1, 17]);
    assert_eq!(reader.sample().unwrap(), [0, 0, 0, 1, 1]);
}

#[test]
fn a_header_declaring_more_rows_than_its_arrays_hold_is_refused_not_allocated() {
    let (mut header, arrays) = split(&golden());
    header["data"]["rows"] = (1_u64 << 40).into();
    let bytes = to_bytes(&header, &[arrays]);
    let path = written(&bytes, "rows");
    // Opening may refuse the header; reading a column must refuse it rather than ask for 8 TB.
    if let Ok(reader) = Reader::open(&path) {
        assert!(reader.sample().is_err());
        for name in names(&header) {
            assert!(reader.column(&name).is_err(), "{name}");
        }
    }
}

/// Whether a header, written with the fixture's arrays, opens and validates; the first error.
fn opens(header: &Value) -> Result<(), String> {
    let (_, arrays) = rebuilt(10.);
    let path = written(&to_bytes(header, &arrays), "changed");
    let opened = Reader::open(&path)
        .map(drop)
        .and_then(|()| validate_file(&path).map(drop));
    opened.map_err(|e| e.to_string())
}

#[test]
fn another_version_is_refused_with_what_to_do() {
    let (mut header, _) = rebuilt(10.);
    header["rowver"] = 13.into();
    let why = opens(&header).unwrap_err();
    assert!(
        why.contains("rowver 13") && why.contains("rvr press"),
        "{why}"
    );
}

#[test]
fn a_made_without_its_study_is_refused_but_can_be_inspected() {
    let (header, arrays) = rebuilt(10.);
    assert_eq!(opens(&header), Ok(()));
    let mut changed = header.clone();
    changed["made"]["lock"]["text"] = "# another lock\n".into();
    assert!(opens(&sealed(changed))
        .unwrap_err()
        .contains("made.lock.sha256"));
    let mut gone = header.clone();
    gone["made"]
        .as_object_mut()
        .unwrap()
        .shift_remove("samples");
    let gone = sealed(gone);
    assert!(opens(&gone).unwrap_err().contains("samples"));
    let path = written(&to_bytes(&gone, &[arrays.concat()]), "inspect");
    let (reader, checked) = Reader::inspect(&path).unwrap();
    assert!(checked.is_err() && reader.column("MET").unwrap().is_some());
}

#[test]
fn the_event_choice_is_made_again_and_checked() {
    let (header, _) = rebuilt(10.);
    let mut seed = header.clone();
    seed["made"]["files"]["B"]["events"]["seed"] = 8.into();
    let why = opens(&sealed(seed)).unwrap_err();
    assert!(why.contains("did not choose"), "{why}");
    let mut take = header.clone();
    take["made"]["files"]["B"]["files"]["./fixture-b.root"]["take"] = 3.into();
    assert!(opens(&sealed(take)).unwrap_err().contains("take"));
    let mut stop = header.clone();
    stop["made"]["files"]["A"]["files"]["./fixture-a2.root"]["stop"] = 6.into();
    assert!(opens(&sealed(stop)).unwrap_err().contains("stop"));
    let mut fewer = header.clone();
    fewer["made"]["files"]["A"]["events"]["n"] = 24.into();
    assert!(opens(&sealed(fewer)).is_err());
}

#[test]
fn a_file_record_holds_nothing_but_its_fields() {
    let (header, _) = rebuilt(10.);
    let mut served = header.clone();
    served["made"]["files"]["B"]["files"]["./fixture-b.root"]["served"] =
        json!({"rse": "T2_US_Nebraska", "host": "xrootd.unl.edu:1094"});
    assert!(opens(&sealed(served)).unwrap_err().contains("served"));
}

#[test]
fn a_signed_header_that_changed_is_refused() {
    let (mut header, _) = rebuilt(10.);
    rowver::sign::sign(&mut header, &[3; 32]).unwrap();
    assert_eq!(opens(&header), Ok(()));
    let mut changed = header.clone();
    changed["made"]["study"] = "another".into();
    let why = opens(&sealed(changed)).unwrap_err();
    assert!(why.contains("after it was signed"), "{why}");
    header["fixture_tool"]["more"] = 1.into();
    assert_eq!(opens(&header), Ok(()));
}

/// Holes put in the fixture's records, with `events_read` and `events_missing` as a press writes
/// them, sealed, and the sample columns made again.
fn holed(holes: &[(&str, &str, Value)], counts: [(u64, u64); 2]) -> Value {
    let (mut header, _) = rebuilt(10.);
    for (sample, file, list) in holes {
        header["made"]["files"][*sample]["files"][*file]["holes"] = list.clone();
    }
    for (sample, (read, missing)) in ["A", "B"].iter().zip(counts) {
        let metadata = &mut header["made"]["files"][*sample]["metadata"];
        metadata["events_read"] = read.into();
        metadata["events_missing"] = missing.into();
    }
    sealed(header)
}

#[test]
fn holes_are_ranges_no_row_lies_in_and_the_counts_hold() {
    let hole = |first: u64, last: u64, kind: &str| json!({"entries": [first, last], "kind": kind, "why": "basket 2 does not decompress"});
    // A has 10 + 10 + 5 entries chosen; B 4 of 20 at random.
    let empty = ("A", "./fixture-empty.root", json!([hole(3, 7, "damaged")]));
    let counts = [(21, 4), (4, 0)];
    assert_eq!(opens(&holed(std::slice::from_ref(&empty), counts)), Ok(()));
    let header = holed(std::slice::from_ref(&empty), counts);
    let found = rowver::rvr::holes(&header);
    assert_eq!((found.len(), found[0].first, found[0].last, found[0].kind.as_str()), (1, 3, 7, "damaged"));

    let refused = |holes: &[(&str, &str, Value)], counts, said: &str| {
        let why = opens(&holed(holes, counts)).unwrap_err();
        assert!(why.contains(said), "{why}");
    };
    refused(&[("A", "./fixture-empty.root", json!([hole(1, 5, "damaged"), hole(4, 6, "gone")]))], counts, "in entry order and apart");
    refused(&[("A", "./fixture-empty.root", json!([hole(3, 11, "damaged")]))], counts, "in entry order and apart");
    refused(&[("A", "./fixture-empty.root", json!([hole(3, 7, "lost")]))], counts, "not one of unreachable");
    refused(&[("A", "./fixture-empty.root", json!([]))], counts, "one hole or more");
    refused(&[("A", "./fixture-a2.root", json!([hole(4, 5, "damaged")]))], [(24, 1), (4, 0)], "or a hole holds");
    refused(std::slice::from_ref(&empty), [(21, 3), (4, 0)], "events_missing is 3");
    refused(std::slice::from_ref(&empty), [(22, 4), (4, 0)], "events_read is 22");
    // Every sample of a file with holes counts what it misses.
    let mut uncounted = holed(std::slice::from_ref(&empty), counts);
    uncounted["made"]["files"]["B"]["metadata"].as_object_mut().unwrap().shift_remove("events_missing");
    assert!(opens(&sealed(uncounted)).unwrap_err().contains("events_missing"));

    // A random choice's hole holds some of the chosen entries.
    let chosen = rowver::choice::per_file(&[20], 4, 7).remove(0);
    let unchosen = (0..20).find(|e| !chosen.contains(e)).unwrap();
    refused(&[("B", "./fixture-b.root", json!([hole(unchosen, unchosen + 1, "unreachable")]))], [(25, 0), (4, 0)], "none of which the event choice chose");

    // A file never opened is one hole of every entry chosen, with the lock's UUID: never null.
    assert_eq!(opens(&holed(&[("A", "./fixture-empty.root", json!([hole(0, 10, "unreachable")]))], [(15, 10), (4, 0)])), Ok(()));
    let mut opened = holed(std::slice::from_ref(&empty), counts);
    opened["made"]["files"]["A"]["files"]["./fixture-empty.root"]["uuid"] = Value::Null;
    assert!(opens(&sealed(opened)).unwrap_err().contains("uuid must be text"));
}

/// A sample's totals are its files' totals, finite doubles, summed in their order into its
/// metadata, and every file the press read holds each total the recipe asks for; anything else is
/// refused.
#[test]
fn a_samples_totals_are_its_files_totals_summed() {
    let totaled = |total: Value, said: Value, asked: &str| {
        let (mut header, _) = rebuilt(10.);
        let text = common::recipe().replace("[totals]\n", &format!("[totals]\n{asked}\n"));
        header["made"]["recipe"]["text"] = text.as_str().into();
        header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(&text).into();
        for sample in ["A", "B"] {
            let files: Vec<String> = header["made"]["files"][sample]["files"].as_object().unwrap().keys().cloned().collect();
            for file in &files {
                header["made"]["files"][sample]["files"][file]["totals"] = json!({ "sumw": 1.0 });
            }
            header["made"]["files"][sample]["metadata"]["sumw"] = (files.len() as f64).into();
        }
        header["made"]["files"]["B"]["files"]["./fixture-b.root"]["totals"] = json!({ "sumw": total });
        header["made"]["files"]["B"]["metadata"]["sumw"] = said;
        opens(&sealed(header))
    };
    let asked = "sumw = \"MET\"";
    assert_eq!(totaled(2.5.into(), 2.5.into(), asked), Ok(()));
    let wrong = totaled(2.5.into(), 3.0.into(), asked).unwrap_err();
    assert!(wrong.contains("made.files[\"B\"].metadata.sumw is 3.0, and its files' totals of sumw"), "{wrong}");
    let text = totaled("2.5".into(), 2.5.into(), asked).unwrap_err();
    assert!(text.contains("totals.sumw is \"2.5\", and a total is a finite double"), "{text}");
    let whole = totaled(2.into(), 2.0.into(), asked).unwrap_err();
    assert!(whole.contains("totals.sumw is 2, and a total is a finite double, written with a . or an exponent"), "{whole}");
    // A total the recipe does not ask for, or one it asks for that a file read does not hold.
    let more = totaled(2.5.into(), 2.5.into(), "").unwrap_err();
    assert!(more.contains(".totals holds [\"sumw\"], and a file the press read holds each total"), "{more}");
    let less = totaled(2.5.into(), 2.5.into(), "sumw = \"MET\"\nn = \"1\"").unwrap_err();
    assert!(less.contains(".totals holds [\"sumw\"]"), "{less}");
}

#[test]
fn a_failed_read_is_said_in_plain_words() {
    let path = written(&golden(), "plain");
    let reader = Reader::open(&path).unwrap();
    // Cut short after it was opened: its last array, `double`'s, ends past the file's end.
    std::fs::write(&path, &golden()[..golden().len() - 10]).unwrap();
    let said = rowver::plain(&path, &reader.column("double").unwrap_err());
    assert_eq!(said, format!("{} changed while it was read (emptied, or written again): open it again", path.0.display()));
    let gone = path.0.with_extension("gone");
    assert_eq!(rowver::plain(&gone, &Reader::open(&gone).unwrap_err()), format!("{} is not there", gone.display()));
    let folder = std::env::temp_dir();
    let said = rowver::plain(&folder, &validate_file(&folder).unwrap_err());
    assert!(said.ends_with("is a folder, not a file: name a file in it"), "{said}");
}
