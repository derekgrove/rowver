//! What built rvr, for `made.toolchain`: the git commit (with `-dirty` when it had changes), from
//! the package cargo made or else this checkout, the compiler, and the target.

use std::process::Command;

/// What a command prints, trimmed, or `None` when it cannot run or fails.
fn said(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
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

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let git = |args: &[&str]| said("git", &[&["-C", root], args].concat());
    // Only rowver's own checkout, which tracks this file at this path: a copy of rvr inside
    // some other repository would otherwise record that repository's commit.
    let checkout = std::path::Path::new(root).join(".git").exists()
        && git(&["ls-files", "--error-unmatch", "crates/rvr/build.rs"]).is_some();
    if checkout {
        for watched in [".git/HEAD", ".git/index", "crates"] {
            println!("cargo:rerun-if-changed={root}/{watched}");
        }
    } else {
        println!("cargo:rerun-if-changed=build.rs");
    }
    let commit = match packaged(manifest) {
        Some(commit) => commit,
        // Only this repository's own checkout: from anywhere else, git would name whatever
        // repository happens to hold the folder.
        None if checkout => match git(&["rev-parse", "HEAD"]) {
            Some(commit) => match git(&["status", "--porcelain", "--untracked-files=no"]) {
                Some(changes) if changes.is_empty() => commit,
                _ => format!("{commit}-dirty"),
            },
            None => "unknown".into(),
        },
        None => "unknown".into(),
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
