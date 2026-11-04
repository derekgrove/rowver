//! The multipart parser on arbitrary answers: a Content-Type line, then the answer to a request
//! for as many bytes as the answer holds.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let (line, answer) = data.split_at(data.iter().position(|b| *b == b'\n').map_or(data.len(), |at| at + 1));
    if let Some(content_type) = rowver_fuzz::text(line) {
        rvr::fuzzing::multipart(content_type.trim_end(), answer, answer.len() as u64);
    }
});
