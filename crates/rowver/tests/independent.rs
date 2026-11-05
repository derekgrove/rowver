//! A second writer of `.rvr` 15 files, from docs/FORMAT.md alone: each test builds a file byte by
//! byte and asks rowver's full validation whether it is valid.

use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAGIC: [u8; 8] = [0x89, b'R', b'V', b'R', 0x0D, 0x0A, 0x1A, 0x0A];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn num(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

// ---- values (§4) ----

#[derive(Clone, Copy)]
enum Num {
    F(f64),
    I(i128),
}

trait Kind: Copy {
    const KIND: &'static str;
    fn le(self, out: &mut Vec<u8>);
    fn num(self) -> Num;
}

macro_rules! kinds {
    ($($t:ty => $k:literal, $n:expr;)*) => {$(
        impl Kind for $t {
            const KIND: &'static str = $k;
            fn le(self, out: &mut Vec<u8>) { out.extend_from_slice(&self.to_le_bytes()) }
            fn num(self) -> Num { $n(self) }
        }
    )*};
}

kinds! {
    f64 => "f8", |x: f64| Num::F(x);
    f32 => "f4", |x: f32| Num::F(x as f64);
    i64 => "i8", |x: i64| Num::I(x as i128);
    i32 => "i4", |x: i32| Num::I(x as i128);
    i16 => "i2", |x: i16| Num::I(x as i128);
    i8 => "i1", |x: i8| Num::I(x as i128);
    u64 => "u8", |x: u64| Num::I(x as i128);
    u32 => "u4", |x: u32| Num::I(x as i128);
    u16 => "u2", |x: u16| Num::I(x as i128);
    u8 => "u1", |x: u8| Num::I(x as i128);
}

impl Kind for bool {
    const KIND: &'static str = "b1";
    fn le(self, out: &mut Vec<u8>) {
        out.push(self as u8)
    }
    fn num(self) -> Num {
        Num::I(self as i128)
    }
}

#[derive(Clone)]
struct Vals {
    kind: &'static str,
    width: usize,
    le: Vec<u8>,
    nums: Vec<Num>,
}

fn vals<T: Kind>(values: impl IntoIterator<Item = T>) -> Vals {
    let mut le = Vec::new();
    let nums = values
        .into_iter()
        .map(|x| {
            x.le(&mut le);
            x.num()
        })
        .collect();
    Vals {
        kind: T::KIND,
        width: std::mem::size_of::<T>(),
        le,
        nums,
    }
}

impl Vals {
    fn bytes(&self, a: usize, b: usize) -> &[u8] {
        &self.le[a * self.width..b * self.width]
    }
}

enum Data {
    Flat(Vals),
    Jagged(Vec<i64>, Vals),
}

struct Col {
    name: String,
    data: Data,
}

struct Derived {
    name: String,
    expression: String,
    data: Data,
}

fn flat(name: &str, v: Vals) -> Col {
    Col {
        name: name.into(),
        data: Data::Flat(v),
    }
}

fn lists<T: Kind>(rows: &[Vec<T>]) -> Data {
    let mut offsets = vec![0i64];
    for l in rows {
        offsets.push(offsets.last().unwrap() + l.len() as i64);
    }
    Data::Jagged(offsets, vals(rows.iter().flatten().copied()))
}

fn jagged<T: Kind>(name: &str, rows: &[Vec<T>]) -> Col {
    Col {
        name: name.into(),
        data: lists(rows),
    }
}

fn derived(name: &str, expression: &str, data: Data) -> Derived {
    Derived {
        name: name.into(),
        expression: expression.into(),
        data,
    }
}

// ---- canonical JSON (§8) ----

fn canon(v: &Value) -> String {
    let mut out = String::new();
    canon_into(v, &mut out);
    out
}

fn canon_into(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&canon_number(&n.to_string())),
        Value::String(s) => canon_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canon_into(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canon_string(k, out);
                out.push(':');
                canon_into(&m[k], out);
            }
            out.push('}');
        }
    }
}

fn canon_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn is_whole(text: &str) -> bool {
    !text.contains(['.', 'e', 'E'])
}

fn canon_number(text: &str) -> String {
    match text {
        "-0" => "0".into(),
        t if is_whole(t) => t.into(),
        t => shortest(t.parse().unwrap()),
    }
}

/// The shortest digits of a double and the power of ten of the first.
fn digits(x: f64) -> (String, i32) {
    let s = format!("{:e}", x.abs());
    let (m, e) = s.split_once('e').unwrap();
    (m.replace('.', ""), e.parse().unwrap())
}

fn shortest(x: f64) -> String {
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x == 0.0 {
        return format!("{sign}0.0");
    }
    let (d, e) = digits(x);
    let body = if (-4..16).contains(&e) {
        let k = e + 1;
        if e < 0 {
            format!("0.{}{d}", "0".repeat((-e - 1) as usize))
        } else if d.len() <= k as usize {
            format!("{d}{}.0", "0".repeat(k as usize - d.len()))
        } else {
            format!("{}.{}", &d[..k as usize], &d[k as usize..])
        }
    } else {
        let rest = if d.len() > 1 {
            format!(".{}", &d[1..])
        } else {
            String::new()
        };
        format!("{}{rest}e{}{:02}", &d[..1], if e < 0 { '-' } else { '+' }, e.abs())
    };
    format!("{sign}{body}")
}

// ---- column facts (§4) ----

/// A decimal: its sign, its significant digits (none for zero), the power of ten of the first.
struct Dec {
    neg: bool,
    digits: String,
    e: i32,
}

fn dec_float(x: f64) -> Dec {
    if x == 0.0 {
        return Dec {
            neg: false,
            digits: String::new(),
            e: 0,
        };
    }
    let (digits, e) = digits(x);
    Dec {
        neg: x < 0.0,
        digits,
        e,
    }
}

fn dec_int(x: i128) -> Dec {
    let s = x.unsigned_abs().to_string();
    let digits = s.trim_end_matches('0').to_string();
    Dec {
        neg: x < 0,
        e: s.len() as i32 - 1,
        digits,
    }
}

/// One end of `nice` as `q × 10^p`: the multiple of `step = 10^(e − 1)` at or below (or above) it.
fn nice_end(d: &Dec, up: bool) -> (i128, i32) {
    if d.digits.is_empty() {
        return (0, 0);
    }
    let two: i128 = format!("{:0<2}", &d.digits[..d.digits.len().min(2)])
        .parse()
        .unwrap();
    let q = two + ((up != d.neg) && d.digits.len() > 2) as i128;
    (if d.neg { -q } else { q }, d.e - 1)
}

fn normal((mut q, mut p): (i128, i32)) -> (i128, i32) {
    if q == 0 {
        return (0, 0);
    }
    while q % 10 == 0 {
        q /= 10;
        p += 1;
    }
    (q, p)
}

fn dec_json((q, p): (i128, i32), whole: bool) -> Value {
    if whole {
        let v = if p >= 0 {
            q * 10i128.pow(p as u32)
        } else {
            q / 10i128.pow((-p) as u32)
        };
        return num(&v.to_string());
    }
    let x: f64 = format!("{q}e{p}").parse().unwrap();
    // Past the largest double, the nearest double is the largest (FORMAT.md does not say).
    json!(if x.is_finite() {
        x
    } else {
        f64::MAX.copysign(x)
    })
}

fn nice(lo: Dec, hi: Dec, whole: bool) -> Value {
    let low = nice_end(&lo, false);
    let mut high = nice_end(&hi, true);
    if normal(low) == normal(high) {
        high = if high.0 == 0 {
            (1, 0)
        } else if whole && high.1 < 0 {
            (high.0 + 10i128.pow((-high.1) as u32), high.1)
        } else {
            (high.0 + 1, high.1)
        };
    }
    json!([dec_json(low, whole), dec_json(high, whole)])
}

fn facts(v: &Vals, m: &mut Map<String, Value>) {
    if v.kind.starts_with('f') {
        let finite: Vec<f64> = v
            .nums
            .iter()
            .filter_map(|n| match n {
                Num::F(x) if x.is_finite() => Some(*x),
                _ => None,
            })
            .collect();
        if finite.is_empty() {
            m.insert("lim".into(), json!([0.0, 0.0]));
            m.insert("nice".into(), json!([0.0, 1.0]));
            return;
        }
        let lo = finite.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        m.insert("lim".into(), json!([lo, hi]));
        m.insert("nice".into(), nice(dec_float(lo), dec_float(hi), false));
        return;
    }
    let mut xs: Vec<i128> = v
        .nums
        .iter()
        .map(|n| match n {
            Num::I(x) => *x,
            Num::F(_) => unreachable!(),
        })
        .collect();
    xs.sort();
    xs.dedup();
    let int = |x: i128| num(&x.to_string());
    if xs.is_empty() {
        m.insert("lim".into(), json!([0, 0]));
        m.insert("nice".into(), json!([0, 1]));
    } else {
        let (lo, hi) = (xs[0], *xs.last().unwrap());
        m.insert("lim".into(), json!([int(lo), int(hi)]));
        m.insert("nice".into(), nice(dec_int(lo), dec_int(hi), true));
    }
    if xs.len() <= 64 {
        m.insert(
            "vals".into(),
            Value::Array(xs.into_iter().map(int).collect()),
        );
    }
}

