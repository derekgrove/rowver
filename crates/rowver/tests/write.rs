//! The writer against the committed fixture, and what it refuses.

mod common;

use common::{golden, pack, rebuilt, to_bytes, Piece};
use rowver::write::{array, Sample, Values};
use serde_json::Value;

/// Drop where each compressed array lies, which depends on the compressor's version.
fn without_spans(value: &mut Value) {
    match value {
        Value::Object(members) => {
            members.shift_remove("at");
            members.shift_remove("size");
            members.values_mut().for_each(without_spans);
        }
        Value::Array(members) => members.iter_mut().for_each(without_spans),
        _ => {}
    }
}

// FORMAT 8, 12
#[test]
fn the_writer_makes_the_committed_fixture() {
    let (header, arrays) = rebuilt(10.);
    let (ours, theirs) = (common::written(&to_bytes(&header, &arrays), "ours"), common::written(&golden(), "theirs"));
    let (ours, theirs) = (rowver::rvr::validate_file(&ours).unwrap().header, rowver::rvr::validate_file(&theirs).unwrap().header);
    for section in ["data", "made"] {
        assert_eq!(ours[section]["sha256"], theirs[section]["sha256"]);
    }
    let (mut ours, mut theirs) = (ours, theirs);
    without_spans(&mut ours);
    without_spans(&mut theirs);
    assert_eq!(
        serde_json::to_string(&ours).unwrap(),
        serde_json::to_string(&theirs).unwrap()
    );
}

/// One sample, A, of two rows holding these columns.
fn one(columns: Vec<(String, Values)>) -> rowver::Result<(Value, Vec<Vec<u8>>)> {
    pack(vec![Piece { name: "A".into(), label: "A".into(), columns, entry: vec![0, 1] }], Vec::new())
}

// FORMAT 3, 4
#[test]
fn the_writer_refuses_what_the_format_refuses() {
    let flat = |values| vec![("x".to_owned(), Values::Flat(values))];
    // A float may be any IEEE value, NaN and infinities included.
    let nan = one(flat(array(&[1.0_f32, f32::NAN]))).unwrap().0;
    assert_eq!(nan["cols"][0]["lim"], serde_json::json!([1.0, 1.0]));
    let jagged = Values::Jagged {
        counts: vec![1, 1],
        content: array(&[1.0_f64, f64::INFINITY]),
    };
    assert!(one(vec![("x".into(), jagged)]).is_ok());
    let short = Values::Jagged {
        counts: vec![1, 2],
        content: array(&[1.0_f64, 2.0]),
    };
    assert!(one(vec![("x".into(), short)]).is_err());
    let piece = |name: &str, values: &[i8], rows: i64| Piece {
        name: name.into(),
        label: name.into(),
        columns: flat(array(values)),
        entry: (0..rows).collect(),
    };
    let even = vec![piece("A", &[1, 2], 2), piece("B", &[3, 4], 2)];
    assert!(pack(even, Vec::new()).is_ok());
    // Within a file, rows are in strictly increasing order of their entries.
    let mut back = piece("A", &[1, 2], 2);
    back.entry = vec![1, 0];
    let files = vec![serde_json::json!({"sample": "A", "path": "./a.root", "rows": 2})];
    let refused = pack(vec![back], files).unwrap_err().to_string();
    assert!(refused.contains("strictly increasing"), "{refused}");
    let twice = vec![
        ("x".to_owned(), Values::Flat(array(&[1_i8, 2]))),
        ("x".to_owned(), Values::Flat(array(&[1_i8, 2]))),
    ];
    assert!(one(twice).is_err());
    let same = vec![Sample { name: "A".into(), label: "A".into(), rows: 0 }; 2];
    let rows = rowver::write::Rows { samples: same, files: Vec::new(), entry: rowver::write::Source::Values(Values::Flat(array::<i64>(&[]))), chunk_rows: 65_536 };
    let beside = std::env::temp_dir().join(format!("rowver-test-{}-twice.rvr", std::process::id()));
    assert!(rowver::write::stream(rows, Vec::new(), 1, &beside).is_err());
}

