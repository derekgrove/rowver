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
    let rows = rowver::write::Rows { samples: same, files: Vec::new(), entry: rowver::write::Source::Values(Values::Flat(array::<i64>(&[]))) };
    let beside = std::env::temp_dir().join(format!("rowver-test-{}-twice.rvr", std::process::id()));
    assert!(rowver::write::stream(rows, Vec::new(), 1, &beside).is_err());
}

#[test]
fn a_jagged_column_may_hold_values_that_are_not_finite() {
    for muon in [f32::NAN, f32::INFINITY] {
        let (header, arrays) = rebuilt(muon);
        let path = common::written(&to_bytes(&header, &arrays), "muon");
        let held = rowver::rvr::Reader::open(&path).unwrap().column("Muon_pt").unwrap().unwrap();
        assert!(!f32::from_le_bytes(held.bytes[..4].try_into().unwrap()).is_finite());
    }
}

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

#[test]
fn a_reader_works_out_a_derived_column_and_validation_checks_it() {
    let bytes = golden();
    let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let arrays = vec![bytes[12 + length..].to_vec()];
    let mut header: Value = serde_json::from_slice(&bytes[12..12 + length]).unwrap();
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
    let made = header["made"]["toolchain"]["rvr"].clone();
    header["made"]["toolchain"]["rvr"] = "15.99.0".into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let path = written(&header, "derived-changed-newer");
    let refused = rowver::rvr::validate_file(&path).unwrap_err().to_string();
    drop(path);
    assert!(refused.contains("is not what the file records") && refused.contains("newer than this one") && !refused.contains("rvr derive"), "{refused}");
    header["made"]["toolchain"]["rvr"] = made;
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
        header["made"]["toolchain"]["rvr"] = made.into();
        header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
        let path = written(&header, "derived-newer");
        let reader = rowver::rvr::Reader::open(&path).unwrap();
        let stored = reader.column("MET").unwrap().is_some();
        let said = [reader.column("again").unwrap_err().to_string(), rowver::rvr::validate_file(&path).unwrap_err().to_string()];
        // Emptied, it fails as before: a function's name is checked before any column it reads.
        rowver::write::empty(&path).unwrap();
        let emptied = rowver::rvr::Reader::open(&path).unwrap().column("again").unwrap_err().to_string();
        drop(path);
        assert!(stored && emptied.contains("There is no function named later") && emptied.contains("newer") == newer, "{emptied}");
        for refused in said {
            let updates = refused.matches("The file was made by rvr 15.99.0, newer than this one").count();
            assert!(refused.contains("There is no function named later") && updates == usize::from(newer), "{refused}");
        }
    }
    // Syntax a newer rvr's recipe holds that this one cannot read says to update too.
    header["derived"]["columns"] = serde_json::json!([header["derived"]["columns"][0].clone()]);
    let unread = later.replace("later(MET)", "MET ?? 2");
    header["made"]["recipe"]["text"] = unread.as_str().into();
    header["made"]["recipe"]["sha256"] = rowver::write::text_checksum(&unread).into();
    header["derived"]["columns"][0]["expression"] = "MET ?? 2".into();
    header["made"]["toolchain"]["rvr"] = "15.99.0".into();
    header["made"]["sha256"] = rowver::write::made_checksum(&header["made"]).unwrap().into();
    let path = written(&header, "derived-unread");
    let refused = rowver::rvr::validate_file(&path).unwrap_err().to_string();
    drop(path);
    assert!(refused.contains("cannot be read") && refused.contains("newer than this one"), "{refused}");
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
    let entry = [0_i64, 2, 4, 1, 17];
    let file = |name: &str| dir.join(name);
    let streamed = |columns: Vec<(String, Source)>, entry: Source, name: &str| {
        let rows = Rows {
            samples: samples.clone(),
            files: files.clone(),
            entry,
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
    std::fs::write(file("lying.rvr"), to_bytes(&lying, &arrays)).unwrap();
    let lying = rowver::rvr::Reader::open(file("lying.rvr")).unwrap();
    let rows = Rows {
        samples: samples.clone(),
        files: files.clone(),
        entry: Source::Values(Values::Flat(array(&entry))),
    };
    let met = vec![("MET".to_owned(), Source::Copied(&lying, "MET".into()))];
    let refused = stream(rows, met, 1, &file("x.rvr"));
    assert!(refused
        .unwrap_err()
        .to_string()
        .contains("does not hold what its record says"));
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

#[test]
fn a_failed_refill_or_rewrite_names_the_file_it_is_about() {
    let (header, arrays) = rebuilt(10.);
    let shell = common::written(&to_bytes(&header, &[]), "shell");
    let gone = shell.0.with_extension("gone");
    let to = shell.0.with_extension("to");
    let said = rowver::write::refilled(&shell, &gone, &to).unwrap_err().to_string();
    assert_eq!(said, format!("{} is not there", gone.display()));
    let full = common::written(&to_bytes(&header, &arrays), "full");
    let said = rowver::write::refilled(&full, &shell, &to).unwrap_err().to_string();
    assert!(said.starts_with(&format!("{}: the file to refill holds its arrays", full.0.display())), "{said}");
    let said = rowver::write::rewrite(&gone, &to, &header).unwrap_err().to_string();
    assert_eq!(said, format!("{} is not there", gone.display()));
    let _ = std::fs::remove_file(&to);
}

#[test]
fn a_column_copied_from_a_file_cut_short_since_names_that_file() {
    use rowver::write::{array, stream, Rows, Source};
    let path = common::written(&golden(), "copied-from");
    let reader = rowver::rvr::Reader::open(&path).unwrap();
    std::fs::write(&path, &golden()[..golden().len() / 2]).unwrap();
    let samples = vec![Sample { name: "A".into(), label: "Sample A, π⁰".into(), rows: 3 }, Sample { name: "B".into(), label: "B".into(), rows: 2 }];
    let files = reader.header()["data"]["files"].as_array().unwrap().clone();
    let rows = Rows { samples, files, entry: Source::Values(Values::Flat(array(&[0_i64, 2, 4, 1, 17]))) };
    let beside = std::env::temp_dir().join(format!("rowver-test-{}-copied.rvr", std::process::id()));
    let refused = stream(rows, vec![("double".into(), Source::Copied(&reader, "double".into()))], 1, &beside).unwrap_err();
    let said = rowver::plain(&beside, &refused);
    assert_eq!(said, format!("{} changed while it was read (emptied, or written again): open it again", path.0.display()));
    assert_eq!(rowver::plain_what(&beside, &refused), format!("{} changed while it was read (emptied, or written again)", path.0.display()));
}
