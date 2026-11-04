//! The conformance corpus (tests/conformance): files and a JSON manifest another implementation
//! checks itself against, made by these tests and checked by them on every run.

use serde_json::Value;

/// The conformance corpus's folder `part` (tests/conformance at the repository's top): with
/// `CONFORMANCE_BLESS` set, `files` and `manifest` (as manifest.json) are written there; else they
/// must be what is there, and nothing else.
pub fn corpus(part: &str, files: &[(String, Vec<u8>)], manifest: &Value) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance").join(part);
    let mut all = files.to_vec();
    all.push(("manifest.json".into(), (serde_json::to_string_pretty(&sorted(manifest)).unwrap() + "\n").into_bytes()));
    if std::env::var_os("CONFORMANCE_BLESS").is_some() {
        let _ = std::fs::remove_dir_all(&dir);
        for (name, bytes) in &all {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        return;
    }
    let mut stale = Vec::new();
    for (name, bytes) in &all {
        if std::fs::read(dir.join(name)).ok().as_ref() != Some(bytes) {
            stale.push(name.clone());
        }
    }
    let mut there = Vec::new();
    let mut folders = vec![dir.clone()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            match entry.path().is_dir() {
                true => folders.push(entry.path()),
                false => there.push(entry.path().strip_prefix(&dir).unwrap().to_string_lossy().into_owned()),
            }
        }
    }
    stale.extend(there.into_iter().filter(|name| !all.iter().any(|(n, _)| n == name)));
    assert!(stale.is_empty(), "the conformance corpus ({}) is not what this code makes: {stale:?}. Make it again with CONFORMANCE_BLESS=1 cargo test -p rowver, and review the change.", dir.display());
}

/// `value` with every object's keys in byte order, whatever order serde_json keeps them in (which
/// depends on the features the crates built together ask of it).
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&map[k]))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}
