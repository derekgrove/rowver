//! `Lock::read` on arbitrary text (a samples.lock).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Some(text) = rowver_fuzz::text(data) {
        let _ = rowver::study::lock::Lock::read(text);
    }
});
