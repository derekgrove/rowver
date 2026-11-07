//! What the integration tests share: a folder of their own, a study in one, and rvr run there.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// The fixture `file` of tests/fixtures.
pub fn fixture(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(file)
}

/// samples.toml with these samples, their tree `analysis` unless they say otherwise.
pub fn samples(body: &str) -> String {
    format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n{body}")
}

/// The name of the file a press of the study in `dir` writes.
pub fn name(dir: &Path) -> String {
    format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())
}

/// The header of the study's file, as rvr header prints it.
pub fn header(dir: &Path) -> serde_json::Value {
    let (code, said) = rvr(dir, &["header", &name(dir)]);
    assert_eq!(code, 0, "{said}");
    serde_json::from_str(&said).unwrap()
}

/// A study folder: three of the fixtures beside a samples.toml and a recipe.toml.
pub fn study(name: &str, samples: &str, recipe: &str) -> Study {
    let dir = Study::new(name);
    for file in ["uproot_flat.root", "stl_vectors.root", "codecs-zlib.root"] {
        std::fs::copy(fixture(file), dir.join(file)).unwrap();
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

/// The file at `from` written to `to` with its header changed by `edit`, its arrays as they are.
pub fn reheadered(from: &Path, to: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let bytes = std::fs::read(from).unwrap();
    let (text, arrays) = parts(&bytes);
    let mut header: serde_json::Value = serde_json::from_slice(text).unwrap();
    edit(&mut header);
    let text = serde_json::to_vec(&header).unwrap();
    std::fs::write(to, framed(&text, arrays)).unwrap();
}

/// The eight bytes an `.rvr` file starts and ends with (FORMAT §1).
pub const MAGIC: &[u8; 8] = b"\x89RVR\r\n\x1a\n";

/// A file's header text and its arrays' bytes (from byte 8 to the header), as its last 12 bytes place them.
pub fn parts(bytes: &[u8]) -> (&[u8], &[u8]) {
    let n = bytes.len();
    let length = u32::from_le_bytes(bytes[n - 12..n - 8].try_into().unwrap()) as usize;
    let start = n - 12 - length;
    (&bytes[start..n - 12], &bytes[8..start])
}

/// A file of these arrays and this header text: the magic, the arrays, the header, its length, the magic.
pub fn framed(text: &[u8], arrays: &[u8]) -> Vec<u8> {
    [&MAGIC[..], arrays, text, &(text.len() as u32).to_le_bytes(), MAGIC].concat()
}

/// The journal beside the file at `path` while a change to it is made (SPEC 8.12).
pub fn journal(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-journal");
    name.into()
}

/// The file's inode: a change made in place keeps it, a file moved over it does not.
pub fn inode(path: &Path) -> u64 {
    std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(path).unwrap())
}

/// Run rvr in `dir` at a terminal, answering `answer` to the question it asks, if it asks one.
pub fn at_terminal(dir: &Path, args: &[&str], answer: &str) -> (i32, String) {
    at_terminal_then(dir, args, answer, || {})
}

/// `at_terminal`, running `asked` while the question waits for its answer.
pub fn at_terminal_then(dir: &Path, args: &[&str], answer: &str, asked: impl FnOnce()) -> (i32, String) {
    let rvr = env!("CARGO_BIN_EXE_rvr");
    let mut command = Command::new("script");
    match cfg!(target_os = "macos") {
        true => command.args(["-q", "/dev/null", rvr]).args(args),
        false => command.args(["-qec", &format!("{rvr} {}", args.join(" ")), "/dev/null"]),
    };
    let mut child = command.current_dir(dir).env("RVR_CONFIG", dir.join(".rvr-config")).env("TERM", "dumb").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut out = child.stdout.take().unwrap();
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reading = {
        let said = said.clone();
        std::thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n @ 1..) = out.read(&mut buf) {
                said.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        })
    };
    let mut input = child.stdin.take().unwrap();
    let (started, mut asked) = (std::time::Instant::now(), Some(asked));
    while child.try_wait().unwrap().is_none() && started.elapsed().as_secs() < 20 {
        if String::from_utf8_lossy(&said.lock().unwrap()).contains("[y/N] ") {
            if let Some(asked) = asked.take() {
                asked();
            }
            input.write_all(format!("{answer}\n").as_bytes()).unwrap();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let status = child.wait().unwrap();
    drop(input);
    reading.join().unwrap();
    let said = String::from_utf8_lossy(&said.lock().unwrap()).replace("\r\n", "\n");
    (status.code().unwrap_or(-1), said)
}
