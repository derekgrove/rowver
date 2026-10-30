//! Reading the committed fixture, and what readers refuse: a damaged header, another version,
//! a broken `made`, an entry the event choice did not read, holes that do not hold, and a header
//! changed after signing.

mod common;

use common::{golden, rebuilt, split, written};
use rowver::rvr::{from_bytes, validate_file, Reader};
use rowver::write::{made_checksum, to_bytes};
use serde_json::{json, Value};

/// Little-endian values of a kind, each as an f64.
fn as_f64(kind: &str, bytes: &[u8]) -> Vec<f64> {
    let w = match kind {
        "f8" | "i8" | "u8" => 8,
        "f4" | "i4" | "u4" => 4,
        "i2" | "u2" => 2,
        _ => 1,
    };
    bytes
        .chunks_exact(w)
        .map(|b| match kind {
            "f8" => f64::from_le_bytes(b.try_into().unwrap()),
            "f4" => f32::from_le_bytes(b.try_into().unwrap()) as f64,
            "i8" => i64::from_le_bytes(b.try_into().unwrap()) as f64,
            "i4" => i32::from_le_bytes(b.try_into().unwrap()) as f64,
            "i2" => i16::from_le_bytes(b.try_into().unwrap()) as f64,
            "i1" => b[0] as i8 as f64,
            "u8" => u64::from_le_bytes(b.try_into().unwrap()) as f64,
            "u4" => u32::from_le_bytes(b.try_into().unwrap()) as f64,
            "u2" => u16::from_le_bytes(b.try_into().unwrap()) as f64,
            _ => b[0] as f64,
        })
        .collect()
}

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
    let bytes = golden();
    let table = from_bytes(&bytes).unwrap();
    let path = written(&bytes, "fixture");
    let reader = Reader::open(&path).unwrap();
    for name in names(table.header()) {
        let held = reader.column(&name).unwrap().unwrap();
        let column = table.column(&name).unwrap();
        let values = as_f64(&held.kind, &held.bytes);
        match &held.offsets {
            None => {
                let want: Vec<f64> = (0..table.len())
                    .map(|row| column.value(row).unwrap())
                    .collect();
                assert_eq!(values, want, "{name}");
            }
            Some(offsets) => {
                for row in 0..table.len() {
                    assert_eq!(
                        offsets[row + 1] - offsets[row],
                        column.object_count(row).unwrap(),
                        "{name}"
                    );
                }
                let want: Vec<f64> = (0..values.len())
                    .map(|i| column.content_value(i).unwrap())
                    .collect();
                assert_eq!(
                    values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "{name}"
                );
            }
        }
    }
    assert_eq!(reader.entry().unwrap().len(), table.len());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_header_declaring_more_rows_than_its_arrays_hold_is_refused_not_allocated() {
    let (mut header, arrays) = split(&golden());
    header["data"]["rows"] = (1_u64 << 40).into();
    let bytes = rowver::write::to_bytes(&header, &[arrays]).unwrap();
    let path = written(&bytes, "rows");
    // Opening may refuse the header; reading a column must refuse it rather than ask for 8 TB.
    if let Ok(reader) = Reader::open(&path) {
        assert!(reader.sample().is_err());
        for name in names(&header) {
            assert!(reader.column(&name).is_err(), "{name}");
        }
    }
    std::fs::remove_file(path).unwrap();
}

/// Whether a header, written with the fixture's arrays, opens and validates; the first error.
fn opens(header: &Value) -> Result<(), String> {
    let (_, arrays) = rebuilt(10.);
    let path = written(&to_bytes(header, &arrays).unwrap(), "changed");
    let opened = Reader::open(&path)
        .map(drop)
        .and_then(|()| validate_file(&path).map(drop));
    std::fs::remove_file(path).unwrap();
    opened.map_err(|e| e.to_string())
}

