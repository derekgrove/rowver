//! The Rust writer against a file the Python RowVer 13.0.0 wrote from the same arrays.

use rowver::write::{
    array, build, made_checksum, pack, sample_cols, to_bytes, Labels, Piece, Values,
};
use serde_json::{json, Value};

/// Decode a committed hexadecimal fixture.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text: String = std::fs::read_to_string(path)
        .unwrap()
        .split_whitespace()
        .collect();
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

/// The columns of `tests/generate_core_fixture.py` over the rows `from..to`, with a chosen first muon.
fn columns(from: usize, to: usize, muon: f32) -> Vec<(String, Values)> {
    let muons = [muon, 20., 30., 40., 50., 60.];
    let counts = [2_usize, 0, 1, 3, 0];
    let first: usize = counts[..from].iter().sum();
    let last: usize = counts[..to].iter().sum();
    let flat = |name: &str, values| (name.to_owned(), Values::Flat(values));
    vec![
        flat("nlep", array(&[2_i32, 0, 1, 3, 2][from..to])),
        flat("MET", array(&[20_f32, 0., 40., 30., 10.][from..to])),
        (
            "Muon_pt".into(),
            Values::Jagged {
                counts: counts[from..to].to_vec(),
                content: array(&muons[first..last]),
            },
        ),
        flat("signed_i1", array(&[-128_i8, -1, 0, 1, 127][from..to])),
        flat("signed_i2", array(&[-32768_i16, -1, 0, 1, 32767][from..to])),
        flat(
            "signed_i8",
            array(&[i64::MIN, -1, 0, (1 << 53) + 1, i64::MAX][from..to]),
        ),
        flat("unsigned_u1", array(&[0_u8, 1, 127, 128, 255][from..to])),
        flat(
            "unsigned_u2",
            array(&[0_u16, 1, 32767, 32768, 65535][from..to]),
        ),
        flat(
            "unsigned_u4",
            array(&[0_u32, 1, 1 << 31, u32::MAX - 1, u32::MAX][from..to]),
        ),
        flat(
            "unsigned_u8",
            array(&[0_u64, 1, (1 << 53) + 1, u64::MAX - 1, u64::MAX][from..to]),
        ),
        flat("flag", array(&[false, true, false, true, true][from..to])),
        flat(
            "double",
            array(&[-1.25_f64, 0., 1e-5, 1e16, 1.2345678901234567][from..to]),
        ),
    ]
}

/// Rebuild the fixture's header and arrays with the Rust writer, with a chosen first muon.
fn rebuilt(muon: f32) -> (Value, Vec<Vec<u8>>) {
    let pieces = vec![
        Piece {
            name: "A".into(),
            columns: columns(0, 3, muon),
            entry: vec![2, 0, 4],
        },
        Piece {
            name: "B".into(),
            columns: columns(3, 5, muon),
            entry: vec![(1 << 53) + 1, 1],
        },
    ];
    let files = vec![
        json!({"sample": "A", "path": "fixture-a.root", "rows": 2}),
        json!({"sample": "A", "path": "fixture-empty.root", "rows": 0}),
        json!({"sample": "A", "path": "fixture-a2.root", "rows": 1}),
        json!({"sample": "B", "path": "fixture-b.root", "rows": 2}),
    ];
    let mut labels = Labels::new();
    labels.insert("flag".into(), [(0, "off".into()), (1, "on".into())].into());
    let (mut data, arrays) = pack(pieces, labels, files).unwrap();
    let source = "# Synthetic reader test: π and μ\n";
    let record = |path: &str, entries: u64| json!({"tree": "Events", "uuid": format!("{}-uuid", path.trim_end_matches(".root")), "entries": entries});
    let mut made = json!({
        "study": "native-reader-test", "built": "2026-10-05T00:00:00Z", "by": "rowver 13",
        "cart": {"source": source, "sha256": null},
        "files": {
            "A": {"files": {
                "fixture-a.root": record("fixture-a.root", 10),
                "fixture-empty.root": record("fixture-empty.root", 10),
                "fixture-a2.root": record("fixture-a2.root", 10),
            }, "metadata": {"xsec": 87.3, "kind": "background"}},
            "B": {"files": {"fixture-b.root": record("fixture-b.root", (1 << 53) + 10)}, "metadata": {}},
        },
        "fixture_floats": null,
    });
    let floats = "[1e-4, 1e-5, 1e15, 1e16, 1e20, 1.1e-5, 0.0, -0.0, 1.2345678901234567, 1e-308, 1345740272956748.25, -101568684769612.625]";
    made["fixture_floats"] = serde_json::from_str(floats).unwrap();
    // The recipe text's own checksum is the presser's to compute; take the recorded one.
    let python = rowver::rvr::from_bytes(&fixture("rowver-v13.hex")).unwrap();
    made["cart"]["sha256"] = python.header()["made"]["cart"]["sha256"].clone();
    let names: Vec<String> = columns(0, 5, muon)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let taken: Vec<&str> = names.iter().map(String::as_str).collect();
    data["sample_cols"] = sample_cols(&["A", "B"], &made["files"], &taken)
        .unwrap()
        .into();
    made["sha256"] = made_checksum(&made).unwrap().into();
    let header =
        json!({"rowver": 13, "data": data, "made": made, "fixture_tool": {"preserved": "μ"}});
    (header, arrays)
}

/// Drop where each compressed array lies, which depends on the zlib implementation.
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
fn rust_file_validates_and_holds_what_python_wrote() {
    let (header, arrays) = rebuilt(10.);
    let bytes = to_bytes(&header, &arrays).unwrap();
    let table = rowver::rvr::from_bytes(&bytes).unwrap();
    let python = rowver::rvr::from_bytes(&fixture("rowver-v13.hex")).unwrap();
    let (mut ours, mut theirs) = (table.header().clone(), python.header().clone());
    without_spans(&mut ours);
    without_spans(&mut theirs);
    assert_eq!(
        serde_json::to_string(&ours).unwrap(),
        serde_json::to_string(&theirs).unwrap()
    );
    assert_eq!(
        table.header()["data"]["sha256"],
        python.header()["data"]["sha256"]
    );
    assert_eq!(
        table.header()["made"]["sha256"],
        python.header()["made"]["sha256"]
    );
}

#[test]
fn build_refuses_what_the_format_refuses() {
    let skim = |columns| rowver::write::Skim {
        samples: vec![("A".into(), 2)],
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
            samples: vec![("A".into(), 2)],
            files: Vec::new(),
            sample: vec![0, 0],
            entry: vec![0, 1],
            columns: vec![("x".into(), Values::Flat(values))],
            labels: Labels::new(),
        };
        build(skim).unwrap().0["cols"][0]["nice"].to_string()
    };
    // `tests/test_facts.py` in the Python RowVer 13.0.1 pins the same values.
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