// SPEC 2.6; FORMAT 4
#[test]
fn a_jagged_column_may_hold_values_that_are_not_finite() {
    for muon in [f32::NAN, f32::INFINITY] {
        let (header, arrays) = rebuilt(muon);
        let path = common::written(&to_bytes(&header, &arrays), "muon");
        let held = rowver::rvr::Reader::open(&path).unwrap().column("Muon_pt").unwrap().unwrap();
        assert!(!f32::from_le_bytes(held.bytes[..4].try_into().unwrap()).is_finite());
    }
}

// FORMAT 4
#[test]
fn nice_is_lim_rounded_outward_to_two_digits_in_exact_decimal() {
    let nice = |values| one(vec![("x".into(), Values::Flat(values))]).unwrap().0["cols"][0]["nice"].to_string();
    assert_eq!(nice(array(&[1.2_f64, 5.0])), "[1.2,5.0]");
    let inexact = serde_json::from_str::<Vec<f64>>("[0.1,0.30000000000000004]").unwrap();
    assert_eq!(nice(array(&inexact)), "[0.1,0.31]");
    assert_eq!(nice(array(&[5_i64, 120000001])), "[5,130000000]");
    assert_eq!(nice(array(&[1.1_f32, 1.1])), "[1.1,1.2]");
    assert_eq!(nice(array(&[5_u64, 9200000000000000001])), "[5,9300000000000000000]");
    assert_eq!(nice(array(&[-1.2_f64, -1.2])), "[-1.2,-1.1]");
    assert_eq!(nice(array(&[-120000001_i64, -7])), "[-130000000,-7]");
    assert_eq!(nice(array(&[150.1_f64, 1583.2])), "[150.0,1600.0]");
    assert_eq!(nice(array(&[-0.037_f64, 41.0])), "[-0.037,41.0]");
    assert_eq!(nice(array(&[true, true])), "[1,2]");
    assert_eq!(nice(array(&[f64::NAN, f64::INFINITY])), "[0.0,1.0]");
}

// FORMAT 10, 12
#[test]
fn a_derived_column_is_a_record_with_no_array_that_the_header_check_takes() {
    let bytes = golden();
    let (header, arrays) = common::split(&bytes);
    let arrays = vec![arrays];
    let rows: Vec<usize> = header["data"]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["rows"].as_u64().unwrap() as usize)
        .collect();
    let path = common::written(&bytes, "derived");
    let met = rowver::rvr::Reader::open(&path).unwrap().column("MET").unwrap().unwrap();
    let twice: Vec<f32> = met
        .bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()) * 2.)
        .collect();
    let twice = rowver::rvr::Held {
        kind: "f4".into(),
        bytes: twice.iter().flat_map(|v| v.to_le_bytes()).collect(),
        offsets: None,
    };
    let record = rowver::write::derived("twice", "MET * 2", &twice, &rows, 65_536).unwrap();
    // A chunk_rows out of bounds, and bytes that are not whole values, are refused.
    let said = rowver::write::derived("twice", "MET * 2", &twice, &rows, 0).unwrap_err().to_string();
    assert!(said.contains("chunk_rows is 0, and a chunk holds from 1,024 to 16,777,216 rows"), "{said}");
    let ragged = rowver::rvr::Held { bytes: twice.bytes[1..].to_vec(), ..twice.clone() };
    let said = rowver::write::derived("twice", "MET * 2", &ragged, &rows, 65_536).unwrap_err().to_string();
    assert!(said.contains("are not whole values of f4, 4 bytes each"), "{said}");
    assert_eq!(record["expression"], "MET * 2");
    assert!(record.get("at").is_none() && record.get("size").is_none());
    assert_eq!(record["by_sample"].as_array().unwrap().len(), rows.len());
    let checked = |record: Value| {
        let mut with = header.clone();
        with["derived"] = serde_json::json!({ "columns": [record] });
        let path = common::written(&to_bytes(&with, &arrays), "derived-record");
        rowver::rvr::Reader::open(&path).map(drop)
    };
    checked(record.clone()).unwrap();
    let refused = |change: &dyn Fn(&mut Value)| {
        let mut wrong = record.clone();
        change(&mut wrong);
        checked(wrong).unwrap_err().to_string()
    };
    assert!(refused(&|r| r["name"] = "MET".into()).contains("a derived column's name is one no stored"));
    assert!(refused(&|r| { r.as_object_mut().unwrap().shift_remove("by_sample"); }).contains("by_sample is missing"));
    assert!(refused(&|r| r["kind"] = "f16".into()).contains("kind is \"f16\", and a kind is one of"));
    assert!(refused(&|r| r["expression"] = " ".into()).contains("expression is blank"));
}