// ---- arrays (§7) and their checksums (§8) ----

/// The rows of one chunk (§7): this writer gives no `chunk_rows`, so the default.
const R: usize = 65_536;

type Ranges = [(usize, usize)];

/// The file's chunks (§7): each sample's rows cut every R rows from its first, as (sample, first, end).
fn chunks_of(ranges: &Ranges) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (s, &(a, b)) in ranges.iter().enumerate() {
        let mut at = a;
        while at < b {
            let to = (at + R).min(b);
            out.push((s, at, to));
            at = to;
        }
    }
    out
}

/// One array of values `w` bytes wide from its chunks' values (§7): each chunk cut into `w` lanes,
/// each lane one zstd frame, chunk after chunk, then the chunk table; and each chunk's SHA-256.
fn stored(chunks: &[Vec<u8>], w: usize) -> (Vec<u8>, Vec<String>) {
    let (mut frames, mut table, mut shas) = (Vec::new(), Vec::new(), Vec::new());
    for chunk in chunks {
        for j in 0..w {
            let lane: Vec<u8> = chunk.iter().skip(j).step_by(w).copied().collect();
            let f = ruzstd::encoding::compress_to_vec(&lane[..], ruzstd::encoding::CompressionLevel::Fastest);
            table.extend((f.len() as u64).to_le_bytes());
            frames.extend(f);
        }
        let d = Sha256::digest(chunk);
        table.extend_from_slice(&d);
        shas.push(hex(&d));
    }
    frames.extend(table);
    (frames, shas)
}

/// The SHA-256 of these SHA-256s' hex texts joined (§8).
fn joined(shas: &[String]) -> String {
    sha(shas.concat().as_bytes())
}

/// An array's `sha256` and `by_sample` (§8) from its chunks' SHA-256s.
fn sums(cut: &[(usize, usize, usize)], shas: &[String], samples: usize) -> (String, Value) {
    let of = |s: usize| joined(&cut.iter().zip(shas).filter(|(c, _)| c.0 == s).map(|(_, h)| h.clone()).collect::<Vec<_>>());
    (joined(shas), json!((0..samples).map(of).collect::<Vec<_>>()))
}

/// The arrays of a file, in file order, and each one's `sha256`.
struct Store {
    bytes: Vec<u8>,
    shas: Vec<String>,
}

/// The chunks, rows and samples of a file.
struct Cut {
    chunks: Vec<(usize, usize, usize)>,
    samples: usize,
}

impl Store {
    /// One array from its chunks' values, after the others: its record's `at`, `size` and `sha256`,
    /// and `by_sample` when `per_sample`.
    fn put(&mut self, chunks: &[Vec<u8>], w: usize, cut: &Cut, per_sample: bool, m: &mut Map<String, Value>) {
        let (bytes, shas) = stored(chunks, w);
        m.insert("at".into(), json!(self.bytes.len()));
        m.insert("size".into(), json!(bytes.len()));
        self.bytes.extend(bytes);
        let (sha256, by_sample) = sums(&cut.chunks, &shas, cut.samples);
        self.shas.push(sha256.clone());
        m.insert("sha256".into(), json!(sha256));
        if per_sample {
            m.insert("by_sample".into(), by_sample);
        }
    }
}

/// An array's checksums with nothing stored (a derived column's): from its chunks' values.
fn measured(chunks: &[Vec<u8>], cut: &Cut, m: &mut Map<String, Value>) {
    let shas: Vec<String> = chunks.iter().map(|c| sha(c)).collect();
    let (sha256, by_sample) = sums(&cut.chunks, &shas, cut.samples);
    m.insert("sha256".into(), json!(sha256));
    m.insert("by_sample".into(), by_sample);
}

/// One array of the values `v` holds, or would hold, at `put` or measured.
fn array(chunks: Vec<Vec<u8>>, w: usize, cut: &Cut, store: Option<&mut Store>, m: &mut Map<String, Value>) {
    match store {
        Some(s) => s.put(&chunks, w, cut, true, m),
        None => measured(&chunks, cut, m),
    }
}

/// A column's record after its name: stored (with `at` and `size`) when `store` is given.
fn record(data: &Data, cut: &Cut, mut store: Option<&mut Store>, m: &mut Map<String, Value>) {
    match data {
        Data::Flat(v) => {
            m.insert("kind".into(), json!(v.kind));
            let chunks = cut.chunks.iter().map(|&(_, a, b)| v.bytes(a, b).to_vec()).collect();
            array(chunks, v.width, cut, store, m);
            facts(v, m);
        }
        Data::Jagged(off, content) => {
            m.insert("kind".into(), json!("jagged"));
            let mut o = Map::new();
            let steps = cut.chunks.iter().map(|&(_, a, b)| off[a..=b].iter().flat_map(|x| (x - off[a]).to_le_bytes()).collect()).collect();
            array(steps, 8, cut, store.as_deref_mut(), &mut o);
            let mut c = Map::new();
            c.insert("kind".into(), json!(content.kind));
            c.insert("count".into(), json!(content.nums.len()));
            let members = cut.chunks.iter().map(|&(_, a, b)| content.bytes(off[a] as usize, off[b] as usize).to_vec()).collect();
            array(members, content.width, cut, store, &mut c);
            m.insert("offsets".into(), Value::Object(o));
            m.insert("content".into(), Value::Object(c));
        }
    }
}

// ---- sample columns (§5) ----

