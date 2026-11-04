//! `samples::read` on arbitrary text (a samples.toml).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Some(text) = rowver_fuzz::text(data) {
        let _ = rowver::study::samples::read(text);
    }
});
