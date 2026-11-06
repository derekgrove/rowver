//! The `rehashed` target: its input made into a `.rvr` file whose every checksum is right, so what
//! lies behind the checksums is fuzzed (offsets against content, chunks against samples, kinds,
//! facts, derived records), then checked in full; a file full validation accepts must read whole.
//!
//! The input is a file unpacked: a control byte, the header's length (4 bytes, little-endian) and
//! JSON, then every chunk's byte lanes, each its length (4 bytes) and its bytes, array by array in
//! record order, chunk by chunk, lane by lane; a lane past the input's end is empty. The control
//! byte's bits: 1, each lane cut or padded with zeros to its chunk's values; 2, each list column's
//! content.count and data.rows made to agree with the arrays and samples; 4, each flat column's
//! facts made again from its values; 8, the arrays let go (an empty file); 16, the arrays laid in
//! the file last record first (FORMAT §7 lets them lie in any order).
//!
//! Only the chunks and checksums are worked out apart from rowver: the test below holds that
//! working to rowver's, every seed made a file full validation accepts. A flat column's facts
//! (`rowver::write::derived`) and the derived records (`derived::records`) are rowver's own, so
//! this target cannot find a wrong fact or derived value; the C++ differential on its files does.

use rowver::rvr::hex;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The most chunks a file is made with, the most values a list column's chunk is padded to, and
/// the most bytes of lanes in all, each lane charged its bytes and `LANE` more (its frame and
/// table entry), so at most 2^18 lanes: past them an input is passed over, so the target's own
/// work stays well inside the time and memory a finding is judged by, and what it uses is the
/// reader's.
const MOST_CHUNKS: usize = 4096;
const MOST_VALUES: usize = 1 << 20;
const MOST_BYTES: usize = 16 << 20;
const LANE: usize = 64;

/// The eight bytes a file starts and ends with.
const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

/// The bytes of a kind's values; 1 for a word that is no kind (the file is refused for it).
fn width(kind: &str) -> usize {
    rvr_recipe::Kind::named(kind).map_or(1, rvr_recipe::Kind::width)
}

/// `value` put at `keys` under `at`, when every object on the way is there (and the last key is
/// added); a header that holds something else there is left as it is.
fn put(at: &mut Value, keys: &[&str], value: Value) {
    let Some((last, path)) = keys.split_last() else { return };
    let mut at = at;
    for key in path {
        match at.as_object_mut().and_then(|object| object.get_mut(*key)) {
            Some(next) => at = next,
            None => return,
        }
    }
    if let Some(object) = at.as_object_mut() {
        object.insert((*last).to_owned(), value);
    }
}

/// The lanes after the header, read one at a time.
struct Lanes<'a>(&'a [u8]);

impl<'a> Lanes<'a> {
    fn next(&mut self) -> &'a [u8] {
        if self.0.len() < 4 {
            self.0 = &[];
            return &[];
        }
        let n = (u32::from_le_bytes(self.0[..4].try_into().unwrap()) as usize).min(self.0.len() - 4);
        let lane = &self.0[4..4 + n];
        self.0 = &self.0[4 + n..];
        lane
    }
}

/// Each chunk's sample and rows, as a `data` section's samples cut every `every` rows make them
/// (none when it holds no list of samples); `None` past `MOST_CHUNKS`.
fn chunks(data: &Value, every: u64) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    for (sample, held) in data["samples"].as_array().into_iter().flatten().enumerate() {
        let mut rows = held["rows"].as_u64().unwrap_or(0);
        while rows > 0 {
            let n = rows.min(every);
            out.push((sample, usize::try_from(n).ok()?));
            rows -= n;
            if out.len() > MOST_CHUNKS {
                return None;
            }
        }
    }
    Some(out)
}

/// The JSON pointers of a header's array records in file order (a list column's offsets, then its
/// content), each with the kind of its values.
fn records(header: &Value) -> Vec<(String, String)> {
    let mut out = vec![("/data/sample".to_owned(), "i4".to_owned()), ("/data/entry".to_owned(), "i8".to_owned())];
    for (index, column) in header["data"]["cols"].as_array().into_iter().flatten().enumerate() {
        let kind = |held: &Value| held["kind"].as_str().unwrap_or("").to_owned();
        match column["kind"] == "jagged" {
            true => out.extend([(format!("/data/cols/{index}/offsets"), "i8".to_owned()), (format!("/data/cols/{index}/content"), kind(&column["content"]))]),
            false => out.push((format!("/data/cols/{index}"), kind(column))),
        }
    }
    out
}