// FORMAT 1, 10, 12
#[test]
fn a_reader_works_out_a_derived_column_and_validation_checks_it() {
    let (mut header, arrays) = common::split(&golden());
    let arrays = vec![arrays];
    let text = common::recipe() + "twice = \"MET * 2\"\n";
    let text = text.as_str();
    header["made"]["recipe"]["text"] = text.into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(text).into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let written = |header: &Value, name: &str| common::written(&to_bytes(header, &arrays), name);
    let plain = written(&header, "derived-plain");
    let derived = [("twice".to_owned(), "MET * 2".to_owned())];
    let opened = rowver::rvr::Reader::inspect(&plain).unwrap().0;
    let records = rowver::rvr::derived::records(&opened, &derived, &[]).unwrap();
    header["derived"] = serde_json::json!({ "columns": records });
    let path = written(&header, "derived-twice");
    let reader = rowver::rvr::Reader::open(&path).unwrap();
    let (met, twice) = (reader.column("MET").unwrap().unwrap(), reader.column("twice").unwrap().unwrap());
    let doubled: Vec<u8> = met.bytes.chunks_exact(4).flat_map(|b| (f32::from_le_bytes(b.try_into().unwrap()) * 2.).to_le_bytes()).collect();
    assert_eq!((twice.kind.as_str(), &twice.bytes), ("f4", &doubled));
    assert_eq!(rowver::rvr::validate_file(&path).unwrap().derived, 1);
    // A file is not written again into itself, which would empty it first.
    let refused = rowver::write::rewrite(&path, &path, &header).unwrap_err().to_string();
    assert!(refused.contains("cannot be written again into itself"), "{refused}");
    assert_eq!(rowver::rvr::validate_file(&path).unwrap().derived, 1);
    drop(path);
    // A record its expression does not give is refused when the file is validated.
    let sha256 = records[0]["sha256"].clone();
    header["derived"]["columns"][0]["sha256"] = "0".repeat(64).into();
    let path = written(&header, "derived-changed");
    let refused = rowver::rvr::validate_file(&path).unwrap_err().to_string();
    drop(path);
    assert!(refused.contains("derived.columns[\"twice\"], worked out here, is not what the file records"), "{refused}");
    // A newer rvr's record that this rowver works out otherwise is not replaced: it says to update.
    let made = header["made"]["toolchain"]["version"].clone();
    header["made"]["toolchain"]["version"] = "15.99.0".into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let path = written(&header, "derived-changed-newer");
    let refused = rowver::rvr::validate_file(&path).unwrap_err().to_string();
    drop(path);
    assert!(refused.contains("is not what the file records") && refused.contains("newer than this one") && !refused.contains("rvr derive"), "{refused}");
    header["made"]["toolchain"]["version"] = made;
    // A newer rvr's derived column, with a function this rowver does not know, says to update;
    // its stored columns still read.
    header["derived"]["columns"][0]["sha256"] = sha256;
    let later = text.replace("MET * 2", "later(MET)");
    header["made"]["recipe"]["text"] = later.as_str().into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(&later).into();
    header["derived"]["columns"][0]["expression"] = "later(MET)".into();
    let this = env!("CARGO_PKG_VERSION");
    // A second derived column that uses the first: the hint is said once, not at each level.
    let both = format!("{later}again = \"twice * 2\"\n");
    header["made"]["recipe"]["text"] = both.as_str().into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(&both).into();
    let mut again = header["derived"]["columns"][0].clone();
    again["name"] = "again".into();
    again["expression"] = "twice * 2".into();
    header["derived"]["columns"] = serde_json::json!([header["derived"]["columns"][0].clone(), again]);
    for (made, newer) in [("15.99.0", true), (this, false)] {
        header["made"]["toolchain"]["version"] = made.into();
        header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
        let path = written(&header, "derived-newer");
        let reader = rowver::rvr::Reader::open(&path).unwrap();
        let stored = reader.column("MET").unwrap().is_some();
        let said = [reader.column("again").unwrap_err().to_string(), rowver::rvr::validate_file(&path).unwrap_err().to_string()];
        // Emptied, it fails as before: a function's name is checked before any column it reads.
        rowver::write::empty(rowver::change::Locked::take(&path).unwrap()).unwrap();
        let emptied = rowver::rvr::Reader::open(&path).unwrap().column("again").unwrap_err().to_string();
        drop(path);
        assert!(stored && emptied.contains("There is no function named later") && emptied.contains("newer") == newer, "{emptied}");
        for refused in said {
            let updates = refused.matches("The file was made by rvr 15.99.0, newer than this one").count();
            assert!(refused.contains("There is no function named later") && updates == usize::from(newer), "{refused}");
        }
    }
    // Another program's file is named for what made it, with no advice to update rvr to its version.
    header["made"]["toolchain"]["program"] = "rvrcpp".into();
    header["made"]["toolchain"]["version"] = "15.1.0".into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let path = written(&header, "derived-other-program");
    let refused = rowver::rvr::Reader::open(&path).unwrap().column("again").unwrap_err().to_string();
    drop(path);
    assert!(refused.contains("The file was made by rvrcpp 15.1.0, which knows a function this rvr does not") && !refused.contains("update rvr"), "{refused}");
    header["made"]["toolchain"]["program"] = "rvr".into();
    // Syntax a newer rvr's recipe holds that this one cannot read says to update too.
    header["derived"]["columns"] = serde_json::json!([header["derived"]["columns"][0].clone()]);
    let unread = later.replace("later(MET)", "MET ?? 2");
    header["made"]["recipe"]["text"] = unread.as_str().into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(&unread).into();
    header["derived"]["columns"][0]["expression"] = "MET ?? 2".into();
    header["made"]["toolchain"]["version"] = "15.99.0".into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let path = written(&header, "derived-unread");
    let refused = rowver::rvr::validate_file(&path).unwrap_err().to_string();
    drop(path);
    assert!(refused.contains("cannot be read") && refused.contains("newer than this one"), "{refused}");
}

