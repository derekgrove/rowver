//! `Reader::open` on arbitrary bytes, and every column of a file it opens read (FORMAT §12).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let input = rowver_fuzz::file(data);
    let Ok(reader) = rowver::rvr::Reader::open(input.path()) else { return };
    let _ = (reader.sample(), reader.entry());
    let header = reader.header();
    let stored = header["data"]["cols"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str());
    let names: Vec<String> = stored.map(str::to_owned).chain(rowver::rvr::derived::listed(header).into_iter().map(|(name, _)| name)).collect();
    for name in names {
        let _ = reader.column(&name);
    }
});