/// One array packed: its bytes (frames, then chunk table), its chunks' SHA-256s, and their values.
struct Packed {
    bytes: Vec<u8>,
    hashes: Vec<[u8; 32]>,
    values: Vec<Vec<u8>>,
}

/// One array of values `w` bytes wide from `lanes`, `n(k)` values in chunk `k` when `padded`: each
/// lane a zstd frame, the chunk's SHA-256 that of its lanes put back together; `None` past `room`.
fn pack(lanes: &mut Lanes, w: usize, n: &dyn Fn(usize) -> Option<usize>, chunks: usize, padded: bool, room: &mut usize) -> Option<Packed> {
    let mut packed = Packed { bytes: Vec::new(), hashes: Vec::new(), values: Vec::new() };
    let mut table = Vec::new();
    for k in 0..chunks {
        let mut held: Vec<Vec<u8>> = (0..w).map(|_| lanes.next().to_vec()).collect();
        // The room is taken before a lane is padded: a header's rows alone may ask for gigabytes.
        let padding = n(k).filter(|_| padded);
        *room = room.checked_sub(match padding {
            Some(n) => n.checked_mul(w)?,
            None => held.iter().map(Vec::len).sum(),
        })?;
        *room = room.checked_sub(w * LANE)?;
        for lane in held.iter_mut().filter(|_| padding.is_some()) {
            lane.resize(padding.unwrap_or(0), 0);
        }
        let count = held.iter().map(Vec::len).min().unwrap_or(0);
        let mut values = vec![0; count * w];
        for (lane, bytes) in held.iter().enumerate() {
            for (at, byte) in bytes.iter().take(count).enumerate() {
                values[at * w + lane] = *byte;
            }
        }
        let hash: [u8; 32] = Sha256::digest(&values).into();
        for lane in &held {
            let frame = ruzstd::encoding::compress_to_vec(&lane[..], ruzstd::encoding::CompressionLevel::Fastest);
            table.extend((frame.len() as u64).to_le_bytes());
            packed.bytes.extend(frame);
        }
        table.extend(hash);
        packed.hashes.push(hash);
        packed.values.push(values);
    }
    packed.bytes.extend(table);
    Some(packed)
}

/// The SHA-256 of these SHA-256s' hex texts joined, as an array's `sha256` and `by_sample` are.
fn joined<'a>(hashes: impl IntoIterator<Item = &'a [u8; 32]>) -> String {
    let mut all = Sha256::new();
    for hash in hashes {
        all.update(hex(hash).as_bytes());
    }
    hex(&all.finalize())
}

