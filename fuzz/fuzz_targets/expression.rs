//! The expression parser on arbitrary text.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Some(text) = rowver_fuzz::text(data) {
        let _ = rvr_recipe::parse(text);
    }
});
