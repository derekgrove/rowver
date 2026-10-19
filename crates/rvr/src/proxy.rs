//! `rvr proxy`: the grid proxy remote files are read with. `rvr proxy` says what state it is
//! in; `rvr proxy init` makes one from your grid certificate in `~/.globus`, with no other
//! tool installed: it asks the VOMS server for your attributes and writes the proxy where grid
//! tools look (`$X509_USER_PROXY`, else `/tmp/x509up_u<uid>`).

use std::path::PathBuf;
use voms_proxy::user::Source;
use voms_proxy::{make, Credentials, Places};

/// What `rvr proxy` takes.
pub const USAGE: &str = "usage:
  rvr proxy                                your grid proxy: whose, for which VO, how long it lasts
  rvr proxy init [--hours H] [--vo VO]     make one from your certificate in ~/.globus
                                           (192 hours and cms unless told otherwise)";

/// A proxy, or VOMS attributes, with less than this many seconds left is warned of.
pub const SHORT: i64 = 6 * 3600;

/// A length of time in days and hours, or hours and minutes.
pub fn left(seconds: i64) -> String {
    let unit = |n: i64, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    let (days, hours, minutes) = (seconds / 86400, seconds % 86400 / 3600, seconds % 3600 / 60);
    match (days, hours) {
        (0, 0) => unit(minutes.max(0), "minute"),
        (0, _) => format!("{} {}", unit(hours, "hour"), unit(minutes, "minute")),
        _ => format!("{} {}", unit(days, "day"), unit(hours, "hour")),
    }
}

/// `rvr proxy …` with the words after `proxy`: the exit status.
pub fn main(words: &[&str]) -> u8 {
    let result = match words {
        [] => show(),
        ["init", given @ ..] => options(given).and_then(|(hours, vo)| init(hours, vo)),
        _ => Err(String::new()),
    };
    match result {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(why) if why.is_empty() => {
            eprintln!("{USAGE}");
            2
        }
        Err(why) => {
            eprintln!("rvr proxy: {why}");
            1
        }
    }
}

/// `init`'s options, each a flag and its value: the hours and the VO; empty text for a usage error.
fn options<'a>(mut words: &[&'a str]) -> Result<(u32, &'a str), String> {
    let (mut hours, mut vo) = (192, "cms");
    while let [flag, value, rest @ ..] = words {
        match *flag {
            "--hours" => {
                hours = value.parse().ok().filter(|h| (1..=24 * 30).contains(h)).ok_or_else(|| {
                    format!("--hours is how long the proxy lasts, a whole number of hours from 1 to 720, not {value}")
                })?
            }
            "--vo" => vo = value,
            _ => return Err(String::new()),
        }
        words = rest;
    }
    match words {
        [] => Ok((hours, vo)),
        _ => Err(String::new()),
    }
}

/// The seconds until the proxy or its VOMS attributes end, whichever is first.
pub fn left_of(status: &voms_proxy::Status) -> Option<i64> {
    match (status.seconds_left, status.attributes_left) {
        (Some(proxy), Some(attributes)) => Some(proxy.min(attributes)),
        (proxy, attributes) => proxy.or(attributes),
    }
}

/// Whether `rvr proxy init` is what an unusable proxy needs: there is none, it ended, or its
/// file holds no proxy that can be used.
pub fn renewable(status: &voms_proxy::Status) -> bool {
    status.renew || ["noCertificate", "noKey", "unreadable"].contains(&status.state)
}

/// Print the proxy's state; true when it can be used.
fn show() -> Result<bool, String> {
    let credentials = Credentials::found(None, None, &Places::here());
    let status = voms_proxy::status(&credentials);
    if let Some(path) = &status.path {
        println!("proxy     {path}");
    }
    if let Some(who) = &status.identity {
        println!("whose     {who}");
    }
    if let Some(vo) = &status.organisation {
        let until = status.attributes_expire.as_deref().unwrap_or("?");
        println!("VO        {vo} (attributes until {until})");
    }
    if let (Some(left), Some(until)) = (status.seconds_left, &status.expires) {
        if left > 0 {
            println!("left      {}, until {until}", self::left(left));
        }
    }
    if let Some(warning) = &status.warning {
        println!("warning   {warning}");
    }
    let usable = status.usable();
    let next = match (usable, left_of(&status)) {
        (false, _) if renewable(&status) => " Run rvr proxy init to make one.",
        (true, _) if status.renew => " Run rvr proxy init for one with new attributes.",
        (true, Some(left)) if left < SHORT => " It ends soon: run rvr proxy init for a new one.",
        _ => "",
    };
    println!("{}.{next}", status.message);
    Ok(usable)
}

/// Make a proxy valid `hours` with the attributes of `vo`, write it, and show it.
fn init(hours: u32, vo: &str) -> Result<bool, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set, so ~/.globus cannot be found")?;
    let globus = PathBuf::from(home).join(".globus");
    let named = |name: &str| std::env::var(name).ok();
    let source = Source::find(&globus, named("X509_USER_CERT"), named("X509_USER_KEY"))?;
    source.private()?;
    let passphrase = match source.encrypted()? {
        true => {
            voms_proxy::user::passphrase(&format!("Passphrase for {}: ", source.key().display()))
                .map_err(|e| format!("the passphrase could not be read: {e}"))?
        }
        false => String::new(),
    };
    let user = source.read(&passphrase);
    voms_proxy::wipe(&mut passphrase.into_bytes());
    let user = user?;
    let seconds = i64::from(hours) * 3600;
    let ca = crate::samples::proxy().ca;
    let server = voms_proxy::server::SERVERS
        .iter()
        .find(|(name, _)| *name == vo);
    println!(
        "Asking {} for your {vo} attributes...",
        server.map_or("the VOMS server", |s| s.1)
    );
    let attributes =
        voms_proxy::server::attributes(vo, seconds as u64, user.loaded(), ca.as_deref())?;
    let now = voms_proxy::now();
    let made = make::proxy(&user, Some(&attributes), now, seconds)?;
    let places = Places::here();
    let path = places
        .proxy_variable
        .or(places.usual_proxy)
        .ok_or("this platform has no usual place for a grid proxy; set X509_USER_PROXY")?;
    make::write(path.as_ref(), &made.text)
        .map_err(|e| format!("{path} could not be written: {e}"))?;
    if made.not_after < now + seconds {
        println!(
            "Your certificate ends at {}, so the proxy ends then too. Renew your certificate with your certificate authority.",
            voms_proxy::utc(made.not_after)
        );
    }
    println!("Wrote {path}:");
    show()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_takes_each_flag_with_its_value() {
        assert_eq!(options(&[]), Ok((192, "cms")));
        assert_eq!(
            options(&["--vo", "atlas", "--hours", "12"]),
            Ok((12, "atlas"))
        );
        assert_eq!(
            options(&["12", "--hours"]),
            Err(String::new()),
            "a value before its flag"
        );
        assert_eq!(options(&["--hours"]), Err(String::new()));
        assert!(options(&["--hours", "0"])
            .unwrap_err()
            .contains("from 1 to 720"));
        assert_eq!(left(3 * 86400 + 2 * 3600), "3 days 2 hours");
        assert_eq!(left(3600 + 60), "1 hour 1 minute");
        assert_eq!(left(30), "0 minutes");
    }
}