/// The header and arrays of the file the input makes, its every checksum right but the derived
/// columns' (which `written` records from the file): `None` for an input whose header is not JSON
/// or that passes a bound. A `chunk_rows` past the format's bounds makes a file of no arrays, and
/// an array record that is not an object no array, for rowver to refuse.
pub fn fixed(data: &[u8]) -> Option<(Value, Vec<u8>)> {
    let control = *data.first()?;
    let length = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?) as usize;
    let text = data.get(5..5 + length.min(data.len().saturating_sub(5)))?;
    let mut header: Value = serde_json::from_slice(text).ok()?;
    header.as_object_mut()?.shift_remove("signature");
    let every = header["data"]["chunk_rows"].as_u64().unwrap_or(rvr_recipe::CHUNK_ROWS as u64);
    let (fewest, most) = rvr_recipe::CHUNK_BOUNDS;
    if !(fewest as u64..=most as u64).contains(&every) {
        return Some((header, Vec::new()));
    }
    let mut lanes = Lanes(data.get(5 + text.len()..).unwrap_or_default());
    let (padded, counted, faced, empty, reversed) = (control & 1 != 0, control & 2 != 0, control & 4 != 0, control & 8 != 0, control & 16 != 0);
    let cut = chunks(&header["data"], every)?;
    let samples = header["data"]["samples"].as_array().map_or(0, Vec::len);
    let mut room = MOST_BYTES;
    let mut arrays: Vec<u8> = Vec::new();
    let mut laid: Vec<(String, Vec<u8>)> = Vec::new();
    let mut members: Vec<usize> = Vec::new();
    for (pointer, kind) in records(&header) {
        if !header.pointer(&pointer).is_some_and(Value::is_object) {
            continue;
        }
        let w = width(&kind);
        let offsets = pointer.ends_with("/offsets");
        let content = pointer.ends_with("/content");
        let n = |k: usize| match (offsets, content) {
            (true, _) => cut[k].1.checked_add(1),
            (_, true) => members.get(k).copied().filter(|m| *m <= MOST_VALUES),
            _ => Some(cut[k].1),
        };
        let packed = pack(&mut lanes, w, &n, cut.len(), padded, &mut room)?;
        if offsets {
            // Each chunk's members: its last offset, as the reader takes it.
            members = packed.values.iter().map(|v| v.get(v.len().saturating_sub(8)..).and_then(|b| b.try_into().ok()).map_or(0, |b| i64::from_le_bytes(b).max(0) as usize)).collect();
        }
        let total = members.iter().try_fold(0_usize, |sum, m| sum.checked_add(*m))?;
        let Some(record) = header.pointer_mut(&pointer).and_then(Value::as_object_mut) else { continue };
        record.insert("at".into(), arrays.len().into());
        record.insert("size".into(), packed.bytes.len().into());
        record.insert("sha256".into(), joined(&packed.hashes).into());
        if record.contains_key("by_sample") {
            let mut by: Vec<Vec<&[u8; 32]>> = vec![Vec::new(); samples];
            for (chunk, hash) in cut.iter().zip(&packed.hashes) {
                by[chunk.0].push(hash);
            }
            let by_sample: Vec<String> = by.into_iter().map(joined).collect();
            record.insert("by_sample".into(), by_sample.into());
        }
        if counted && content {
            record.insert("count".into(), total.into());
        }
        if faced && !offsets && !content && pointer.starts_with("/data/cols/") {
            facts(record, &kind, &packed.values, &cut, samples, every as usize);
        }
        arrays.extend(&packed.bytes);
        laid.push((pointer.clone(), packed.bytes));
    }
    if reversed {
        // Laid last record first, each record's `at` where its array now lies.
        arrays.clear();
        for (pointer, bytes) in laid.iter().rev() {
            if let Some(record) = header.pointer_mut(pointer).and_then(Value::as_object_mut) {
                record.insert("at".into(), arrays.len().into());
            }
            arrays.extend(bytes);
        }
    }
    if counted {
        let rows = header["data"]["samples"].as_array().into_iter().flatten().try_fold(0_u64, |sum, s| sum.checked_add(s["rows"].as_u64().unwrap_or(0)))?;
        put(&mut header, &["data", "rows"], rows.into());
    }
    let mut all = String::new();
    for (pointer, _) in records(&header) {
        all.push_str(header.pointer(&pointer).and_then(|r| r["sha256"].as_str()).unwrap_or_default());
    }
    put(&mut header, &["data", "sha256"], hex(&Sha256::digest(all.as_bytes())).into());
    for key in ["recipe", "samples", "lock"] {
        if let Some(text) = header["made"][key]["text"].as_str().map(str::to_owned) {
            put(&mut header, &["made", key, "sha256"], rowver::write::text_checksum(&text).into());
        }
    }
    let names: Vec<String> = header["data"]["samples"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_owned)).collect();
    let taken: Vec<String> = header["data"]["cols"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str().map(str::to_owned)).collect();
    let (names, taken): (Vec<&str>, Vec<&str>) = (names.iter().map(String::as_str).collect(), taken.iter().map(String::as_str).collect());
    if let Ok(cols) = rowver::write::sample_cols(&names, &header["made"]["files"], &taken) {
        put(&mut header, &["data", "sample_cols"], cols.into());
    }
    if let Ok(sum) = rowver::write::made_checksum(&header["made"]) {
        put(&mut header, &["made", "sha256"], sum.into());
    }
    Some((header, if empty { Vec::new() } else { arrays }))
}

/// A flat column's facts made again from its values, as rowver measures a column of them; left as
/// they are when it cannot measure them (a kind it refuses).
fn facts(record: &mut serde_json::Map<String, Value>, kind: &str, values: &[Vec<u8>], cut: &[(usize, usize)], samples: usize, chunk_rows: usize) {
    let held = rowver::rvr::Held { kind: kind.to_owned(), bytes: values.concat(), offsets: None };
    let mut rows = vec![0; samples];
    for (sample, n) in cut {
        rows[*sample] += n;
    }
    let Ok(measured) = rowver::write::derived("x", "x", &held, &rows, chunk_rows) else { return };
    for key in ["lim", "nice", "vals"] {
        match measured.get(key) {
            Some(value) => drop(record.insert(key.to_owned(), value.clone())),
            None => drop(record.shift_remove(key)),
        }
    }
}

