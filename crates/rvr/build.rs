//! What built rvr, for `made.toolchain`: the commit (docs/SPEC.md §12.2: from the package cargo
//! made, this checkout, with `-dirty` and a hash of the changes when its code had changes, or the
//! Python source package), the compiler, and the target.

use std::io::Write;
use std::process::{Command, Stdio};

/// What a command prints, trimmed, or `None` when it cannot run or fails.
fn said(program: &str, args: &[&str]) -> Option<String> {
    printed(program, args).map(|out| String::from_utf8_lossy(&out).trim().to_owned())
}

/// What a command prints, as it prints it, or `None` when it cannot run or fails.
fn printed(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// The first 12 hex digits of git's hash of `bytes`, written nowhere.
fn hashed(root: &str, bytes: &[u8]) -> Option<String> {
    let mut git = Command::new("git")
        .args(["-C", root, "hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    git.stdin.take()?.write_all(bytes).ok()?;
    let out = git.wait_with_output().ok()?;
    let hash = String::from_utf8(out.stdout).ok()?;
    (out.status.success() && hash.len() >= 12).then(|| hash[..12].to_owned())
}

/// The code that is built (everything under `crates`, and the workspace's `Cargo.toml` and
/// `Cargo.lock`, the files watched below).
const CODE: [&str; 3] = ["crates", "Cargo.toml", "Cargo.lock"];

/// How the code differs from its commit: every change to a tracked file of it, and every file
/// under `crates` git does not track or ignore, by path and contents; empty when none.
fn changes(root: &str) -> Option<Vec<u8>> {
    let git = |args: &[&str]| printed("git", &[&["-C", root], args].concat());
    let diff = [
        "diff",
        "HEAD",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
        "--",
    ];
    let mut bytes = git(&[&diff[..], &CODE].concat())?;
    let untracked = git(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        "crates",
    ])?;
    for path in untracked.split(|b| *b == 0).filter(|path| !path.is_empty()) {
        let path = std::str::from_utf8(path).ok()?;
        bytes.extend_from_slice(b"\0untracked\0");
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&std::fs::read(format!("{root}/{path}")).ok()?);
    }
    Some(bytes)
}

/// The commit cargo recorded when it packaged this crate (`.cargo_vcs_info.json`, written by
/// `cargo package` and `cargo publish`), for a build from crates.io, which has no git checkout.
fn packaged(manifest: &str) -> Option<String> {
    let info = std::fs::read_to_string(format!("{manifest}/.cargo_vcs_info.json")).ok()?;
    let info: serde_json::Value = serde_json::from_str(&info).ok()?;
    let sha = info["git"]["sha1"].as_str()?;
    Some(match info["git"]["dirty"].as_bool() {
        Some(true) => format!("{sha}-dirty"),
        _ => sha.to_owned(),
    })
}

/// Whether `text` is one line naming a commit: 40 lowercase hex digits.
fn named(text: &str) -> Option<&str> {
    let commit = text.strip_suffix('\n').unwrap_or(text);
    let hex = commit.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (commit.len() == 40 && hex).then_some(commit)
}

/// Whether `text` is a commit as a build names it (docs/SPEC.md §12.2): 40 lowercase hex digits,
/// alone or followed by `-dirty` or `-dirty-` and 12 more, or `unknown`.
fn a_commit(text: &str) -> bool {
    let (sha, rest) = (text.get(..40).unwrap_or(""), text.get(40..).unwrap_or(""));
    let hash = |h: &str| h.len() == 12 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    text == "unknown" || (named(sha).is_some() && (rest.is_empty() || rest == "-dirty" || rest.strip_prefix("-dirty-").is_some_and(hash)))
}

/// The commit a build names, first that applies (docs/SPEC.md §12.2): the one cargo packaged, the
/// one the release wrote into the Python source package (its `COMMIT`, read only outside a
/// checkout), this checkout's, or `unknown`.
fn chosen(packaged: Option<String>, sdist: Option<&str>, checkout: Option<String>) -> String {
    let sdist = sdist.and_then(named).filter(|_| checkout.is_none());
    packaged
        .or(sdist.map(str::to_owned))
        .or(checkout)
        .unwrap_or_else(|| "unknown".into())
}

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let git = |args: &[&str]| said("git", &[&["-C", root], args].concat());
    // Only rowver's own checkout, which tracks this file at this path: a copy of rvr inside
    // some other repository would otherwise record that repository's commit.
    let checkout = std::path::Path::new(root).join(".git").exists()
        && git(&["ls-files", "--error-unmatch", "crates/rvr/build.rs"]).is_some();
    if checkout {
        for watched in [&[".git/HEAD", ".git/index"][..], &CODE].concat() {
            println!("cargo:rerun-if-changed={root}/{watched}");
        }
    } else {
        println!("cargo:rerun-if-changed=build.rs");
        // Watched only when there: cargo takes a missing file for one always changed.
        if std::path::Path::new(root).join("COMMIT").exists() {
            println!("cargo:rerun-if-changed={root}/COMMIT");
        }
    }
    // Two builds with the same changes get the same hash, and with different changes different
    // ones; changes that cannot be read leave `-dirty` alone.
    let checkout = checkout.then(|| match git(&["rev-parse", "HEAD"]) {
        Some(commit) => match changes(root) {
            Some(bytes) if bytes.is_empty() => commit,
            bytes => match bytes.and_then(|bytes| hashed(root, &bytes)) {
                Some(hash) => format!("{commit}-dirty-{hash}"),
                None => format!("{commit}-dirty"),
            },
        },
        None => "unknown".into(),
    });
    let sdist = std::fs::read_to_string(format!("{root}/COMMIT")).ok();
    let commit = chosen(packaged(manifest), sdist.as_deref(), checkout);
    // A debug build for tests/builds.rs only: the commit given, to press as an unknown or dirty build.
    println!("cargo:rerun-if-env-changed=RVR_BUILD_COMMIT");
    let given = std::env::var("RVR_BUILD_COMMIT").ok().filter(|c| !c.is_empty());
    let commit = match given {
        Some(given) if std::env::var("PROFILE").as_deref() == Ok("debug") => {
            assert!(a_commit(&given), "RVR_BUILD_COMMIT is {given:?}, not a commit a build names (40 lowercase hex digits, then nothing, -dirty or -dirty- and 12 more; or unknown): give one such, or unset it");
            given
        }
        _ => commit,
    };
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rustc = said(&rustc, &["-V"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RVR_COMMIT={commit}");
    println!("cargo:rustc-env=RVR_RUSTC={rustc}");
    println!(
        "cargo:rustc-env=RVR_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}

#[cfg(test)]
mod tests {
    use super::{a_commit, chosen, packaged};

    const A: &str = "0123456789abcdef0123456789abcdef01234567";

    // SPEC 12.2
    #[test]
    fn packaged_comes_first() {
        let packaged = Some(format!("{A}-dirty"));
        assert_eq!(chosen(packaged.clone(), Some(A), None), format!("{A}-dirty"));
        assert_eq!(chosen(packaged, None, Some("b".into())), format!("{A}-dirty"));
    }

    // SPEC 12.2
    #[test]
    fn sdist_names_a_commit_outside_a_checkout() {
        assert_eq!(chosen(None, Some(&format!("{A}\n")), None), A);
        assert_eq!(chosen(None, Some(A), None), A);
    }

    // SPEC 12.2
    #[test]
    fn sdist_file_that_names_no_commit_is_unknown() {
        let upper = A.to_uppercase();
        let bad = ["", "\n", &A[..39], &format!("{A}0"), &upper, &format!("{A}\n\n"), &format!(" {A}")];
        for text in bad {
            assert_eq!(chosen(None, Some(text), None), "unknown", "{text:?}");
        }
    }

    // SPEC 12.2
    #[test]
    fn checkout_is_its_own_even_with_the_file() {
        let dirty = format!("{A}-dirty-0123456789ab");
        let other = "fedcba9876543210fedcba9876543210fedcba98";
        assert_eq!(chosen(None, Some(other), Some(dirty.clone())), dirty);
        assert_eq!(chosen(None, Some(other), Some(A.into())), A);
        assert_eq!(chosen(None, Some(other), Some("unknown".into())), "unknown");
    }

    // SPEC 12.2
    #[test]
    fn a_crate_from_crates_io_names_the_commit_cargo_recorded() {
        let dir = std::env::temp_dir().join(format!("rvr-vcs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let info = |dirty: &str| format!("{{\"git\": {{\"sha1\": \"{A}\"{dirty}}}, \"path_in_vcs\": \"crates/rvr\"}}");
        std::fs::write(dir.join(".cargo_vcs_info.json"), info("")).unwrap();
        assert_eq!(packaged(dir.to_str().unwrap()), Some(A.to_owned()));
        std::fs::write(dir.join(".cargo_vcs_info.json"), info(", \"dirty\": true")).unwrap();
        assert_eq!(packaged(dir.to_str().unwrap()), Some(format!("{A}-dirty")));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(packaged(dir.to_str().unwrap()), None);
    }

    // SPEC 12.2
    #[test]
    fn a_test_build_names_only_a_commit_a_build_can() {
        for good in ["unknown", A, &format!("{A}-dirty"), &format!("{A}-dirty-0123456789ab")] {
            assert!(a_commit(good), "{good}");
        }
        for bad in ["", "abc", "é".repeat(30).as_str(), &A.to_uppercase(), &format!("{A}-dirty-0123"), &format!("{A}-clean"), &format!("{A}0"), &format!("{A}-dirty-0123456789AB")] {
            assert!(!a_commit(bad), "{bad}");
        }
    }

    // SPEC 12.2
    #[test]
    fn nothing_is_unknown() {
        assert_eq!(chosen(None, None, None), "unknown");
    }
}
