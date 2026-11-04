//! Generated inputs for the property tests: SplitMix64, as FORMAT.md §6.1 draws, and valid files
//! of every kind of column built from it. A case is named by its seed, and every failure says it.

use super::{pack, text, Piece};
use rowver::choice::Events;
use rowver::study::lock::{Branch, Branches, File, Lock, Sample, Shape};
use rowver::write::{array, made_checksum, sample_cols, text_checksum, NumericBuffer, Values};
use serde_json::{json, Map, Value};

/// SplitMix64, the generator of FORMAT.md §6.1.
pub struct Rng(u64);

impl Rng {
    /// The generator of one seed.
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// Its next draw: the state stepped by the golden gamma, then mixed.
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A whole number below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    /// One of `items`.
    pub fn one<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// How many cases a property runs: `ROWVER_CASES`, else `default`.
pub fn cases(default: u64) -> u64 {
    std::env::var("ROWVER_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(default)
}

/// The eleven kinds a column holds (FORMAT.md §4).
pub const KINDS: [&str; 11] = ["f8", "f4", "i8", "i4", "i2", "i1", "u8", "u4", "u2", "u1", "b1"];

/// `n` values of `kind`, a third of them the kind's edges or specials (NaN, ±inf, -0.0, subnormals).
pub fn values(rng: &mut Rng, kind: &str, n: usize) -> NumericBuffer {
    macro_rules! drawn {
        ($t:ty, $edges:expr, $any:expr) => {{
            let edges: Vec<$t> = $edges;
            let all: Vec<$t> = (0..n).map(|_| if rng.below(3) == 0 { *rng.one(&edges) } else { $any(rng.next()) }).collect();
            array(&all)
        }};
    }
    macro_rules! whole {
        ($t:ty) => {
            drawn!($t, vec![<$t>::MIN, <$t>::MAX, 0, 1, <$t>::MIN.wrapping_add(1), <$t>::MAX - 1], |bits: u64| bits as $t)
        };
    }
    match kind {
        "f8" => drawn!(f64, vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0, 0.0, 5e-324, f64::MIN_POSITIVE, f64::MAX, f64::MIN, 1.0, 0.1, -2.5], f64::from_bits),
        "f4" => drawn!(f32, vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0, 0.0, 1e-45, f32::MIN_POSITIVE, f32::MAX, f32::MIN, 1.0, 0.1, -2.5], |bits: u64| f32::from_bits(bits as u32)),
        "i8" => whole!(i64),
        "i4" => whole!(i32),
        "i2" => whole!(i16),
        "i1" => whole!(i8),
        "u8" => whole!(u64),
        "u4" => whole!(u32),
        "u2" => whole!(u16),
        "u1" => whole!(u8),
        _ => array(&(0..n).map(|_| rng.below(2) == 1).collect::<Vec<bool>>()),
    }
}

/// One column of a generated table: its name, its kind (its content's, for a list), and whether it is a list.
#[derive(Clone)]
pub struct Column {
    pub name: String,
    pub kind: String,
    pub jagged: bool,
}

/// A generated sample: its name, the entry of each row, the entries of its one file, its metadata.
pub type Generated = (String, Vec<i64>, u64, Map<String, Value>);

/// A generated study: its samples and columns.
pub struct Table {
    pub samples: Vec<Generated>,
    pub columns: Vec<Column>,
}

/// A table of 1 to 3 samples of 0 to `rows` rows each, and 1 to 6 columns of any kind, flat or lists.
pub fn table(rng: &mut Rng, rows: u64) -> Table {
    let columns = (0..1 + rng.below(6))
        .map(|i| Column { name: format!("c{i}"), kind: (*rng.one(&KINDS)).to_owned(), jagged: rng.below(2) == 0 })
        .collect();
    let samples = (0..1 + rng.below(3))
        .map(|s| {
            let entries = rng.below(rows * 2 + 1);
            let mut picked: Vec<i64> = (0..entries as i64).filter(|_| rng.below(2) == 0).collect();
            picked.truncate(rows as usize);
            let mut metadata = Map::new();
            for (key, value) in [("xsec", json!(rng.next() as f64 / 1e15)), ("kind", json!(format!("k{}", rng.below(3)))), ("n", json!(rng.below(9))), ("flag", json!(rng.below(2) == 0))] {
                if rng.below(2) == 0 {
                    metadata.insert(key.into(), value);
                }
            }
            (format!("s{s}"), picked, entries, metadata)
        })
        .collect();
    Table { samples, columns }
}

/// The values of each sample's rows for each column (`[sample][column]`).
pub fn filled(rng: &mut Rng, table: &Table) -> Vec<Vec<Values>> {
    table
        .samples
        .iter()
        .map(|(_, rows, _, _)| {
            table
                .columns
                .iter()
                .map(|c| match c.jagged {
                    false => Values::Flat(values(rng, &c.kind, rows.len())),
                    true => {
                        let counts: Vec<usize> = rows.iter().map(|_| rng.below(4) as usize * (rng.below(3) as usize)).collect();
                        Values::Jagged { content: values(rng, &c.kind, counts.iter().sum()), counts }
                    }
                })
                .collect()
        })
        .collect()
}

