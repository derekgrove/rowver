//! How rvr replaces a file: a new one written beside it, then renamed over it.

use std::path::{Path, PathBuf};

/// `path` with `suffix` added to its name.
pub(crate) fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The file `write` makes at `staged` renamed over `target`, once on disk when `sync`; on a failure `staged` goes and `target` is as it was.
pub(crate) fn in_place<E>(staged: &Path, target: &Path, sync: bool, write: impl FnOnce() -> Result<(), E>, failed: impl Fn(&Path, std::io::Error) -> E) -> Result<(), E> {
    let put = write()
        .and_then(|()| match sync {
            true => std::fs::File::open(staged).and_then(|f| f.sync_all()).map_err(|e| failed(staged, e)),
            false => Ok(()),
        })
        .and_then(|()| std::fs::rename(staged, target).map_err(|e| failed(target, e)));
    if put.is_err() {
        let _ = std::fs::remove_file(staged);
    }
    put
}