// SPEC 4.6.1, 7.2.2, 7.6.1, 8.2; FORMAT 7
#[test]
fn a_streamed_file_is_the_one_built_whole_from_spooled_or_copied_columns() {
    use rowver::write::{stream, write_laid, Rows, Source, Spool};
    let dir = std::env::temp_dir().join(format!("rowver-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The committed fixture, built whole.
    let (header, arrays) = rebuilt(10.);
    let whole = header["data"].clone();
    let samples = vec![
        Sample {
            name: "A".into(),
            label: "Sample A, π⁰".into(),
            rows: 3,
        },
        Sample {
            name: "B".into(),
            label: "B".into(),
            rows: 2,
        },
    ];
    let files = whole["files"].as_array().unwrap().clone();
    let entry = [0_i64, 2, 4, 1, 17];
    let file = |name: &str| dir.join(name);
    let streamed = |columns: Vec<(String, Source)>, entry: Source, name: &str| {
        let rows = Rows {
            samples: samples.clone(),
            files: files.clone(),
            entry,
            chunk_rows: 65_536,
        };
        let (mut data, laid) = stream(rows, columns, 3, &file(name)).unwrap();
        data["sample_cols"] = whole["sample_cols"].clone();
        let mut written = header.clone();
        written["data"] = data;
        // Derived columns worked out from the arrays laid, before the file is written, are
        // those worked out from the file.
        let derived = [("twice".to_owned(), "MET * 2".to_owned()), ("lead".to_owned(), "first(Muon_pt, -1) + nlep".to_owned())];
        let early = laid.derived(&written, &derived).unwrap();
        write_laid(&written, laid).unwrap();
        let reader = rowver::rvr::Reader::open(file(name)).unwrap();
        assert_eq!(early, rowver::rvr::derived::records(&reader, &derived, &[]).unwrap(), "{name}");
        assert_eq!(
            std::fs::read(file(name)).unwrap(),
            to_bytes(&header, &arrays),
            "{name}"
        );
    };
    // Every column and the entries in uneven pieces, added out of order, one of them empty.
    let spool = Spool::create(&file("spooled.rvr.spool")).unwrap();
    let cuts = [
        (3, 5, (1, 0)),
        (1, 3, (0, 1)),
        (0, 1, (0, 0)),
        (3, 3, (0, 2)),
    ];
    for (from, to, order) in cuts {
        let entries = Values::Flat(array(&entry[from..to]));
        let columns = common::columns(from, to, 10.);
        spool.add(columns.len(), order, entries).unwrap();
        for (index, (_, values)) in columns.into_iter().enumerate() {
            spool.add(index, order, values).unwrap();
        }
    }
    let names: Vec<String> = common::columns(0, 5, 10.)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let columns = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                name.clone(),
                Source::Spooled(spool.take(index).unwrap().unwrap()),
            )
        })
        .collect();
    let entries = Source::Spooled(spool.take(names.len()).unwrap().unwrap());
    streamed(columns, entries, "spooled.rvr");
    // Every column copied from that file as it lies.
    let reader = rowver::rvr::Reader::open(file("spooled.rvr")).unwrap();
    let copied = names
        .iter()
        .map(|name| (name.clone(), Source::Copied(&reader, name.clone())))
        .collect();
    streamed(
        copied,
        Source::Values(Values::Flat(array(&entry))),
        "copied.rvr",
    );
    // A damaged array is refused, not copied: the first byte of its last column's first frame.
    let mut bytes = std::fs::read(file("spooled.rvr")).unwrap();
    let last = reader.header()["data"]["cols"].as_array().unwrap().last().unwrap();
    // The arrays begin at byte 8 (FORMAT §1).
    bytes[8 + last.get("offsets").unwrap_or(last)["at"].as_u64().unwrap() as usize] ^= 0xff;
    std::fs::write(file("damaged.rvr"), bytes).unwrap();
    let damaged = rowver::rvr::Reader::open(file("damaged.rvr")).unwrap();
    let last = names.last().unwrap().clone();
    let rows = |entry: &[i64]| Rows {
        samples: samples.clone(),
        files: files.clone(),
        entry: Source::Values(Values::Flat(array(entry))),
        chunk_rows: 65_536,
    };
    // On one thread, and decoded on several, each chunk written in order.
    for workers in [1, 3] {
        let refused = stream(rows(&entry), vec![(last.clone(), Source::Copied(&damaged, last.clone()))], workers, &file("x.rvr"));
        assert!(refused.unwrap_err().to_string().contains("is damaged"), "{workers}");
    }
    std::fs::remove_file(file("damaged.rvr")).unwrap();
    // A record that does not match its array is refused, though the file opens: MET's by_sample
    // is not under the data checksum.
    let mut lying = header.clone();
    lying["data"]["cols"][1]["by_sample"][0] = "0".repeat(64).into();
    std::fs::write(file("lying.rvr"), to_bytes(&lying, &arrays)).unwrap();
    let lying = rowver::rvr::Reader::open(file("lying.rvr")).unwrap();
    let met = vec![("MET".to_owned(), Source::Copied(&lying, "MET".into()))];
    let refused = stream(rows(&entry), met, 1, &file("x.rvr"));
    assert!(refused
        .unwrap_err()
        .to_string()
        .contains("does not hold what its record says"));
    // Facts that are not its values', which the file opens with, are refused once every chunk is
    // copied: nlep holds 0, 1, 2 and 3.
    let mut lying = header.clone();
    lying["data"]["cols"][0]["vals"] = serde_json::json!([0, 1, 3]);
    rowver::write::data_sealed(&mut lying["data"]).unwrap();
    std::fs::write(file("lying.rvr"), to_bytes(&lying, &arrays)).unwrap();
    let lying = rowver::rvr::Reader::open(file("lying.rvr")).unwrap();
    for workers in [1, 3] {
        let refused = stream(rows(&entry), vec![("nlep".to_owned(), Source::Copied(&lying, "nlep".into()))], workers, &file("x.rvr"));
        assert!(refused.unwrap_err().to_string().contains("does not hold what its record says"), "{workers}");
    }
    std::fs::remove_file(file("lying.rvr")).unwrap();
    // Nothing is left beside the files but the spool, which its maker removes.
    drop(spool);
    std::fs::remove_file(file("spooled.rvr.spool")).unwrap();
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["copied.rvr", "spooled.rvr"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

// FORMAT 4; SPEC 8.2
#[test]
fn a_copied_column_with_zeros_of_both_signs_keeps_a_whole_press_facts_however_its_chunks_finish() {
    use rowver::write::{stream, write_laid, Rows, Source};
    let n = 4 * 1024;
    let dir = std::env::temp_dir().join(format!("rowver-zeros-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rows = || Rows {
        samples: vec![Sample { name: "A".into(), label: "A".into(), rows: n }],
        files: vec![serde_json::json!({"sample": "A", "path": "./a.root", "rows": n})],
        entry: Source::Values(Values::Flat(array(&(0..n as i64).collect::<Vec<_>>()))),
        chunk_rows: 1024,
    };
    // A zero of each sign, at the low end and at the high end, in the first chunk and the third.
    for (rest, first, later) in [(1.0, 0.0, -0.0), (1.0, -0.0, 0.0), (-1.0, 0.0, -0.0), (-1.0, -0.0, 0.0)] {
        let mut values = vec![rest; n];
        (values[5], values[2 * 1024 + 7]) = (first, later);
        let whole = dir.join("whole.rvr");
        let (data, laid) = stream(rows(), vec![("x".into(), Source::Values(Values::Flat(array(&values))))], 1, &whole).unwrap();
        write_laid(&serde_json::json!({"rowver": 15, "data": data}), laid).unwrap();
        let (reader, _) = rowver::rvr::Reader::inspect(&whole).unwrap();
        let facts = |record: &Value| (record["lim"].to_string(), record["nice"].to_string());
        let pressed = facts(&reader.header()["data"]["cols"][0]);
        // Copied on one thread and on eight, its chunks finishing in any order.
        for workers in [1, 8] {
            for _ in 0..20 {
                let copied = stream(rows(), vec![("x".into(), Source::Copied(&reader, "x".into()))], workers, &dir.join("copied.rvr"));
                let (data, laid) = copied.unwrap_or_else(|e| panic!("{rest} {first} {later}, {workers} workers: {e}"));
                drop(laid);
                assert_eq!(facts(&data["cols"][0]), pressed);
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// SPEC 7.5.1
#[test]
fn a_stream_that_fails_removes_only_a_file_it_opened() {
    use rowver::write::{stream, Rows, Source};
    let path = common::written(b"not a file stream made", "stream-kept");
    let sample = |name: &str, rows: usize| Sample { name: name.into(), label: name.into(), rows };
    let rows = |samples: Vec<Sample>, entry: &[i64]| Rows {
        samples,
        files: vec![serde_json::json!({"sample": "A", "path": "./a.root", "rows": 2})],
        entry: Source::Values(Values::Flat(array(entry))),
        chunk_rows: 65_536,
    };
    // Refused before it opens the file (a sample named twice): what is there stays.
    let said = stream(rows(vec![sample("A", 1), sample("A", 1)], &[0, 1]), Vec::new(), 1, &path.0).unwrap_err().to_string();
    assert!(said.contains("given twice"), "{said}");
    assert_eq!(std::fs::read(&path).unwrap(), b"not a file stream made");
    // Failing once it has (a file's entries out of order): the file it began goes.
    let said = stream(rows(vec![sample("A", 2)], &[1, 0]), Vec::new(), 1, &path.0).unwrap_err().to_string();
    assert!(said.contains("strictly increasing"), "{said}");
    assert!(!path.0.exists());
}

// SPEC 7.5.1
#[test]
fn a_file_streamed_but_never_finished_goes_with_what_laid_it() {
    use rowver::write::{stream, Rows, Source};
    let path = common::Written(std::env::temp_dir().join(format!("rowver-test-{}-laid.rvr", std::process::id())));
    let rows = || Rows {
        samples: vec![Sample { name: "A".into(), label: "A".into(), rows: 2 }],
        files: vec![serde_json::json!({"sample": "A", "path": "./a.root", "rows": 2})],
        entry: Source::Values(Values::Flat(array(&[0_i64, 1]))),
        chunk_rows: 65_536,
    };
    let (_, laid) = stream(rows(), Vec::new(), 1, &path.0).unwrap();
    assert!(path.0.exists());
    drop(laid);
    assert!(!path.0.exists(), "a press stopped before its header goes leaves no arrays behind");
    let (data, laid) = stream(rows(), Vec::new(), 1, &path.0).unwrap();
    rowver::write::write_laid(&serde_json::json!({"rowver": 15, "data": data}), laid).unwrap();
    assert!(path.0.exists(), "a file finished stays");
}

// SPEC 1.5, 8.10
#[test]
fn a_failed_refill_or_rewrite_names_the_file_it_is_about() {
    let (header, arrays) = rebuilt(10.);
    let shell = common::written(&to_bytes(&header, &[]), "shell");
    let gone = shell.0.with_extension("gone");
    let to = shell.0.with_extension("to");
    let said = rowver::write::refilled(&rowver::change::Locked::take(&shell).unwrap(), &gone, &to).unwrap_err().to_string();
    assert_eq!(said, format!("{} is not there", gone.display()));
    let full = common::written(&to_bytes(&header, &arrays), "full");
    let said = rowver::write::refilled(&rowver::change::Locked::take(&full).unwrap(), &shell, &to).unwrap_err().to_string();
    assert!(said.starts_with(&format!("{}: the file to refill holds its arrays", full.0.display())), "{said}");
    let said = rowver::write::rewrite(&gone, &to, &header).unwrap_err().to_string();
    assert_eq!(said, format!("{} is not there", gone.display()));
    // Refilled from itself, named as it is or through a link: refused before it is opened again under its lock.
    let link = shell.0.with_extension("link");
    std::os::unix::fs::symlink(&shell.0, &link).unwrap();
    for from in [&shell.0, &link] {
        let said = rowver::write::refilled(&rowver::change::Locked::take(&shell).unwrap(), from, &to).unwrap_err().to_string();
        assert_eq!(said, format!("{} is {} itself: refill it from another file, a press of its own study", from.display(), shell.0.display()));
    }
    std::fs::remove_file(&link).unwrap();
    let _ = std::fs::remove_file(&to);
}

// SPEC 1.5, 8.2
#[test]
fn a_column_copied_from_a_file_cut_short_since_names_that_file() {
    use rowver::write::{array, stream, Rows, Source};
    let path = common::written(&golden(), "copied-from");
    let reader = rowver::rvr::Reader::open(&path).unwrap();
    // Cut short into its arrays, which lie from byte 8 to its header.
    std::fs::write(&path, &golden()[..8 + common::parts(&golden()).1.len() / 2]).unwrap();
    let samples = vec![Sample { name: "A".into(), label: "Sample A, π⁰".into(), rows: 3 }, Sample { name: "B".into(), label: "B".into(), rows: 2 }];
    let files = reader.header()["data"]["files"].as_array().unwrap().clone();
    let rows = Rows { samples, files, entry: Source::Values(Values::Flat(array(&[0_i64, 2, 4, 1, 17]))), chunk_rows: 65_536 };
    let beside = std::env::temp_dir().join(format!("rowver-test-{}-copied.rvr", std::process::id()));
    let refused = stream(rows, vec![("double".into(), Source::Copied(&reader, "double".into()))], 1, &beside).unwrap_err();
    let said = rowver::plain(&beside, &refused);
    assert_eq!(said, format!("{} changed while it was read (emptied, or written again): open it again", path.0.display()));
    assert_eq!(rowver::plain_what(&beside, &refused), format!("{} changed while it was read (emptied, or written again)", path.0.display()));
}