/// Seal a changed `made` with its new checksum, so only the change itself is wrong.
fn sealed(mut header: Value) -> Value {
    header["made"]["sha256"] = made_checksum(&header["made"]).unwrap().into();
    header
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
    let path = written(&to_bytes(&gone, &[arrays.concat()]).unwrap(), "inspect");
    let (reader, checked) = Reader::inspect(&path).unwrap();
    assert!(checked.is_err() && reader.column("MET").unwrap().is_some());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn the_event_choice_is_made_again_and_checked() {
    let (header, _) = rebuilt(10.);
    let mut seed = header.clone();
    seed["made"]["files"]["B"]["events"]["seed"] = 8.into();
    let why = opens(&sealed(seed)).unwrap_err();
    assert!(why.contains("did not read"), "{why}");
    let mut take = header.clone();
    take["made"]["files"]["B"]["files"]["fixture-b.root"]["take"] = 3.into();
    assert!(opens(&sealed(take)).unwrap_err().contains("take"));
    let mut stop = header.clone();
    stop["made"]["files"]["A"]["files"]["fixture-a2.root"]["stop"] = 6.into();
    assert!(opens(&sealed(stop)).unwrap_err().contains("stop"));
    let mut fewer = header.clone();
    fewer["made"]["files"]["A"]["events"]["n"] = 24.into();
    assert!(opens(&sealed(fewer)).is_err());
}

#[test]
fn a_file_may_say_which_storage_served_it_and_nothing_else() {
    let (header, _) = rebuilt(10.);
    fn record(header: &mut Value) -> &mut Value {
        &mut header["made"]["files"]["B"]["files"]["fixture-b.root"]
    }
    let mut served = header.clone();
    record(&mut served)["served"] = json!({"rse": "T2_US_Nebraska", "host": "xrootd.unl.edu:1094"});
    assert_eq!(opens(&sealed(served)), Ok(()));
    let mut unnamed = header.clone();
    record(&mut unnamed)["served"] = json!({"rse": "", "host": "xrootd.unl.edu:1094"});
    assert!(opens(&sealed(unnamed)).unwrap_err().contains("served"));
    let mut more = header.clone();
    record(&mut more)["served"] = json!({"rse": "T2_US_Nebraska", "host": "h:1", "when": 1});
    assert!(opens(&sealed(more)).unwrap_err().contains("when"));
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
/// them (or left out), sealed, and the sample columns made again.
fn holed(holes: &[(&str, &str, Value)], counts: Option<[(u64, u64); 2]>) -> Value {
    let (mut header, _) = rebuilt(10.);
    for (sample, file, list) in holes {
        header["made"]["files"][*sample]["files"][*file]["holes"] = list.clone();
    }
    for (sample, (read, missing)) in ["A", "B"].iter().zip(counts.into_iter().flatten()) {
        let metadata = &mut header["made"]["files"][*sample]["metadata"];
        metadata["events_read"] = read.into();
        metadata["events_missing"] = missing.into();
    }
    let names: Vec<String> = header["made"]["recipe"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    let taken: Vec<&str> = names.iter().map(String::as_str).collect();
    header["data"]["sample_cols"] =
        rowver::write::sample_cols(&["A", "B"], &header["made"]["files"], &taken)
            .unwrap()
            .into();
    sealed(header)
}

#[test]
fn holes_are_ranges_no_row_lies_in_and_the_counts_hold() {
    let hole = |first: u64, last: u64, kind: &str| json!({"entries": [first, last], "kind": kind, "why": "basket 2 does not decompress"});
    // A has 10 + 10 + 5 entries chosen; B 4 of 20 at random.
    let empty = ("A", "fixture-empty.root", json!([hole(3, 7, "damaged")]));
    assert_eq!(opens(&holed(std::slice::from_ref(&empty), Some([(21, 4), (4, 0)]))), Ok(()));
    let header = holed(std::slice::from_ref(&empty), Some([(21, 4), (4, 0)]));
    let found = rowver::rvr::holes(&header);
    assert_eq!((found.len(), found[0].first, found[0].last, found[0].kind.as_str()), (1, 3, 7, "damaged"));
    // Holes without the counts are a file a tool other than rvr press wrote.
    assert_eq!(opens(&holed(std::slice::from_ref(&empty), None)), Ok(()));

    let refused = |holes: &[(&str, &str, Value)], counts, said: &str| {
        let why = opens(&holed(holes, counts)).unwrap_err();
        assert!(why.contains(said), "{why}");
    };
    let counts = Some([(21, 4), (4, 0)]);
    refused(&[("A", "fixture-empty.root", json!([hole(1, 5, "damaged"), hole(4, 6, "tape")]))], counts, "in order and apart");
    refused(&[("A", "fixture-empty.root", json!([hole(3, 11, "damaged")]))], counts, "in order and apart");
    refused(&[("A", "fixture-empty.root", json!([hole(3, 7, "lost")]))], counts, "not one of tape");
    refused(&[("A", "fixture-empty.root", json!([]))], counts, "one hole or more");
    refused(&[("A", "fixture-a2.root", json!([hole(4, 5, "damaged")]))], Some([(24, 1), (4, 0)]), "or one in a hole");
    refused(std::slice::from_ref(&empty), Some([(21, 3), (4, 0)]), "events_missing");
    // events_read is the press's own, which rvr validate checks; the reader leaves it.
    assert_eq!(opens(&holed(std::slice::from_ref(&empty), Some([(22, 4), (4, 0)]))), Ok(()));
    let mut uncounted = holed(std::slice::from_ref(&empty), Some([(21, 4), (4, 0)]));
    uncounted["made"]["files"]["B"]["metadata"].as_object_mut().unwrap().shift_remove("events_missing");
    assert!(opens(&uncounted).is_err(), "a pressed file with holes counts what each sample misses");

    // A random choice's hole holds some of the chosen entries.
    let chosen = rowver::choice::per_file(&[20], 4, 7).remove(0);
    let unchosen = (0..20).find(|e| !chosen.contains(e)).unwrap();
    refused(&[("B", "fixture-b.root", json!([hole(unchosen, unchosen + 1, "unreachable")]))], None, "holds no entry the press chose");

    // Only a file never opened (one hole of every entry chosen, or one chosen nothing of) may
    // lack its UUID.
    let mut never = holed(&[("A", "fixture-empty.root", json!([hole(0, 10, "tape")]))], Some([(15, 10), (4, 0)]));
    never["made"]["files"]["A"]["files"]["fixture-empty.root"]["uuid"] = Value::Null;
    assert_eq!(opens(&sealed(never)), Ok(()));
    let mut opened = holed(std::slice::from_ref(&empty), counts);
    opened["made"]["files"]["A"]["files"]["fixture-empty.root"]["uuid"] = Value::Null;
    assert!(opens(&sealed(opened)).unwrap_err().contains("no uuid"));
    let (mut untaken, _) = rebuilt(10.);
    untaken["made"]["files"]["B"]["files"]["fixture-b.root"]["take"] = 0.into();
    untaken["made"]["files"]["B"]["files"]["fixture-b.root"]["uuid"] = Value::Null;
    assert!(!opens(&sealed(untaken)).unwrap_err().contains("no uuid"), "take 0 may lack it");
}
