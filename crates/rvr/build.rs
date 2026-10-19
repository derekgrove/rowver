//! What built rvr, for `made.toolchain`: the git commit of this checkout (with `-dirty` when it
//! has changes), the compiler, and the target.

use std::process::Command;

/// What a command prints, trimmed, or `None` when it cannot run or fails.
fn said(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    for watched in [".git/HEAD", ".git/index", "crates"] {
        println!("cargo:rerun-if-changed={root}/{watched}");
    }
    let git = |args: &[&str]| said("git", &[&["-C", root], args].concat());
    let commit = match git(&["rev-parse", "HEAD"]) {
        Some(commit) => match git(&["status", "--porcelain", "--untracked-files=no"]) {
            Some(changes) if changes.is_empty() => commit,
            _ => format!("{commit}-dirty"),
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
