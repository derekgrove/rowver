//! The options parser on arbitrary argument lists: the arguments, a zero byte after each.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some(text) = rowver_fuzz::text(data) else { return };
    let args: Vec<&str> = text.split_terminator('\0').collect();
    rvr::fuzzing::words(&args);
});
