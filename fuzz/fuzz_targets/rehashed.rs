//! Full validation of files whose every checksum is right, made from the input (fuzz/src/rehash.rs);
//! a file it accepts must open and read every column, whole and sliced, alike.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| rowver_fuzz::rehash::check(data));
