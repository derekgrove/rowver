//! What the integration tests share: a folder of their own, a study in one, and rvr run there.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A folder of its own in the temporary directory, deleted when the test ends, passed or failed.
pub struct Study(pub PathBuf);

impl Study {
    /// An empty folder named for `name`.
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rvr-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Study(dir)
    }
}

impl std::ops::Deref for Study {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Study {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A study folder: the fixtures beside a samples.toml and a recipe.toml.
pub fn study(name: &str, samples: &str, recipe: &str) -> Study {
    let dir = Study::new(name);
    for file in ["uproot_flat.root", "stl_vectors.root", "codecs-zlib.root"] {
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(file), dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("samples.toml"), samples).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe).unwrap();
    dir
}

/// Run rvr in `dir`: its exit status, and what it printed and complained.
pub fn rvr(dir: &Path, args: &[&str]) -> (i32, String) {
    rvr_with(dir, &[], args)
}

/// Run rvr in `dir` with `home` as the home folder.
pub fn rvr_at_home(dir: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    rvr_with(dir, &[("HOME", home.to_str().unwrap())], args)
}

/// Run rvr in `dir` with these variables set.
pub fn rvr_with(dir: &Path, set: &[(&str, &str)], args: &[&str]) -> (i32, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rvr"));
    command.args(args).current_dir(dir).env("RVR_CONFIG", dir.join(".rvr-config"));
    for (name, value) in set {
        command.env(name, value);
    }
    let out = command.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}
