//! The writer against the committed fixture, and what it refuses.

mod common;

use common::{golden, rebuilt};
use rowver::write::{array, build, pack, to_bytes, Labels, Piece, Sample, Values};
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

#[test]
fn the_writer_makes_the_committed_fixture() {
    let (header, arrays) = rebuilt(10.);
    let table = rowver::rvr::from_bytes(&to_bytes(&header, &arrays).unwrap()).unwrap();
    let committed = rowver::rvr::from_bytes(&golden()).unwrap();
    let (mut ours, mut theirs) = (table.header().clone(), committed.header().clone());
    without_spans(&mut ours);
    without_spans(&mut theirs);
    assert_eq!(
        serde_json::to_string(&ours).unwrap(),
        serde_json::to_string(&theirs).unwrap()
    );
    for section in ["data", "made"] {
        assert_eq!(
            table.header()[section]["sha256"],
            committed.header()[section]["sha256"]
        );
    }
}

#[test]
fn build_refuses_what_the_format_refuses() {
    let skim = |columns| rowver::write::Skim {
        samples: vec![Sample {
            name: "A".into(),
            label: "A".into(),
            rows: 2,
        }],
        files: Vec::new(),
        sample: vec![0, 0],
        entry: vec![0, 1],
        columns,
        labels: Labels::new(),
    };
    let flat = |values| vec![("x".to_owned(), Values::Flat(values))];
    assert!(build(skim(flat(array(&[1.0_f32, f32::NAN])))).is_err());
    assert!(build(skim(flat(array(&[1_i32])))).is_err());
    let jagged = Values::Jagged {
        counts: vec![1, 1],
        content: array(&[1.0_f64, f64::INFINITY]),
    };
    assert!(build(skim(vec![("x".into(), jagged)])).is_ok());
    let short = Values::Jagged {
        counts: vec![1, 2],
        content: array(&[1.0_f64, 2.0]),
    };
    assert!(build(skim(vec![("x".into(), short)])).is_err());
    let mut wrong = skim(flat(array(&[1_i8, 2])));
    wrong.sample = vec![0, 1];
    assert!(build(wrong).is_err());
    let piece = |name: &str, values: &[i8], rows: i64| Piece {
        name: name.into(),
        label: name.into(),
        columns: flat(array(values)),
        entry: (0..rows).collect(),
    };
    let uneven = vec![piece("A", &[1, 2, 3], 2), piece("B", &[4], 2)];
    assert!(pack(uneven, Labels::new(), Vec::new()).is_err());
    let even = vec![piece("A", &[1, 2], 2), piece("B", &[3, 4], 2)];
    assert!(pack(even, Labels::new(), Vec::new()).is_ok());
    let twice = vec![
        ("x".to_owned(), Values::Flat(array(&[1_i8, 2]))),
        ("x".to_owned(), Values::Flat(array(&[1_i8, 2]))),
    ];
    assert!(build(skim(twice)).is_err());
}

#[test]
fn a_jagged_column_may_hold_values_that_are_not_finite() {
    for muon in [f32::NAN, f32::INFINITY] {
        let (header, arrays) = rebuilt(muon);
        let table = rowver::rvr::from_bytes(&to_bytes(&header, &arrays).unwrap()).unwrap();
        let value = table.column("Muon_pt").unwrap().content_value(0).unwrap();
        assert!(!value.is_finite());
    }
}

#[test]
fn nice_keeps_the_limit_where_two_digit_rounding_fell_inside() {
    let nice = |values| {
        let skim = rowver::write::Skim {
            samples: vec![Sample {
                name: "A".into(),
                label: "A".into(),
                rows: 2,
            }],
            files: Vec::new(),
            sample: vec![0, 0],
            entry: vec![0, 1],
            columns: vec![("x".into(), Values::Flat(values))],
            labels: Labels::new(),
        };
        build(skim).unwrap().0["cols"][0]["nice"].to_string()
    };
    assert_eq!(nice(array(&[1.2_f64, 5.0])), "[1.2,5.0]");
    let inexact = "[0.1,0.30000000000000004]";
    assert_eq!(
        nice(
            serde_json::from_str::<Vec<f64>>(inexact)
                .map(|v| array(&v))
                .unwrap()
        ),
        inexact
    );
    assert_eq!(nice(array(&[5_i64, 120000001])), "[5,120000001]");
    assert_eq!(nice(array(&[1.1_f32, 1.1])), "[1.1,1.100000023841858]");
    assert_eq!(
        nice(array(&[5_u64, 9200000000000000001])),
        "[5,9200000000000000001]"
    );
    assert_eq!(nice(array(&[-1.2_f64, -1.2])), "[-1.2000000000000002,-1.2]");
    assert_eq!(nice(array(&[-120000001_i64, -7])), "[-120000001,-7]");
    assert_eq!(nice(array(&[150.1_f64, 1583.2])), "[150.0,1600.0]");
    assert_eq!(nice(array(&[-0.037_f64, 41.0])), "[-0.037,41.0]");
}

