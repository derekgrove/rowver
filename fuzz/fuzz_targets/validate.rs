//! Full validation (`rvr validate`) on arbitrary bytes.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let input = rowver_fuzz::file(data);
    let _ = rowver::rvr::validate_file(input.path());
});
