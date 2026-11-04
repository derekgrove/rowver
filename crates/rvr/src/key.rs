//! `rvr key`: the ed25519 key that signs every file you press, and the keys you trust.
//!
//! The key lives in `~/.config/rvr/key` (the folder is `$RVR_CONFIG` when set and not empty, else
//! `$XDG_CONFIG_HOME/rvr` when that is), one line of 64 lowercase hexadecimal characters, readable
//! by you alone; `trusted` beside it holds one line per trusted public key: the key, a space, and
//! whose it is (`#` lines and blank lines passed over).

use std::path::PathBuf;

pub const USAGE: &str = "usage:
  rvr key new [NAME]                       make your signing key; every press signs from then on
  rvr key show                             your public key, to give to whoever reads your files
  rvr key trust KEY NAME...                trust files signed by KEY, as NAME's (its words joined)";

/// The folder rvr keeps its key and trusted keys in; a variable set but empty counts as not set.
fn folder() -> Result<PathBuf, String> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(dir) = set("RVR_CONFIG") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = set("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("rvr"));
    }
    let home =
        std::env::var_os("HOME").ok_or("HOME is not set, so rvr cannot find ~/.config/rvr")?;
    Ok(PathBuf::from(home).join(".config/rvr"))
}

/// Your secret key, or `None` when you have made none.
pub fn secret() -> Result<Option<[u8; 32]>, String> {
    let path = folder()?.join("key");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(rowver::plain_io(&path, &e)),
    };
    rowver::sign::bytes(text.trim()).map(Some).map_err(|_| {
        format!("{} is not an rvr key (one line of 64 lowercase hexadecimal characters): move it away and run rvr key new", path.display())
    })
}

/// The first 16 characters of a key, which is enough to tell keys apart by eye.
pub fn short(key: &str) -> String {
    format!("{}…", &key[..16.min(key.len())])
}

/// The trusted keys, as (key, whose).
fn trusted() -> Result<Vec<(String, String)>, String> {
    let path = folder()?.join("trusted");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(rowver::plain_io(&path, &e)),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| {
            let (key, name) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
            (key.to_owned(), name.trim().to_owned())
        })
        .collect())
}

/// Whose `key` is: yours, a trusted key's owner's (by name), or no one's you know.
enum Owner {
    You,
    Trusted(String),
    Unknown,
}

/// Whose `key` is, as your key and the keys you trust say.
fn owner(key: &str) -> Owner {
    if secret().ok().flatten().is_some_and(|own| rowver::sign::public_key(&own) == key) {
        return Owner::You;
    }
    match trusted().unwrap_or_default().into_iter().find(|(k, _)| k == key) {
        Some((_, name)) => Owner::Trusted(name),
        None => Owner::Unknown,
    }
}

/// Who signed a file, in words: you, a trusted key's owner, or an unknown key.
pub fn signer(key: &str) -> String {
    match owner(key) {
        Owner::You => "signed by you".into(),
        Owner::Trusted(name) => format!("signed by {name} (trusted)"),
        Owner::Unknown => format!("signed by key {}, which you do not trust yet (rvr key trust {key} NAME)", short(key)),
    }
}

/// Whose a signature is, in words: your own key, a trusted key's owner, or a key you do not trust.
pub fn whose(key: &str) -> String {
    match owner(key) {
        Owner::You => format!("your own key {}", short(key)),
        Owner::Trusted(name) => format!("{name} (trusted)"),
        Owner::Unknown => format!("key {}, which you do not trust", short(key)),
    }
}

/// What signing a file again with `signer` (yours, when you have one) does to its signature by `was`, in words.
pub fn resigned(signer: Option<&[u8; 32]>, was: Option<&str>) -> String {
    match (signer, was) {
        (Some(secret), Some(was)) => format!("signed with your key {}, which replaces the signature of {}", short(&rowver::sign::public_key(secret)), whose(was)),
        (Some(secret), None) => format!("signed with your key {}; it was not signed before", short(&rowver::sign::public_key(secret))),
        (None, Some(was)) => format!("no longer signed: it was signed by {}, and you have no key to sign it again (rvr key new makes one)", whose(was)),
        (None, None) => "not signed, as it was not before".into(),
    }
}