#[test]
fn a_derived_column_is_a_record_with_no_array_that_the_header_check_takes() {
    let bytes = golden();
    let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let arrays = vec![bytes[12 + length..].to_vec()];
    let header: Value = serde_json::from_slice(&bytes[12..12 + length]).unwrap();
    let rows: Vec<usize> = header["data"]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["rows"].as_u64().unwrap() as usize)
        .collect();
    let path = common::written(&bytes, "derived");
    let met = rowver::rvr::Reader::open(&path).unwrap().column("MET").unwrap().unwrap();
    std::fs::remove_file(path).unwrap();
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
    let record = rowver::write::derived("twice", "MET * 2", &twice, &rows).unwrap();
    assert_eq!(record["expression"], "MET * 2");
    assert!(record.get("at").is_none() && record.get("size").is_none());
    assert_eq!(record["by_sample"].as_array().unwrap().len(), rows.len());
    let checked = |record: Value| {
        let mut with = header.clone();
        with["derived"] = serde_json::json!({ "columns": [record] });
        rowver::rvr::from_bytes(&to_bytes(&with, &arrays).unwrap()).map(drop)
    };
    checked(record.clone()).unwrap();
    let refused = |change: &dyn Fn(&mut Value)| {
        let mut wrong = record.clone();
        change(&mut wrong);
        checked(wrong).unwrap_err().to_string()
    };
    assert!(refused(&|r| r["name"] = "MET".into()).contains("needs a name no other column has"));
    assert!(refused(&|r| { r.as_object_mut().unwrap().shift_remove("by_sample"); }).contains("is missing by_sample"));
    assert!(refused(&|r| r["kind"] = "f16".into()).contains("its kind is not one the format defines"));
    assert!(refused(&|r| r["expression"] = " ".into()).contains("has no expression"));
}

#[test]
fn a_reader_works_out_a_derived_column_and_validation_checks_it() {
    let bytes = golden();
    let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let arrays = vec![bytes[12 + length..].to_vec()];
    let mut header: Value = serde_json::from_slice(&bytes[12..12 + length]).unwrap();
    let text = "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [\"MET\"]\n\n[derived]\ntwice = \"MET * 2\"\n";
    header["made"]["recipe"]["text"] = text.into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(text).into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let written = |header: &Value, name: &str| common::written(&to_bytes(header, &arrays).unwrap(), name);
    let plain = written(&header, "derived-plain");
    let derived = [("twice".to_owned(), "MET * 2".to_owned())];
    let records = rowver::rvr::derived::records(&rowver::rvr::Reader::open(&plain).unwrap(), &derived, &[]).unwrap();
    std::fs::remove_file(plain).unwrap();
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
    std::fs::remove_file(path).unwrap();
    // A record its expression does not give is refused when the file is validated.
    header["derived"]["columns"][0]["lim"][1] = 1e9.into();
    let path = written(&header, "derived-changed");
    let refused = rowver::rvr::validate_file(&path).unwrap_err().to_string();
    std::fs::remove_file(path).unwrap();
    assert!(refused.contains("derived column twice, worked out here, is not what the file records"), "{refused}");
}

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
    let mut labels = Labels::new();
    labels.insert("flag".into(), [(0, "off".into()), (1, "on".into())].into());
    let entry = [2_i64, 0, 4, 17, 1];
    let file = |name: &str| dir.join(name);
    let streamed = |columns: Vec<(String, Source)>, entry: Source, name: &str| {
        let rows = Rows {
            samples: samples.clone(),
            files: files.clone(),
            entry,
            labels: labels.clone(),
        };
        let (mut data, laid) = stream(rows, columns, 3, &file(name)).unwrap();
        data["sample_cols"] = whole["sample_cols"].clone();
        let mut written = header.clone();
        written["data"] = data;
        // Derived columns worked out from the arrays laid, before the file is written, are
        // those worked out from the file.
        let derived = [("twice".to_owned(), "MET * 2".to_owned()), ("lead".to_owned(), "first(Muon_pt, -1) + nlep".to_owned())];
        let early = laid.derived(&written, &derived).unwrap();
        write_laid(file(name), &written, laid).unwrap();
        let reader = rowver::rvr::Reader::open(file(name)).unwrap();
        assert_eq!(early, rowver::rvr::derived::records(&reader, &derived, &[]).unwrap(), "{name}");
        assert_eq!(
            std::fs::read(file(name)).unwrap(),
            to_bytes(&header, &arrays).unwrap(),
            "{name}"
        );
    };
    // Every column and the entries in uneven pieces, added out of order, one of them empty.
    let spool = Spool::beside(&file("spooled.rvr")).unwrap();
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
    // A damaged array is refused, not copied: the file's last byte lies in its last column.
    let mut bytes = std::fs::read(file("spooled.rvr")).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    std::fs::write(file("damaged.rvr"), bytes).unwrap();
    let damaged = rowver::rvr::Reader::open(file("damaged.rvr")).unwrap();
    let last = names.last().unwrap().clone();
    let rows = Rows {
        samples: samples.clone(),
        files: files.clone(),
        entry: Source::Values(Values::Flat(array(&entry))),
        labels: labels.clone(),
    };
    let refused = stream(
        rows,
        vec![(last.clone(), Source::Copied(&damaged, last))],
        1,
        &file("x.rvr"),
    );
    assert!(refused.unwrap_err().to_string().contains("is damaged"));
    std::fs::remove_file(file("damaged.rvr")).unwrap();
    // A record that does not match its array is refused, though the file opens: MET's by_sample
    // is not under the data checksum.
    let mut lying = header.clone();
    lying["data"]["cols"][1]["by_sample"][0] = "0".repeat(64).into();
    std::fs::write(file("lying.rvr"), to_bytes(&lying, &arrays).unwrap()).unwrap();
    let lying = rowver::rvr::Reader::open(file("lying.rvr")).unwrap();
    let rows = Rows {
        samples: samples.clone(),
        files: files.clone(),
        entry: Source::Values(Values::Flat(array(&entry))),
        labels: labels.clone(),
    };
    let met = vec![("MET".to_owned(), Source::Copied(&lying, "MET".into()))];
    let refused = stream(rows, met, 1, &file("x.rvr"));
    assert!(refused
        .unwrap_err()
        .to_string()
        .contains("does not hold what its record says"));
    std::fs::remove_file(file("lying.rvr")).unwrap();
    // Nothing is left beside the files.
    drop(spool);
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["copied.rvr", "spooled.rvr"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