/// The samples.toml, samples.lock and recipe.toml of a table, as resolve would write the lock.
pub fn texts(table: &Table) -> (String, String, String) {
    let mut samples = "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"Events\"\n\n[eras]\n".to_owned();
    for (name, _, _, metadata) in &table.samples {
        let meta: Vec<String> = metadata.iter().map(|(k, v)| format!("{k} = {}", toml_value(v))).collect();
        samples += &format!("\n[[sample]]\nname = \"{name}\"\nfiles = \"./{name}.root\"\nmetadata = {{ {} }}\n", meta.join(", "));
    }
    let list = Branches::new(table.columns.iter().map(|c| Branch { name: c.name.clone(), kind: c.kind.clone(), shape: if c.jagged { Shape::Jagged } else { Shape::Flat }, why: None }).collect());
    let made = rowver::study::samples::read(&samples).unwrap();
    let locked = table
        .samples
        .iter()
        .zip(&made)
        .enumerate()
        .map(|(n, ((_, _, entries, _), sample))| {
            let file = File { at: sample.files[0].clone(), bytes: 1000 + n as u64, adler32: Some(format!("{n:08x}")), uuid: format!("31e851e4-b994-11f1-ba5c-{n:012x}"), entries: *entries, stop: None, take: None };
            Sample { name: sample.name.clone(), label: sample.label.clone(), asked: sample.files.clone(), tree: sample.tree.clone(), branches: list.sha256.clone(), events: Events::All, events_total: 0, events_read: 0, metadata: sample.metadata.clone(), files: vec![file] }.planned(Events::All)
        })
        .collect();
    let lock = Lock { resolved: "2026-11-04T00:00:00Z".into(), samples_toml_sha256: text_checksum(&samples), samples: locked, branches: vec![list] };
    let names: Vec<String> = table.columns.iter().map(|c| format!("{:?}", c.name)).collect();
    let recipe = format!("format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [{}]\n\n[totals]\n\n[derived]\n", names.join(", "));
    (samples, lock.write().unwrap(), recipe)
}

/// A JSON value of a sample's metadata as TOML writes it.
fn toml_value(value: &Value) -> String {
    match value {
        Value::Number(n) if n.is_f64() => format!("{:?}", n.as_f64().unwrap()),
        other => other.to_string(),
    }
}

/// The header and arrays of a table with these values, as a press of its study would write them.
pub fn file(table: &Table, values: Vec<Vec<Values>>) -> (Value, Vec<Vec<u8>>) {
    let (samples_text, lock_text, recipe_text) = texts(table);
    let lock = Lock::read(&lock_text).unwrap();
    let pieces = table
        .samples
        .iter()
        .zip(values)
        .map(|((name, rows, _, _), values)| Piece { name: name.clone(), label: name.clone(), columns: table.columns.iter().map(|c| c.name.clone()).zip(values).collect(), entry: rows.clone() })
        .collect();
    let files = table.samples.iter().map(|(name, rows, _, _)| json!({"sample": name, "path": format!("./{name}.root"), "rows": rows.len()})).collect();
    let (mut data, arrays) = pack(pieces, files).unwrap();
    let mut made_files = Map::new();
    for sample in &lock.samples {
        let file = &sample.files[0];
        let records = json!({ (&file.at): {"tree": sample.tree, "uuid": file.uuid, "entries": file.entries} });
        let mut metadata = sample.metadata.clone();
        metadata.insert("events_read".into(), sample.events_read.into());
        metadata.insert("events_total".into(), sample.events_total.into());
        made_files.insert(sample.name.clone(), json!({"events": sample.events.record(), "files": records, "metadata": metadata}));
    }
    let names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
    let mut recipe = text(&recipe_text);
    recipe["format"] = "rvr-recipe".into();
    recipe["columns"] = names.clone().into();
    let mut made = json!({
        "study": "generated",
        "built": "2026-11-04T00:00:00Z",
        "by": "rowver tests",
        "toolchain": {"program": "rvr", "version": "15.0.0", "rowver": "15", "commit": "unknown", "compiler": "rustc 1.91.0", "target": "test"},
        "recipe": recipe,
        "samples": text(&samples_text),
        "lock": text(&lock_text),
        "files": made_files,
    });
    let taken: Vec<&str> = names.iter().map(String::as_str).collect();
    let samples: Vec<&str> = table.samples.iter().map(|(n, _, _, _)| n.as_str()).collect();
    data["sample_cols"] = sample_cols(&samples, &made["files"], &taken).unwrap().into();
    made["sha256"] = made_checksum(&made).unwrap().into();
    (json!({"rowver": 15, "data": data, "made": made}), arrays)
}
