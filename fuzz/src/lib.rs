//! What the fuzz targets share: the reader takes a path, so a target's bytes are written to a file first.

use std::path::{Path, PathBuf};

pub mod rehash;

/// A fuzz input written to a file of `FUZZ_INPUTS` (else the temporary folder), removed when dropped.
pub struct Input(PathBuf);

impl Input {
    /// Its path.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `data` written to this process's one file in `FUZZ_INPUTS`, a folder for the inputs alone (a RAM
/// disk spares the disk), else the temporary folder.
pub fn file(data: &[u8]) -> Input {
    let folder = std::env::var_os("FUZZ_INPUTS").map_or_else(std::env::temp_dir, PathBuf::from);
    let path = folder.join(format!("rowver-fuzz-{}.rvr", std::process::id()));
    std::fs::write(&path, data).expect("the inputs' folder takes the fuzz input");
    Input(path)
}

/// `data` as text, when it is UTF-8 (every text RowVer reads is).
pub fn text(data: &[u8]) -> Option<&str> {
    std::str::from_utf8(data).ok()
}