/// A file of this header and these arrays (FORMAT §1): the magic, the arrays, the header, its length, the magic.
fn framed(header: &Value, arrays: &[u8]) -> Vec<u8> {
    let text = header.to_string();
    [&MAGIC[..], arrays, text.as_bytes(), &(text.len() as u32).to_le_bytes(), MAGIC].concat()
}

/// The file of `header` and `arrays`, written to the target's input file, its derived columns
/// recorded again as this rowver works them out from it when it opens and has any.
fn written(mut header: Value, arrays: &[u8]) -> crate::Input {
    let input = crate::file(&framed(&header, arrays));
    let records = rowver::rvr::Reader::open(input.path()).ok().and_then(|reader| {
        let listed = rowver::rvr::derived::listed(reader.header());
        (!listed.is_empty() && !reader.empty()).then(|| rowver::rvr::derived::records(&reader, &listed, &[]).ok()).flatten()
    });
    if let Some(records) = records {
        put(&mut header, &["derived", "columns"], records.into());
        let _ = std::fs::write(input.path(), framed(&header, arrays));
    }
    input
}

/// The target: the input made a file, its derived columns recorded, written out to `REHASHED_OUT`
/// when that names a folder, then checked in full; a file that passes must open and read whole.
pub fn check(data: &[u8]) {
    let Some((header, arrays)) = fixed(data) else { return };
    let input = written(header, &arrays);
    if let Some(folder) = std::env::var_os("REHASHED_OUT") {
        let name = hex(&Sha256::digest(data)[..8]);
        let _ = std::fs::copy(input.path(), std::path::Path::new(&folder).join(format!("{name}.rvr")));
    }
    if rowver::rvr::validate_file(input.path()).is_err() {
        return;
    }
    fn failed(what: &str, why: &dyn std::fmt::Display) -> ! {
        panic!("full validation accepted the file, and {what}: {why}")
    }
    let reader = rowver::rvr::Reader::open(input.path()).unwrap_or_else(|e| failed("it does not open", &e));
    if reader.empty() {
        return;
    }
    let rows = reader.rows().unwrap_or_else(|e| failed("its rows are not a count", &e));
    reader.sample().unwrap_or_else(|e| failed("its sample array does not read", &e));
    reader.entry().unwrap_or_else(|e| failed("its entry array does not read", &e));
    let stored = reader.header()["data"]["cols"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str().map(str::to_owned));
    let names: Vec<String> = stored.chain(rowver::rvr::derived::listed(reader.header()).into_iter().map(|(name, _)| name)).collect();
    for name in names {
        let whole = reader.column(&name).unwrap_or_else(|e| failed(&format!("column {name} does not read"), &e));
        let whole = whole.unwrap_or_else(|| failed(&format!("column {name} is not there"), &"none"));
        let slice = reader.slice(&name, 0, rows).unwrap_or_else(|e| failed(&format!("column {name}'s rows do not read"), &e));
        if slice.as_ref() != Some(&whole) {
            failed(&format!("column {name} reads otherwise sliced"), &"its rows 0 to the end differ from the column");
        }
    }
}

#[cfg(test)]
mod tests {
    // Every seed of fuzz/seeds/rehashed (made by fuzz/seed.py from valid files) is made a file full
    // validation accepts, its arrays laid in record order and last record first: this working of
    // the format's rules agrees with rowver's.
    #[test]
    fn every_seed_made_a_file_passes_full_validation() {
        let folder = concat!(env!("CARGO_MANIFEST_DIR"), "/seeds/rehashed");
        let seeds: Vec<_> = std::fs::read_dir(folder).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        assert!(!seeds.is_empty(), "{folder} holds no seeds: make them with mamba run -n rowver python fuzz/seed.py --only rehashed");
        for seed in seeds {
            let mut input = std::fs::read(&seed).unwrap();
            for reversed in [false, true] {
                input[0] = (input[0] & !16) | if reversed { 16 } else { 0 };
                let (header, arrays) = super::fixed(&input).unwrap_or_else(|| panic!("{}: the harness made no file of it", seed.display()));
                let file = super::written(header, &arrays);
                if let Err(why) = rowver::rvr::validate_file(file.path()) {
                    panic!("{} (reversed: {reversed}): the file the harness made of it fails full validation, so the harness's checksums are not rowver's: {why}", seed.display());
                }
            }
        }
    }
}