fn sample_cols(metas: &[Map<String, Value>], columns: &[String]) -> Value {
    let mut keys: Vec<&String> = Vec::new();
    for m in metas {
        for k in m.keys() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    let cols: Vec<Value> = keys
        .into_iter()
        .filter_map(|k| sample_col(k, metas, columns))
        .collect();
    Value::Array(cols)
}

fn sample_col(key: &str, metas: &[Map<String, Value>], columns: &[String]) -> Option<Value> {
    if columns.iter().any(|c| c == key) || ["sample", "entry", "file"].contains(&key) {
        return None;
    }
    let vs: Vec<Option<&Value>> = metas
        .iter()
        .map(|m| m.get(key).filter(|v| !v.is_null()))
        .collect();
    let have: Vec<&Value> = vs.iter().flatten().copied().collect();
    let every = have.len() == metas.len();
    let texts: Vec<String> = have.iter().map(|v| v.to_string()).collect();
    let kind = if have.is_empty() {
        return None;
    } else if have.iter().all(|v| v.is_boolean()) {
        if every {
            "b1"
        } else {
            return None;
        }
    } else if have.iter().all(|v| v.is_string()) {
        "text"
    } else if !have.iter().all(|v| v.is_number()) {
        return None;
    } else if every && texts.iter().all(|t| is_whole(t)) {
        if texts.iter().all(|t| t.parse::<i64>().is_ok()) {
            "i8"
        } else {
            return None;
        }
    } else if texts
        .iter()
        .all(|t| t.parse::<f64>().is_ok_and(f64::is_finite))
    {
        "f8"
    } else {
        return None;
    };
    let mut bytes = Vec::new();
    for v in &vs {
        match (kind, v) {
            ("b1", Some(v)) => bytes.push(v.as_bool().unwrap() as u8),
            ("i8", Some(v)) => bytes.extend(v.to_string().parse::<i64>().unwrap().to_le_bytes()),
            ("f8", Some(v)) => bytes.extend(v.to_string().parse::<f64>().unwrap().to_le_bytes()),
            ("f8", None) => bytes.extend([0, 0, 0, 0, 0, 0, 0xf8, 0x7f]),
            ("text", Some(v)) => {
                bytes.push(1);
                bytes.extend(v.as_str().unwrap().as_bytes());
                bytes.push(0);
            }
            ("text", None) => bytes.push(0),
            _ => unreachable!(),
        }
    }
    let values: Vec<Value> = vs
        .iter()
        .map(|v| v.cloned().unwrap_or(Value::Null))
        .collect();
    Some(json!({"name": key, "kind": kind, "values": values, "sha256": sha(&bytes)}))
}

// ---- the event choice (§6.1) ----

#[derive(Clone)]
enum Choice {
    All,
    First(u64),
    Random(u64, u64),
}

fn splitmix_choice(total: u64, n: u64, seed: u64) -> Vec<u64> {
    let mut state = seed;
    let mut draw = || {
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut needed, mut out, mut i) = (n.min(total), Vec::new(), 0);
    while needed > 0 {
        if ((total - i) as f64) * draw() < needed as f64 {
            out.push(i);
            needed -= 1;
        }
        i += 1;
    }
    out
}

impl Choice {
    fn json(&self) -> Value {
        match *self {
            Choice::All => json!({"choose": "all"}),
            Choice::First(n) => json!({"choose": "first", "n": n}),
            Choice::Random(n, s) => {
                json!({"choose": "random", "n": n, "seed": s, "algorithm": "splitmix64-selection-sampling"})
            }
        }
    }

    /// The entries chosen of each file, in the files' order.
    fn chosen(&self, entries: &[u64]) -> Vec<Vec<u64>> {
        match *self {
            Choice::All => entries.iter().map(|&e| (0..e).collect()).collect(),
            Choice::First(n) => {
                let mut left = n;
                entries
                    .iter()
                    .map(|&e| {
                        let r = e.min(left);
                        left -= r;
                        (0..r).collect()
                    })
                    .collect()
            }
            Choice::Random(n, seed) => {
                let picks = splitmix_choice(entries.iter().sum(), n, seed);
                let mut start = 0;
                entries
                    .iter()
                    .map(|&e| {
                        let mine = picks
                            .iter()
                            .filter(|&&p| p >= start && p < start + e)
                            .map(|p| p - start)
                            .collect();
                        start += e;
                        mine
                    })
                    .collect()
            }
        }
    }

    fn stop_take(&self, entries: u64, chosen: usize) -> Option<(&'static str, u64)> {
        match self {
            Choice::All => None,
            Choice::First(_) => (chosen < entries as usize).then_some(("stop", chosen as u64)),
            Choice::Random(..) => Some(("take", chosen as u64)),
        }
    }

    fn listed(&self, entries: u64, chosen: &[u64]) -> bool {
        !matches!(self, Choice::First(_)) || !chosen.is_empty() || entries == 0
    }
}

// ---- the study (§6, §6.3) ----

struct Hole {
    first: u64,
    last: u64,
    kind: &'static str,
    why: String,
}

struct File {
    at: String,
    entries: u64,
    holes: Vec<Hole>,
}

impl File {
    fn in_hole(&self, e: u64) -> bool {
        self.holes.iter().any(|h| h.first <= e && e < h.last)
    }
}

struct Sample {
    name: String,
    label: String,
    asked: Vec<String>,
    tree: String,
    metadata: Map<String, Value>,
    choice: Choice,
    press: Option<Choice>,
    files: Vec<File>,
}

fn sample(name: &str, tree: &str, files: &[(&str, u64)]) -> Sample {
    Sample {
        name: name.into(),
        label: name.into(),
        asked: files.iter().map(|f| f.0.to_string()).collect(),
        tree: tree.into(),
        metadata: Map::new(),
        choice: Choice::All,
        press: None,
        files: files
            .iter()
            .map(|&(at, entries)| File {
                at: at.into(),
                entries,
                holes: vec![],
            })
            .collect(),
    }
}

impl Sample {
    fn label(mut self, label: &str) -> Self {
        self.label = label.into();
        self
    }
    fn meta(mut self, m: Value) -> Self {
        self.metadata = m.as_object().unwrap().clone();
        self
    }
    fn choice(mut self, c: Choice) -> Self {
        self.choice = c;
        self
    }
    fn press(mut self, c: Choice) -> Self {
        self.press = Some(c);
        self
    }
    fn asked(mut self, asked: &[&str]) -> Self {
        self.asked = asked.iter().map(|a| a.to_string()).collect();
        self
    }
    fn hole(mut self, file: usize, first: u64, last: u64, kind: &'static str, why: &str) -> Self {
        self.files[file].holes.push(Hole {
            first,
            last,
            kind,
            why: why.into(),
        });
        self
    }
    fn pressed(&self) -> &Choice {
        self.press.as_ref().unwrap_or(&self.choice)
    }
    fn entries(&self) -> Vec<u64> {
        self.files.iter().map(|f| f.entries).collect()
    }
}

#[derive(Clone, Copy)]
struct Row {
    sample: usize,
    file: usize,
    entry: u64,
}

fn br(name: &str, kind: &str, shape: &str) -> Value {
    json!({"name": name, "kind": kind, "shape": shape})
}

/// A total: its name, and its value at each entry read.
type Total = (&'static str, fn(Row) -> f64);

struct Study {
    study: String,
    samples_toml: String,
    recipe: String,
    title: Option<String>,
    commit: String,
    branches: Vec<Value>,
    samples: Vec<Sample>,
    keep: fn(Row) -> bool,
    totals: Vec<Total>,
}

fn study(
    name: &str,
    samples_toml: &str,
    recipe: &str,
    branches: Vec<Value>,
    samples: Vec<Sample>,
) -> Study {
    Study {
        study: name.into(),
        samples_toml: samples_toml.into(),
        recipe: recipe.into(),
        title: None,
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        branches,
        samples,
        keep: |_| true,
        totals: vec![],
    }
}

fn uuid(at: &str) -> String {
    let h = sha(at.as_bytes());
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn toml_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml(v: &Value) -> String {
    match v {
        Value::String(s) => toml_string(s),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => format!("[{}]", a.iter().map(toml).collect::<Vec<_>>().join(", ")),
        Value::Object(m) if m.is_empty() => "{}".into(),
        Value::Object(m) => {
            let key = |k: &str| {
                if k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    k.to_string()
                } else {
                    toml_string(k)
                }
            };
            format!(
                "{{ {} }}",
                m.iter()
                    .map(|(k, v)| format!("{} = {}", key(k), toml(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Null => panic!("TOML has no null"),
    }
}

impl Study {
    fn keep(mut self, keep: fn(Row) -> bool) -> Self {
        self.keep = keep;
        self
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (si, s) in self.samples.iter().enumerate() {
            let chosen = s.pressed().chosen(&s.entries());
            for (fi, f) in s.files.iter().enumerate() {
                for &entry in &chosen[fi] {
                    let row = Row {
                        sample: si,
                        file: fi,
                        entry,
                    };
                    if !f.in_hole(entry) && (self.keep)(row) {
                        rows.push(row);
                    }
                }
            }
        }
        rows
    }

    fn branch_list(&self) -> (Vec<Value>, String) {
        let mut list = self.branches.clone();
        list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let digest = sha(canon(&Value::Array(list.clone())).as_bytes());
        (list, digest)
    }

    fn lock(&self) -> String {
        let (list, list_sha) = self.branch_list();
        let mut t = String::from(
            "# samples.lock: written by rowver's independent test writer.\n\n\
             format = \"rvr-samples-lock\"\nversion = 1\nresolved = \"2026-11-02T11:59:00Z\"\n",
        );
        t += &format!("samples_toml_sha256 = \"{}\"\n", sha(self.samples_toml.as_bytes()));
        for s in &self.samples {
            let entries = s.entries();
            let chosen = s.choice.chosen(&entries);
            t += "\n[[sample]]\n";
            t += &format!("name = {}\nlabel = {}\n", toml_string(&s.name), toml_string(&s.label));
            t += &format!("asked = {}\ntree = {}\n", toml(&json!(s.asked)), toml_string(&s.tree));
            t += &format!("branches = \"{list_sha}\"\n");
            t += &match s.choice {
                Choice::All => "events = \"all\"\n".to_string(),
                Choice::First(n) => format!("events = {n}\n"),
                Choice::Random(n, seed) => {
                    format!("events = {n}\nchoose = \"random\"\nseed = {seed}\n")
                }
            };
            let read: usize = chosen.iter().map(Vec::len).sum();
            t += &format!("events_total = {}\nevents_read = {read}\n", entries.iter().sum::<u64>());
            t += &format!("metadata = {}\n", toml(&Value::Object(s.metadata.clone())));
            for (fi, f) in s.files.iter().enumerate() {
                t += &format!(
                    "\n[[sample.file]]\nat = {}\nbytes = {}\nadler32 = \"{}\"\nuuid = \"{}\"\nentries = {}\n",
                    toml_string(&f.at),
                    1000 + f.at.len(),
                    &sha(f.at.as_bytes())[32..40],
                    uuid(&f.at),
                    f.entries
                );
                if let Some((k, v)) = s.choice.stop_take(f.entries, chosen[fi].len()) {
                    t += &format!("{k} = {v}\n");
                }
            }
        }
        t += &format!("\n[[branches]]\nsha256 = \"{list_sha}\"\nlist = [\n");
        for b in &list {
            t += &format!("  {},\n", toml(b));
        }
        t + "]\n"
    }

    /// The header and the arrays of the file this study makes with these columns.
    fn header(&self, cols: &[Col], derived_cols: &[Derived]) -> (Value, Vec<u8>) {
        let rows = self.rows();
        let mut ranges = Vec::new();
        for si in 0..self.samples.len() {
            let start = ranges.last().map_or(0, |r: &(usize, usize)| r.1);
            ranges.push((
                start,
                start + rows.iter().filter(|r| r.sample == si).count(),
            ));
        }
        let mut store = Store {
            bytes: Vec::new(),
            shas: Vec::new(),
        };
        let cut = Cut { chunks: chunks_of(&ranges), samples: ranges.len() };
        let of = |v: &Vals| cut.chunks.iter().map(|&(_, a, b)| v.bytes(a, b).to_vec()).collect::<Vec<_>>();
        let sample_vals = vals(rows.iter().map(|r| r.sample as i32));
        let mut sample_ref = Map::new();
        store.put(&of(&sample_vals), 4, &cut, false, &mut sample_ref);
        let entry_vals = vals(rows.iter().map(|r| r.entry as i64));
        let mut entry_ref = Map::new();
        store.put(&of(&entry_vals), 8, &cut, true, &mut entry_ref);
        let mut col_records = Vec::new();
        for c in cols {
            assert_eq!(c.data_rows(), rows.len(), "column {} has the wrong number of rows", c.name);
            let mut m = Map::new();
            m.insert("name".into(), json!(c.name));
            record(&c.data, &cut, Some(&mut store), &mut m);
            col_records.push(Value::Object(m));
        }
        let derived_records: Vec<Value> = derived_cols
            .iter()
            .map(|d| {
                let mut m = Map::new();
                m.insert("name".into(), json!(d.name));
                m.insert("expression".into(), json!(d.expression));
                record(&d.data, &cut, None, &mut m);
                Value::Object(m)
            })
            .collect();

        let has_holes = self
            .samples
            .iter()
            .any(|s| s.files.iter().any(|f| !f.holes.is_empty()));
        let (mut data_samples, mut data_files, mut made_files, mut metas) =
            (vec![], vec![], Map::new(), vec![]);
        for (si, s) in self.samples.iter().enumerate() {
            let choice = s.pressed();
            let chosen = choice.chosen(&s.entries());
            let (mut files, mut read, mut missing) = (Map::new(), 0, 0);
            let mut sums = vec![0.0f64; self.totals.len()];
            for (fi, f) in s.files.iter().enumerate() {
                if !choice.listed(f.entries, &chosen[fi]) {
                    continue;
                }
                let got: Vec<u64> = chosen[fi]
                    .iter()
                    .copied()
                    .filter(|&e| !f.in_hole(e))
                    .collect();
                for h in &f.holes {
                    assert!(
                        chosen[fi].iter().any(|&e| h.first <= e && e < h.last),
                        "a hole holds no chosen entry"
                    );
                }
                read += got.len();
                missing += chosen[fi].len() - got.len();
                let n = rows
                    .iter()
                    .filter(|r| r.sample == si && r.file == fi)
                    .count();
                data_files.push(json!({"sample": s.name, "path": f.at, "rows": n}));
                let mut rec = Map::new();
                rec.insert("tree".into(), json!(s.tree));
                rec.insert("uuid".into(), json!(uuid(&f.at)));
                rec.insert("entries".into(), json!(f.entries));
                if let Some((k, v)) = choice.stop_take(f.entries, chosen[fi].len()) {
                    rec.insert(k.into(), json!(v));
                }
                if !f.holes.is_empty() {
                    let holes: Vec<Value> = f
                        .holes
                        .iter()
                        .map(
                            |h| json!({"entries": [h.first, h.last], "kind": h.kind, "why": h.why}),
                        )
                        .collect();
                    rec.insert("holes".into(), json!(holes));
                }
                let opened = !got.is_empty() || (f.entries == 0 && !matches!(choice, Choice::Random(..)));
                if !self.totals.is_empty() && opened {
                    let mut t = Map::new();
                    for (i, (name, value)) in self.totals.iter().enumerate() {
                        let mut sum = 0.0;
                        for &entry in &got {
                            sum += value(Row {
                                sample: si,
                                file: fi,
                                entry,
                            });
                        }
                        sums[i] += sum;
                        t.insert(name.to_string(), json!(sum));
                    }
                    rec.insert("totals".into(), Value::Object(t));
                }
                files.insert(f.at.clone(), Value::Object(rec));
            }
            let mut meta = s.metadata.clone();
            meta.insert("events_read".into(), json!(read));
            meta.insert(
                "events_total".into(),
                json!(s.entries().iter().sum::<u64>()),
            );
            if has_holes {
                meta.insert("events_missing".into(), json!(missing));
            }
            for (i, (name, _)) in self.totals.iter().enumerate() {
                meta.insert(name.to_string(), json!(sums[i]));
            }
            data_samples.push(
                json!({"name": s.name, "label": s.label, "rows": ranges[si].1 - ranges[si].0}),
            );
            made_files.insert(
                s.name.clone(),
                json!({"events": choice.json(), "files": files, "metadata": meta}),
            );
            metas.push(meta);
        }

        let names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
        let data = json!({
            "rows": rows.len(),
            "chunk_rows": R,
            "samples": data_samples,
            "files": data_files,
            "sample": sample_ref,
            "entry": entry_ref,
            "cols": col_records,
            "sha256": sha(store.shas.concat().as_bytes()),
            "sample_cols": sample_cols(&metas, &names),
        });
        let lock = self.lock();
        let mut made = Map::new();
        made.insert("study".into(), json!(self.study));
        if let Some(t) = &self.title {
            made.insert("title".into(), json!(t));
        }
        made.insert("built".into(), json!("2026-11-02T12:00:00Z"));
        made.insert(
            "by".into(),
            json!("rowver/tests/independent.rs, a second writer of .rvr files"),
        );
        made.insert(
            "toolchain".into(),
            json!({"program": "rvr", "version": "15.0.0", "rowver": "15", "commit": self.commit,
                   "compiler": "rustc 1.91.0 (f8297e351 2025-10-28)", "target": "aarch64-apple-darwin"}),
        );
        made.insert(
            "recipe".into(),
            json!({"sha256": sha(self.recipe.as_bytes()), "text": self.recipe, "format": "rvr-recipe", "columns": names}),
        );
        made.insert(
            "samples".into(),
            json!({"sha256": sha(self.samples_toml.as_bytes()), "text": self.samples_toml}),
        );
        made.insert(
            "lock".into(),
            json!({"sha256": sha(lock.as_bytes()), "text": lock}),
        );
        made.insert("files".into(), Value::Object(made_files));
        let made_sha = sha(canon(&Value::Object(made.clone())).as_bytes());
        made.insert("sha256".into(), json!(made_sha));

        let mut header = Map::new();
        header.insert("rowver".into(), json!(15));
        header.insert("data".into(), data);
        header.insert("made".into(), Value::Object(made));
        if !derived_records.is_empty() {
            header.insert("derived".into(), json!({"columns": derived_records}));
        }
        (Value::Object(header), store.bytes)
    }
}

impl Col {
    fn data_rows(&self) -> usize {
        match &self.data {
            Data::Flat(v) => v.nums.len(),
            Data::Jagged(off, _) => off.len() - 1,
        }
    }
}

// ---- the signature (§9) and the file (§1) ----

fn sign(header: &mut Value, key: &SigningKey) {
    let mut covered = Map::new();
    for k in ["rowver", "data", "made", "derived"] {
        if let Some(v) = header.get(k) {
            covered.insert(k.into(), v.clone());
        }
    }
    let digest = Sha256::digest(canon(&Value::Object(covered)).as_bytes());
    let signature = key.sign(digest.as_slice());
    header["signature"] = json!({
        "ed25519": hex(&signature.to_bytes()),
        "key": hex(&key.verifying_key().to_bytes()),
        "sha256": hex(&digest),
    });
}

fn assemble(header_text: &str, arrays: &[u8]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend((header_text.len() as u32).to_le_bytes());
    out.extend(header_text.as_bytes());
    out.extend(arrays);
    out
}

fn file(header: &Value, arrays: &[u8]) -> Vec<u8> {
    assemble(&serde_json::to_string(header).unwrap(), arrays)
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

static COUNT: AtomicUsize = AtomicUsize::new(0);

/// What rowver's full validation says of these bytes, written to a temporary file.
fn validate(name: &str, bytes: &[u8]) -> Result<(), String> {
    let n = COUNT.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("rowver-independent-{}-{n}-{name}.rvr", std::process::id()));
    let temp = TempFile(path);
    std::fs::write(&temp.0, bytes).unwrap();
    rowver::rvr::validate_file(&temp.0)
        .map(|_| ())
        .map_err(|e| format!("rowver refused {name}: {e}"))
}

fn assert_valid(name: &str, bytes: &[u8]) {
    validate(name, bytes).unwrap_or_else(|e| panic!("{e}"));
}

// ---- the running example, tiny.rvr ----

const TINY_SAMPLES: &str = r#"format = "rvr-samples"
version = 1

[places]

[defaults]
tree = "analysis"

[eras]

[[sample]]
name = "signal"
label = "Signal"
files = "./signal.root"
metadata = { kind = "signal", xsec = 2.5 }

[[sample]]
name = "background"
label = "Background"
files = "./background.root"
metadata = { kind = "background" }
"#;

const TINY_RECIPE: &str = r#"format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["nJet", "Jet_pt"]
ht = "sum(Jet_pt)"

[totals]

[derived]
mean_jet_pt = "ht / nJet"
"#;

fn tiny() -> (Value, Vec<u8>) {
    let jets: [&[f32]; 5] = [
        &[65.3125, 19.96875, 16.421875],
        &[51.75, 49.3125, 31.90625, 21.078125],
        &[37.03125, 34.03125, 33.40625],
        &[],
        &[120.5],
    ];
    let study = study(
        "tiny",
        TINY_SAMPLES,
        TINY_RECIPE,
        vec![br("Jet_pt", "f4", "jagged"), br("nJet", "i4", "flat")],
        vec![
            sample("signal", "analysis", &[("./signal.root", 5)])
                .label("Signal")
                .meta(json!({"kind": "signal", "xsec": 2.5})),
            sample("background", "analysis", &[("./background.root", 5)])
                .label("Background")
                .meta(json!({"kind": "background"})),
        ],
    )
    .keep(|r| r.entry != 3);
    let rows = study.rows();
    let pts: Vec<Vec<f32>> = rows
        .iter()
        .map(|r| jets[r.entry as usize].to_vec())
        .collect();
    let njet: Vec<i32> = pts.iter().map(|l| l.len() as i32).collect();
    let ht: Vec<f32> = pts
        .iter()
        .map(|l| l.iter().fold(0.0f32, |a, b| a + b))
        .collect();
    let mean: Vec<f64> = ht
        .iter()
        .zip(&njet)
        .map(|(h, n)| *h as f64 / *n as f64)
        .collect();
    let cols = [
        flat("nJet", vals(njet)),
        jagged("Jet_pt", &pts),
        flat("ht", vals(ht)),
    ];
    study.header(
        &cols,
        &[derived("mean_jet_pt", "ht / nJet", Data::Flat(vals(mean)))],
    )
}

// FORMAT example, 1, 2, 7
#[test]
fn the_running_example_tiny_rvr_is_valid() {
    let (header, arrays) = tiny();
    assert_valid("tiny", &file(&header, &arrays));
}

// FORMAT example, 4, 5, 8, 10
#[test]
fn my_checksums_agree_with_the_ones_format_md_quotes_for_tiny_rvr() {
    let (h, _) = tiny();
    let (d, cols, sc) = (&h["data"], &h["data"]["cols"], &h["data"]["sample_cols"]);
    let mean = &h["derived"]["columns"][0];
    let quoted = [
        (&d["sample"]["sha256"], "0b8e189d"),
        (&d["entry"]["sha256"], "d618105e"),
        (&d["entry"]["by_sample"][0], "904543e2"),
        (&cols[0]["sha256"], "d9237950"),
        (&cols[0]["by_sample"][1], "84697797"),
        (&cols[1]["offsets"]["sha256"], "af72e3c9"),
        (&cols[1]["offsets"]["by_sample"][0], "439e0157"),
        (&cols[1]["offsets"]["by_sample"][1], "439e0157"),
        (&cols[1]["content"]["sha256"], "69fa98c7"),
        (&cols[1]["content"]["by_sample"][0], "bcda9316"),
        (&cols[2]["sha256"], "e5fc4c13"),
        (&d["sha256"], "d943535f"),
        (&mean["sha256"], "9e715cef"),
        (&mean["by_sample"][0], "124d1600"),
        (&sc[0]["sha256"], "023c71f9"),
        (&sc[1]["sha256"], "e98f4aa4"),
    ];
    let wrong: Vec<String> = quoted
        .iter()
        .filter(|(v, want)| !v.as_str().unwrap().starts_with(want))
        .map(|(v, want)| format!("{want} != {v}"))
        .collect();
    assert!(
        wrong.is_empty(),
        "checksums that differ from FORMAT.md: {wrong:?}"
    );
    assert_eq!(cols[2]["nice"], json!([100.0, 160.0]));
    assert_eq!(cols[0]["vals"], json!([1, 3, 4]));
    assert_eq!(mean["nice"], json!([33.0, 130.0]));
    assert_eq!(mean["lim"], json!([33.901041666666664, 120.5]));
}

// FORMAT 8
#[test]
fn canonical_json_follows_the_table_of_section_8() {
    for (written, canonical) in [
        (
            r#"{ "b": 1, "a": [true, null] }"#,
            r#"{"a":[true,null],"b":1}"#,
        ),
        ("18446744073709551615", "18446744073709551615"),
        ("-0", "0"),
        ("1.5e2", "150.0"),
        ("0.00010", "0.0001"),
        ("0.00001", "1e-05"),
        ("1234567890123456.0", "1234567890123456.0"),
        ("15000000000000000.0", "1.5e+16"),
    ] {
        assert_eq!(canon(&num(written)), canonical);
    }
}

// ---- the tests ----

const NO_DEFAULTS: &str =
    "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n";

fn recipe(keep: &str, columns: &str, totals: &str, derived: &str) -> String {
    format!(
        "format = \"rvr-recipe\"\nversion = 1\n\n[define]\n\n[keep]\n{keep}\n[columns]\n{columns}\n[totals]\n{totals}\n[derived]\n{derived}"
    )
}

// FORMAT 4, 6
#[test]
fn a_file_with_a_column_of_every_flat_kind_is_valid() {
    let kinds = [
        ("v_f8", "f8"),
        ("v_f4", "f4"),
        ("v_i8", "i8"),
        ("v_i4", "i4"),
        ("v_i2", "i2"),
        ("v_i1", "i1"),
    ];
    let kinds = kinds.into_iter().chain([
        ("v_u8", "u8"),
        ("v_u4", "u4"),
        ("v_u2", "u2"),
        ("v_u1", "u1"),
        ("v_b1", "b1"),
    ]);
    let kinds: Vec<_> = kinds.collect();
    let samples_toml =
        format!("{NO_DEFAULTS}\n[[sample]]\nname = \"kinds\"\nfiles = \"./kinds.root\"\ntree = \"Events\"\n");
    let names: Vec<String> = kinds.iter().map(|k| format!("\"{}\"", k.0)).collect();
    let recipe = format!(
        "format = \"rvr-recipe\"\nversion = 1\ntitle = \"every flat kind\"\n\n[define]\n\n[keep]\n\n[columns]\nbranches = [{}]\n\n[totals]\n\n[derived]\n",
        names.join(", ")
    );
    let mut s = study(
        "kinds",
        &samples_toml,
        &recipe,
        kinds.iter().map(|k| br(k.0, k.1, "flat")).collect(),
        vec![sample("kinds", "Events", &[("./kinds.root", 70)])],
    );
    s.title = Some("every flat kind".into());
    s.commit = "unknown".into();
    let e: Vec<u64> = s.rows().iter().map(|r| r.entry).collect();
    let f8 = |i: u64| match i {
        0 => -0.037,
        1 => 373.67,
        i => i as f64 * 1.5,
    };
    let cols = [
        flat("v_f8", vals(e.iter().map(|&i| f8(i)))),
        flat("v_f4", vals(e.iter().map(|&i| 1.1f32 + i as f32 * 0.25))),
        flat("v_i8", vals(e.iter().map(|&i| i as i64 * 1000 - 35000))),
        flat("v_i4", vals(e.iter().map(|&i| (i % 5) as i32 - 2))),
        flat("v_i2", vals(e.iter().map(|&i| (i % 64) as i16 * -3))),
        flat("v_i1", vals(e.iter().map(|&i| (i % 3) as i8 - 1))),
        flat("v_u8", vals(e.iter().map(|&i| i * 1_000_000_007))),
        flat("v_u4", vals(e.iter().map(|_| 7u32))),
        flat("v_u2", vals(e.iter().map(|&i| (i % 65) as u16))),
        flat("v_u1", vals(e.iter().map(|&i| 200 + (i % 10) as u8))),
        flat("v_b1", vals(e.iter().map(|&i| i % 3 == 0))),
    ];
    let (header, arrays) = s.header(&cols, &[]);
    let c = &header["data"]["cols"];
    assert_eq!(c[0]["nice"], json!([-0.037, 380.0]));
    assert_eq!(c[1]["nice"], json!([1.1, 19.0]));
    assert!(c[4]["vals"].as_array().unwrap().len() == 64 && c[8].get("vals").is_none());
    assert_eq!(c[7]["nice"], json!([7, 8]));
    assert_eq!(c[9]["nice"], json!([200, 210]));
    assert_valid("kinds", &file(&header, &arrays));
}

const TWO_SAMPLES: &str = r#"format = "rvr-samples"
version = 1

[places]

[defaults]
tree = "Events"
metadata = { lumi = 59.8 }

[eras]

[[sample]]
name = "full"
files = "./full.root"

[[sample]]
name = "none"
label = "Nothing kept"
files = "./none.root"
"#;

fn x_of(r: Row) -> f64 {
    if r.sample == 0 {
        [1.5, -2.0, 3.25, 0.5][r.entry as usize]
    } else {
        -1.0 - r.entry as f64
    }
}

fn pts_of(r: Row) -> Vec<f32> {
    let all: [&[f32]; 4] = [&[40.0, 12.5], &[], &[31.0], &[7.25, 99.0, 30.0]];
    all[r.entry as usize % 4].to_vec()
}

fn two_samples(keep: fn(Row) -> bool) -> (Value, Vec<u8>) {
    let recipe = recipe(
        "pos = \"x > 0\"\n",
        "branches = [\"x\", \"pts\"]\n",
        "",
        "twice = \"x * 2\"\nhard = \"pts > 30\"\n",
    );
    let mut s = study(
        "two",
        TWO_SAMPLES,
        &recipe,
        vec![br("x", "f8", "flat"), br("pts", "f4", "jagged")],
        vec![
            sample("full", "Events", &[("./full.root", 4)]).meta(json!({"lumi": 59.8})),
            sample("none", "Events", &[("./none.root", 3)])
                .label("Nothing kept")
                .meta(json!({"lumi": 59.8})),
        ],
    )
    .keep(keep);
    s.commit = "0123456789abcdef0123456789abcdef01234567-dirty-0123456789ab".into();
    let rows = s.rows();
    let x: Vec<f64> = rows.iter().map(|&r| x_of(r)).collect();
    let pts: Vec<Vec<f32>> = rows.iter().map(|&r| pts_of(r)).collect();
    let hard: Vec<Vec<bool>> = pts
        .iter()
        .map(|l| l.iter().map(|&p| p > 30.0).collect())
        .collect();
    let cols = [flat("x", vals(x.clone())), jagged("pts", &pts)];
    let derived = [
        derived("twice", "x * 2", Data::Flat(vals(x.iter().map(|v| v * 2.0)))),
        derived("hard", "pts > 30", lists(&hard)),
    ];
    s.header(&cols, &derived)
}

// FORMAT 3, 6, 8, 10
#[test]
fn two_samples_one_with_no_rows_is_valid() {
    let (header, arrays) = two_samples(|r| x_of(r) > 0.0);
    assert_eq!(header["data"]["samples"][1]["rows"], json!(0));
    assert_valid("two", &file(&header, &arrays));
}

// FORMAT 3, 4, 8
#[test]
fn a_file_with_no_rows_at_all_is_valid() {
    let (header, arrays) = two_samples(|_| false);
    assert_eq!(header["data"]["cols"][0]["nice"], json!([0.0, 1.0]));
    assert_valid("norows", &file(&header, &arrays));
}

// SPEC 3.3.2, 3.7.3; FORMAT 5
#[test]
fn sample_columns_of_every_kind_are_valid() {
    let samples_toml = r#"format = "rvr-samples"
version = 1

[places]

[defaults]
tree = "Events"
metadata = { lumi = 59.8, is_data = false }

[eras]

[[sample]]
name = "data"
files = "./data.root"
metadata = { is_data = true, era = 3, kind = "data", tags = ["a", "b"], info = { run = 1 }, mixed = "x", flag = true, nJet = 7, nevents = 100, weight_scale = 1.0 }

[[sample]]
name = "ttbar"
files = "./ttbar.root"
metadata = { era = 3, kind = "background", xsec = 831.76, mixed = 2, flag = false, nJet = 8, weight_scale = 0.5 }

[[sample]]
name = "signal"
files = "./signal.root"
metadata = { era = 2, xsec = 0.386, tags = [], info = {}, nevents = 250, weight_scale = 1 }
"#;
    let recipe = recipe("", "branches = [\"nJet\"]\n", "", "");
    let s = study(
        "metadata",
        samples_toml,
        &recipe,
        vec![br("nJet", "i4", "flat")],
        vec![
            sample("data", "Events", &[("./data.root", 2)]).meta(json!({"lumi": 59.8, "is_data": true, "era": 3,
                "kind": "data", "tags": ["a", "b"], "info": {"run": 1}, "mixed": "x", "flag": true, "nJet": 7,
                "nevents": 100, "weight_scale": 1.0})),
            sample("ttbar", "Events", &[("./ttbar.root", 3)]).meta(json!({"lumi": 59.8, "is_data": false, "era": 3,
                "kind": "background", "xsec": 831.76, "mixed": 2, "flag": false, "nJet": 8, "weight_scale": 0.5})),
            sample("signal", "Events", &[("./signal.root", 1)]).meta(json!({"lumi": 59.8, "is_data": false, "era": 2,
                "xsec": 0.386, "tags": [], "info": {}, "nevents": 250, "weight_scale": 1})),
        ],
    );
    let n = s.rows().len();
    let (header, arrays) = s.header(&[flat("nJet", vals((0..n as i32).map(|i| i % 4)))], &[]);
    let names: Vec<&str> = header["data"]["sample_cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "lumi",
            "is_data",
            "era",
            "kind",
            "nevents",
            "weight_scale",
            "events_read",
            "events_total",
            "xsec"
        ]
    );
    assert_valid("metadata", &file(&header, &arrays));
}

// FORMAT 3, 6.1, 6.3
#[test]
fn a_first_n_choice_over_several_files_is_valid() {
    let samples_toml = format!(
        "{NO_DEFAULTS}\n[[sample]]\nname = \"parts\"\nfiles = \"./part_*.root\"\ntree = \"Events\"\nevents = 7\n\n\
         [[sample]]\nname = \"exact\"\nfiles = [\"./e1.root\", \"./e2.root\"]\ntree = \"Events\"\nevents = 3\n\n\
         [[sample]]\nname = \"more\"\nfiles = [\"./m1.root\", \"./m2.root\"]\ntree = \"Events\"\nevents = 100\n"
    );
    let recipe = recipe("odd = \"x != 1\"\n", "branches = [\"x\"]\n", "", "");
    let s = study(
        "first",
        &samples_toml,
        &recipe,
        vec![br("x", "i8", "flat")],
        vec![
            sample("parts", "Events", &[("./part_1.root", 5), ("./part_2.root", 5), ("./part_3.root", 4)])
            .asked(&["./part_*.root"])
            .choice(Choice::First(7)),
            sample("exact", "Events", &[("./e1.root", 3), ("./e2.root", 4)])
                .choice(Choice::First(3)),
            sample("more", "Events", &[("./m1.root", 2), ("./m2.root", 3)])
                .choice(Choice::First(100)),
        ],
    )
    .keep(|r| r.entry != 1);
    let x: Vec<i64> = s.rows().iter().map(|r| r.entry as i64).collect();
    let (header, arrays) = s.header(&[flat("x", vals(x))], &[]);
    assert_eq!(header["data"]["files"].as_array().unwrap().len(), 5);
    assert_valid("first", &file(&header, &arrays));
}

fn random_study(press: Option<Choice>) -> (Value, Vec<u8>) {
    let samples_toml = format!(
        "{NO_DEFAULTS}\n[[sample]]\nname = \"picked\"\nfiles = [\"./r1.root\", \"./r2.root\", \"./r3.root\"]\n\
         tree = \"Events\"\nevents = 4\nchoose = \"random\"\nseed = 9876543211\n\n\
         [[sample]]\nname = \"few\"\nfiles = [\"./f1.root\", \"./f2.root\"]\ntree = \"Events\"\nevents = 50\nchoose = \"random\"\n"
    );
    let recipe = recipe("", "branches = [\"x\"]\n", "", "");
    let mut picked = sample("picked", "Events", &[("./r1.root", 6), ("./r2.root", 1), ("./r3.root", 6)])
    .choice(Choice::Random(4, 9876543211));
    let mut few = sample("few", "Events", &[("./f1.root", 3), ("./f2.root", 2)])
        .choice(Choice::Random(50, 1));
    if let Some(c) = press {
        picked = picked.press(c.clone());
        few = few.press(c);
    }
    let s = study("random", &samples_toml, &recipe, vec![br("x", "f4", "flat")], vec![picked, few]);
    let x: Vec<f32> = s.rows().iter().map(|r| r.entry as f32 / 4.0).collect();
    s.header(&[flat("x", vals(x))], &[])
}

// SPEC 3.5.9; FORMAT 6.1, 6.3
#[test]
fn a_random_choice_is_valid() {
    let (header, arrays) = random_study(None);
    let takes: Vec<&Value> = header["made"]["files"]["picked"]["files"]
        .as_object()
        .unwrap()
        .values()
        .map(|f| &f["take"])
        .collect();
    assert_eq!(takes, [&json!(3), &json!(0), &json!(1)]);
    assert_valid("random", &file(&header, &arrays));
}

// FORMAT 1, 6.1, 8
#[test]
fn a_random_choice_from_the_largest_seed_is_valid() {
    let (header, arrays) = random_study(Some(Choice::Random(4, u64::MAX)));
    assert_valid("seed", &file(&header, &arrays));
}

// FORMAT 6, 6.1
#[test]
fn a_press_that_changed_the_event_choice_is_valid() {
    let samples_toml = format!(
        "{NO_DEFAULTS}\n[[sample]]\nname = \"a\"\nfiles = [\"./a1.root\", \"./a2.root\"]\ntree = \"Events\"\n\n\
         [[sample]]\nname = \"b\"\nfiles = \"./b1.root\"\ntree = \"Events\"\n"
    );
    let recipe = recipe("", "branches = [\"x\"]\n", "", "");
    let s = study(
        "events",
        &samples_toml,
        &recipe,
        vec![br("x", "u2", "flat")],
        vec![
            sample("a", "Events", &[("./a1.root", 2), ("./a2.root", 4)]).press(Choice::First(3)),
            sample("b", "Events", &[("./b1.root", 5)]).press(Choice::First(3)),
        ],
    );
    let x: Vec<u16> = s.rows().iter().map(|r| r.entry as u16).collect();
    let (header, arrays) = s.header(&[flat("x", vals(x))], &[]);
    assert_valid("events", &file(&header, &arrays));
}

fn holes_study(why: &str) -> (Value, Vec<u8>) {
    let samples_toml = r#"format = "rvr-samples"
version = 1

[places]

[defaults]
tree = "Events"

[eras]

[[sample]]
name = "a"
files = ["./a1.root", "./a2.root"]

[[sample]]
name = "b"
files = "./b1.root"
events = 5

[[sample]]
name = "c"
files = ["./c1.root", "./c2.root"]
events = 6
choose = "random"

[[sample]]
name = "d"
files = "./d1.root"
"#;
    let recipe = recipe("", "branches = [\"x\"]\n", "", "");
    let s = study(
        "holes",
        samples_toml,
        &recipe,
        vec![br("x", "f8", "flat")],
        vec![
            sample("a", "Events", &[("./a1.root", 10), ("./a2.root", 4)])
                .hole(0, 3, 6, "damaged", why)
                .hole(0, 6, 7, "damaged", "basket 3 of branch x (entries 6 to 6) does not decompress: read the file again")
                .hole(1, 0, 4, "gone", "./a2.root is not there any more: resolve the study again"),
            sample("b", "Events", &[("./b1.root", 8)])
                .choice(Choice::First(5))
                .hole(0, 2, 5, "unreachable", "the disk kept failing after every wait: press --fill later"),
            sample("c", "Events", &[("./c1.root", 20), ("./c2.root", 5)])
                .choice(Choice::Random(6, 1))
                .hole(0, 10, 20, "refused", "its storage refuses the file with a valid grid proxy: ask its admins")
                .hole(1, 0, 5, "changed", "its size differs from the lock: resolve again and review the lock"),
            sample("d", "Events", &[("./d1.root", 3)]),
        ],
    );
    let x: Vec<f64> = s.rows().iter().map(|r| r.entry as f64).collect();
    s.header(&[flat("x", vals(x))], &[])
}

// FORMAT 5, 6.2
#[test]
fn a_file_with_holes_is_valid() {
    let (header, arrays) =
        holes_study("basket 1 of branch x (entries 3 to 5) does not decompress: it may be damaged; press --fill");
    let m = &header["made"]["files"];
    let counts = |s: &str| {
        (
            m[s]["metadata"]["events_read"].clone(),
            m[s]["metadata"]["events_missing"].clone(),
        )
    };
    assert_eq!(counts("a"), (json!(6), json!(8)));
    assert_eq!(counts("b"), (json!(2), json!(3)));
    assert_eq!(counts("c"), (json!(1), json!(5)));
    assert_eq!(counts("d"), (json!(3), json!(0)));
    assert_valid("holes", &file(&header, &arrays));
}

// FORMAT 8
#[test]
fn texts_that_need_escapes_in_canonical_json_are_valid() {
    let (header, arrays) = holes_study("a \"quoted\" \\ path, a tab\there, an escape \u{1b}[0m and \u{1}\u{7f} ü ∑ 🙂");
    assert_valid("escapes", &file(&header, &arrays));
}

const GENWEIGHT: [[f64; 4]; 3] = [
    [0.5, -0.25, 1.5, 0.75],
    [2.0, 0.125, -1.0, 0.0],
    [1.25, 9.0, 0.5, 0.0],
];

fn genweight(r: Row) -> f64 {
    GENWEIGHT[if r.sample == 0 { r.file } else { 2 + r.file }][r.entry as usize]
}

// SPEC 4.5.1, 4.5.3, 4.6.2; FORMAT 5, 6, 10
#[test]
fn a_file_with_totals_is_valid() {
    let samples_toml = format!(
        "{NO_DEFAULTS}\n[[sample]]\nname = \"a\"\nfiles = [\"./a1.root\", \"./a2.root\"]\ntree = \"Events\"\n\n\
         [[sample]]\nname = \"b\"\nfiles = [\"./b1.root\", \"./b2.root\"]\ntree = \"Events\"\nevents = 2\nchoose = \"random\"\nseed = 5\n"
    );
    let recipe = recipe(
        "positive = \"genWeight > 0\"\n",
        "branches = [\"genWeight\"]\n",
        "sumw = \"genWeight\"\nsumw2 = \"genWeight * genWeight\"\n",
        "weight = \"genWeight / sample.sumw\"\n",
    );
    let mut s = study(
        "totals",
        &samples_toml,
        &recipe,
        vec![br("genWeight", "f8", "flat")],
        vec![
            sample("a", "Events", &[("./a1.root", 4), ("./a2.root", 3)]),
            sample("b", "Events", &[("./b1.root", 3), ("./b2.root", 1)])
                .choice(Choice::Random(2, 5)),
        ],
    )
    .keep(|r| genweight(r) > 0.0);
    s.totals = vec![
        ("sumw", genweight),
        ("sumw2", |r| genweight(r) * genweight(r)),
    ];
    let rows = s.rows();
    let w: Vec<f64> = rows.iter().map(|&r| genweight(r)).collect();
    let sumw = [3.625, 1.75];
    let weight: Vec<f64> = rows
        .iter()
        .map(|&r| genweight(r) / sumw[r.sample])
        .collect();
    let (header, arrays) = s.header(
        &[flat("genWeight", vals(w))],
        &[derived("weight", "genWeight / sample.sumw", Data::Flat(vals(weight)))],
    );
    let m = &header["made"]["files"];
    assert_eq!(m["a"]["metadata"]["sumw"], json!(3.625));
    assert_eq!(m["b"]["metadata"]["sumw2"], json!(1.8125));
    assert!(m["b"]["files"]["./b2.root"].get("totals").is_none());
    assert_valid("totals", &file(&header, &arrays));
}

// SPEC 2.3, 4.5.2; FORMAT 6
#[test]
fn a_total_over_another_tree_is_valid() {
    let samples_toml = format!("{NO_DEFAULTS}\n[[sample]]\nname = \"a\"\nfiles = \"./a1.root\"\ntree = \"Events\"\n");
    let recipe = recipe(
        "",
        "branches = [\"genWeight\"]\n",
        "sumw = { value = \"genEventSumw\", tree = \"Runs\" }\n",
        "",
    );
    let mut s = study(
        "runs",
        &samples_toml,
        &recipe,
        vec![br("genWeight", "f8", "flat")],
        vec![sample("a", "Events", &[("./a1.root", 4)])],
    );
    s.totals = vec![("sumw", |r| if r.entry == 0 { 1234.5 } else { 0.0 })];
    let w: Vec<f64> = s.rows().iter().map(|&r| genweight(r)).collect();
    let (header, arrays) = s.header(&[flat("genWeight", vals(w))], &[]);
    assert_valid("runs", &file(&header, &arrays));
}

// FORMAT 9, 11
#[test]
fn a_signed_file_is_valid() {
    let (mut header, arrays) = tiny();
    sign(&mut header, &SigningKey::from_bytes(&[7u8; 32]));
    header["viewer"] = json!({"zoom": 2});
    assert_valid("signed", &file(&header, &arrays));
}

// FORMAT 1, 11
#[test]
fn a_file_with_tool_keys_is_valid() {
    let (mut header, arrays) = tiny();
    header["my-tool.v2"] = json!({"anything": [1, 2.5, "x", null, true, {"deep": [[], {}]}]});
    header["A_b-9.z"] = json!({});
    assert_valid("tools", &file(&header, &arrays));
}

/// A study of one sample `a`, of one file `./a.root` of `entries` entries, its tree's branches
/// `branches`, its recipe storing the first.
fn one_file(name: &str, branches: Vec<Value>, entries: u64) -> Study {
    let samples_toml = format!("{NO_DEFAULTS}\n[[sample]]\nname = \"a\"\nfiles = \"./a.root\"\ntree = \"Events\"\n");
    let recipe = recipe("", &format!("branches = [{}]\n", branches[0]["name"]), "", "");
    study(name, &samples_toml, &recipe, branches, vec![sample("a", "Events", &[("./a.root", entries)])])
}

// FORMAT 1, 12
#[test]
fn an_empty_file_is_valid() {
    let s = one_file("empty", vec![br("x", "i2", "flat")], 3);
    let (header, _) = s.header(&[flat("x", vals([5i16, -5, 0]))], &[]);
    assert_valid("empty", &file(&header, &[]));
}

// FORMAT 1, 10, 12
#[test]
fn an_empty_file_with_derived_columns_is_valid() {
    let (header, _) = tiny();
    assert_valid("empty-derived", &file(&header, &[]));
}

// FORMAT 1
#[test]
fn a_header_with_whitespace_between_its_tokens_is_valid() {
    let (header, arrays) = tiny();
    let mut text = b"\n\t ".to_vec();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut text,
        serde_json::ser::PrettyFormatter::with_indent(b"\t"),
    );
    serde::Serialize::serialize(&header, &mut ser).unwrap();
    text.extend(b"\r\n ");
    assert_valid(
        "whitespace",
        &assemble(std::str::from_utf8(&text).unwrap(), &arrays),
    );
}

// FORMAT 4
#[test]
fn a_stored_column_with_facts_in_another_json_spelling_is_valid() {
    let (mut header, arrays) = tiny();
    header["data"]["cols"][2]["nice"] = json!([num("1e2"), num("16E1")]);
    header["data"]["cols"][2]["lim"] = json!([num("101.7031250"), num("1.54046875e+2")]);
    assert_valid("spelling", &file(&header, &arrays));
}

// FORMAT 4, 10
#[test]
fn a_derived_column_with_facts_in_another_json_spelling_is_valid() {
    let (mut header, arrays) = tiny();
    header["derived"]["columns"][0]["nice"] = json!([num("33"), num("1.3e2")]);
    assert_valid("derived-spelling", &file(&header, &arrays));
}

// SPEC 2.3, 3.2.2, 3.2.3, 3.4.2, 3.5.5, 3.5.6, 3.6.1, 3.6.3, 3.7.1, 3.7.2, 3.7.3; FORMAT 6.3
#[test]
fn a_study_with_places_eras_and_trees_is_valid() {
    let samples_toml = r#"format = "rvr-samples"
version = 1

[places]
here = "./ntuples/"

[defaults]
metadata = { lumi = 59.8, kind = "background" }

[eras.A]
era = 1

[eras.B]
era = 2

[[sample]]
name = "scan"
label = "Scan"
files = "{here}/scan_{era}.root"
trees = { m300 = "t300", m400 = "t400" }
eras = ["A", "B"]
metadata = { kind = "signal", source = "{file}" }

[[sample]]
name = "bkg"
files = ["{here}/bkg_1.root", "{here}/bkg_2.root"]
tree = "Events"
"#;
    let recipe = recipe("", "branches = [\"x\"]\n", "", "");
    let scan = |era: &str, n: u64, point: &str, tree: &str, entries: u64| {
        let at = format!("./ntuples/scan_{era}.root");
        sample(&format!("scan_{point}_{era}"), tree, &[(at.as_str(), entries)])
        .label(&format!("Scan {point} ({era})"))
        .meta(
            json!({"era": n, "lumi": 59.8, "kind": "signal", "source": format!("scan_{era}.root")}),
        )
    };
    let s = study(
        "scan",
        samples_toml,
        &recipe,
        vec![br("x", "f8", "flat")],
        vec![
            scan("A", 1, "m300", "t300", 4),
            scan("A", 1, "m400", "t400", 3),
            scan("B", 2, "m300", "t300", 2),
            scan("B", 2, "m400", "t400", 5),
            sample("bkg", "Events", &[("./ntuples/bkg_1.root", 2), ("./ntuples/bkg_2.root", 2)])
            .asked(&["./ntuples/bkg_1.root", "./ntuples/bkg_2.root"])
            .meta(json!({"lumi": 59.8, "kind": "background"})),
        ],
    );
    let x: Vec<f64> = s
        .rows()
        .iter()
        .map(|r| r.sample as f64 + r.entry as f64 / 8.0)
        .collect();
    let (header, arrays) = s.header(&[flat("x", vals(x))], &[]);
    assert_valid("scan", &file(&header, &arrays));
}

fn one_column_study(name: &str, kind: &'static str, col: Vals) -> (Value, Vec<u8>) {
    let s = one_file(name, vec![br("x", kind, "flat")], col.nums.len() as u64);
    s.header(&[flat("x", col)], &[])
}

// FORMAT 4
#[test]
fn the_nice_examples_of_section_4_are_valid() {
    let cases: Vec<(&'static str, Vals, Value)> = vec![
        ("f8", vals([5.0f64, 5.0]), json!([5.0, 5.1])),
        ("f8", vals([-1.2f64, -1.2]), json!([-1.2, -1.1])),
        ("f8", vals([0.0f64, 0.0]), json!([0.0, 1.0])),
        ("i4", vals([5i32, 5]), json!([5, 6])),
        ("f8", vals([0.3381, 373.67]), json!([0.33, 380.0])),
        ("f4", vals([1.1f32, 1.1]), json!([1.1, 1.2])),
    ];
    for (i, (kind, v, want)) in cases.into_iter().enumerate() {
        let (header, arrays) = one_column_study("nice", kind, v);
        assert_eq!(header["data"]["cols"][0]["nice"], want, "case {i}");
        assert_valid(&format!("nice-{i}"), &file(&header, &arrays));
    }
}

// SPEC 2.6; FORMAT 4
#[test]
fn a_flat_float_column_with_nan_and_infinities_is_valid() {
    let cases = [
        vals([-0.037, f64::NAN, f64::INFINITY, 373.67, f64::NEG_INFINITY]),
        vals([f64::NAN, f64::NAN]),
        vals([f64::INFINITY, f64::NEG_INFINITY]),
    ];
    let mut refused = Vec::new();
    for (i, v) in cases.into_iter().enumerate() {
        let (header, arrays) = one_column_study("nan", "f8", v);
        let want = if i == 0 {
            json!([-0.037, 380.0])
        } else {
            json!([0.0, 1.0])
        };
        assert_eq!(header["data"]["cols"][0]["nice"], want);
        refused.extend(validate(&format!("nan-{i}"), &file(&header, &arrays)).err());
    }
    assert!(refused.is_empty(), "{refused:#?}");
}

// SPEC 2.6; FORMAT 4
#[test]
fn a_jagged_float_column_with_nan_and_infinities_is_valid() {
    let s = one_file("nanj", vec![br("pts", "f4", "jagged")], 2);
    let (header, arrays) = s.header(&[jagged("pts", &[vec![1.0f32, f32::NAN], vec![f32::INFINITY]])], &[]);
    assert_valid("nanj", &file(&header, &arrays));
}

// FORMAT 7, 8, 12
#[test]
fn a_changed_array_byte_is_refused_so_these_checks_mean_something() {
    let (header, arrays) = tiny();
    let mut bytes = file(&header, &arrays);
    let n = bytes.len();
    bytes[n - 10] ^= 1;
    assert!(validate("refused", &bytes).is_err());
}

// FORMAT 4
#[test]
fn a_b1_column_of_one_value_is_valid() {
    let (header, arrays) = one_column_study("b1", "b1", vals([true, true, true]));
    assert_eq!(header["data"]["cols"][0]["lim"], json!([1, 1]));
    assert_eq!(header["data"]["cols"][0]["nice"], json!([1, 2]));
    assert_valid("b1", &file(&header, &arrays));
}

// FORMAT 4
#[test]
fn whole_number_facts_beyond_64_bits_are_valid() {
    let (header, arrays) = one_column_study("u8", "u8", vals([0, u64::MAX]));
    assert_eq!(header["data"]["cols"][0]["nice"], json!([0, num("19000000000000000000")]));
    assert_valid("u8", &file(&header, &arrays));
    let (header, arrays) = one_column_study("i8", "i8", vals([i64::MIN, i64::MAX]));
    assert_eq!(header["data"]["cols"][0]["nice"], json!([num("-9300000000000000000"), num("9300000000000000000")]));
    assert_valid("i8", &file(&header, &arrays));
}

// FORMAT 4
#[test]
fn a_float_column_holding_the_largest_double_is_valid() {
    let (header, arrays) = one_column_study("max", "f8", vals([0.0, f64::MAX]));
    assert_valid("max", &file(&header, &arrays));
}

// FORMAT 6.3
#[test]
fn a_lock_with_every_branch_shape_is_valid() {
    let unreadable = json!({"name": "clones", "kind": "none", "shape": "flat", "why": "a TClonesArray, which rvr cannot read"});
    let branches = vec![
        br("x", "f8", "flat"),
        json!({"name": "xyz", "kind": "f4", "shape": 3}),
        br("tracks", "f4", "jagged 2"),
        br("Muon_tightId", "b1", "jagged"),
        unreadable,
    ];
    let s = one_file("shapes", branches, 2);
    let (header, arrays) = s.header(&[flat("x", vals([1.0f64, 2.0]))], &[]);
    assert_valid("shapes", &file(&header, &arrays));
}

// FORMAT 6.1
#[test]
fn a_sample_of_two_to_the_forty_entries_is_valid() {
    let samples_toml =
        format!("{NO_DEFAULTS}\n[[sample]]\nname = \"big\"\nfiles = \"./big.root\"\ntree = \"Events\"\nevents = 3\n");
    let recipe = recipe("", "branches = [\"x\"]\n", "", "");
    let s = study(
        "big",
        &samples_toml,
        &recipe,
        vec![br("x", "u1", "flat")],
        vec![sample("big", "Events", &[("./big.root", 1 << 40)]).choice(Choice::First(3))],
    );
    let (header, arrays) = s.header(&[flat("x", vals([1u8, 2, 3]))], &[]);
    assert_valid("big", &file(&header, &arrays));
}