/// 32 bytes from the system's random source.
fn random() -> Result<[u8; 32], String> {
    use std::io::Read;
    let mut bytes = [0; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("/dev/urandom cannot be read to make a key ({}): run rvr key new on a computer that has it", rowver::described(&e)))?;
    Ok(bytes)
}

/// Make a new key, never over one: written into a file beside it, then linked in its place, or
/// where links are refused, written at its place as a new file.
fn new(name: Option<&str>) -> Result<(), String> {
    let dir = folder()?;
    let path = dir.join("key");
    let have = || format!("You already have a key ({}); every press signs with it. To start again, move it away first.", path.display());
    if path.exists() {
        return Err(have());
    }
    std::fs::create_dir_all(&dir).map_err(|e| rowver::plain_io(&dir, &e))?;
    let secret = random()?;
    let text = format!("{}\n", rowver::rvr::hex(&secret));
    // A new file, readable by you alone, whole on disk; it fails if one is there.
    let fresh = |at: &std::path::Path| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        use std::io::Write;
        options.open(at).and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
    };
    let beside = dir.join(format!(".key.{}", &rowver::rvr::hex(&random()?)[..12]));
    let written = fresh(&beside).map_err(|e| rowver::plain_io(&beside, &e)).and_then(|()| match std::fs::hard_link(&beside, &path) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(have()),
        Err(_) => match fresh(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(have()),
            made => made.map_err(|e| rowver::plain_io(&path, &e)),
        },
        Ok(()) => Ok(()),
    });
    let _ = std::fs::remove_file(&beside);
    written?;
    let public = rowver::sign::public_key(&secret);
    println!(
        "Made your key {} (keep it private). Every file you press is signed with it from now on.",
        path.display()
    );
    println!("Your public key, for whoever reads your files:\n  {public}");
    println!(
        "They trust your files with:\n  rvr key trust {public} {}",
        name.unwrap_or("YOUR_NAME")
    );
    Ok(())
}

/// Print the public key.
fn show() -> Result<(), String> {
    match secret()? {
        Some(secret) => println!("{}", rowver::sign::public_key(&secret)),
        None => return Err("You have no key yet: run rvr key new.".into()),
    }
    Ok(())
}

/// Add a key to the trusted keys; a key trusted already is left as it is, under its name.
fn trust(key: &str, name: &str) -> Result<(), String> {
    rowver::sign::bytes::<32>(key).map_err(|_| format!("{key:?} is not a public key: one is 64 lowercase hexadecimal characters, as rvr key show prints it"))?;
    let path = folder()?.join("trusted");
    match trusted()?.into_iter().find(|(k, _)| k == key) {
        Some((_, whose)) if whose == name => {
            println!("{} is trusted already, as {whose}'s: nothing to change.", short(key));
            return Ok(());
        }
        Some((_, whose)) => {
            return Err(format!("{} is trusted already, as {whose}'s, and is left so: to trust it as {name}'s, take its line out of {} and trust it again", short(key), path.display()));
        }
        None => {}
    }
    let dir = folder()?;
    std::fs::create_dir_all(&dir).map_err(|e| rowver::plain_io(&dir, &e))?;
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{key} {name}"))
        .map_err(|e| rowver::plain_io(&path, &e))?;
    println!("Files signed by {} now say they are {name}'s.", short(key));
    Ok(())
}

/// `rvr key new|show|trust` (`name`) with its words: the exit status.
pub fn main(name: &str, words: &[&str]) -> u8 {
    let result = match (name, words) {
        ("key new", owner) => new(owner.first().copied()),
        ("key show", _) => show(),
        (_, [key, owner @ ..]) if !owner.is_empty() => trust(key, &owner.join(" ")),
        _ => Err("rvr key trust takes a key and its owner's name".into()),
    };
    crate::report(name, result.map(|()| 0))
}
