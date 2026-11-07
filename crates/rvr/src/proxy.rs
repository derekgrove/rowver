//! `rvr proxy`: the grid proxy remote files are read with. `rvr proxy` says what state it is
//! in; `rvr proxy init` makes one from your grid certificate in `~/.globus`, with no other
//! tool installed: it asks the VOMS server for your attributes and writes the proxy where grid
//! tools look (`$X509_USER_PROXY`, else `/tmp/x509up_u<uid>`).

use voms_proxy::user::Source;
use voms_proxy::{make, Credentials, Places};

/// What `rvr proxy` takes.
pub(crate) const USAGE: &str = "usage:
  rvr proxy                                your grid proxy: whose, for which VO, how long it lasts
  rvr proxy init [--hours H]               make one for the VO cms from your certificate in
                                           ~/.globus, lasting H hours (1 to 720; 192 when not given)";

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

/// `rvr proxy` or `rvr proxy init` (`name`) with its options: the exit status.
pub fn main(name: &str, options: &crate::options::Options) -> u8 {
    let result = match name {
        "proxy" => show(),
        _ => match options.value("--hours").map(|h| crate::options::count("--hours", h, 720)).transpose() {
            Ok(hours) => init(hours.unwrap_or(192) as u32),
            Err(why) => return crate::wrong(name, &why),
        },
    };
    crate::report(name, result.map(|good| u8::from(!good)))
}

/// The seconds until the proxy or its VOMS attributes end, whichever is first.
pub fn left_of(status: &voms_proxy::Status) -> Option<i64> {
    match (status.seconds_left, status.attributes_left) {
        (Some(proxy), Some(attributes)) => Some(proxy.min(attributes)),
        (proxy, attributes) => proxy.or(attributes),
    }
}

/// Whether `rvr proxy init` is what an unusable proxy needs: there is none, it ended, or it cannot be used.
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

/// Make a proxy valid `hours` with the attributes of the VO cms, write it, and show it.
fn init(hours: u32) -> Result<bool, String> {
    let vo = "cms";
    let home = crate::samples::resolve::home().ok_or("HOME is not set to a full path, so ~/.globus cannot be found: set HOME")?;
    let globus = home.join(".globus");
    let named = |name: &str| std::env::var(name).ok();
    let source = Source::find(&globus, named("X509_USER_CERT"), named("X509_USER_KEY"))?;
    source.private()?;
    // Said before the passphrase is asked.
    let proxy = crate::samples::proxy();
    if let Some(trust) = proxy.trust() {
        eprintln!("rvr: {trust}");
    }
    let ca = proxy.ca;
    let passphrase = match source.encrypted()? {
        true => {
            voms_proxy::user::passphrase(&format!("Passphrase for {}: ", source.key().display()))
                .map_err(|e| format!("the passphrase could not be read ({}): run rvr proxy init at a terminal", rowver::described(&e)))?
        }
        false => String::new(),
    };
    let user = source.read(&passphrase);
    voms_proxy::wipe(&mut passphrase.into_bytes());
    let user = user?;
    let seconds = i64::from(hours) * 3600;
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
    // X509_USER_PROXY set but empty counts as not set, as it does where the proxy is looked for.
    let path = places
        .proxy_variable
        .filter(|path| !path.trim().is_empty())
        .or(places.usual_proxy)
        .ok_or("this platform has no usual place for a grid proxy; set X509_USER_PROXY")?;
    make::write(path.trim().as_ref(), &made.text).map_err(|e| rowver::plain_io(path.trim(), &e))?;
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

    // SPEC 10.4
    #[test]
    fn a_length_of_time_is_said_in_its_two_largest_units() {
        assert_eq!(left(3 * 86400 + 2 * 3600), "3 days 2 hours");
        assert_eq!(left(3600 + 60), "1 hour 1 minute");
        assert_eq!(left(30), "0 minutes");
    }
}
