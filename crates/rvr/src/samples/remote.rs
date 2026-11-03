//! Storage over HTTPS: any `root://`, `davs://` or `https://` location, through the WebDAV door
//! the WLCG registry (wlcg-doors) lists for its host. Folders are listed with one PROPFIND, and a
//! file's adler32 is asked with one HEAD (`Want-Digest`); nothing is read from any file.

use hep_root::ByteSource;
use std::collections::{BTreeSet, HashMap};
use crate::holes::{Failed, Fault, Seen};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One file in a listed folder.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
}

/// The HTTPS door and path of a storage location, or why it has none.
pub fn door(url: &str) -> Result<(String, String), String> {
    let rest = ["root://", "davs://", "https://"]
        .iter()
        .find_map(|s| url.strip_prefix(s))
        .ok_or_else(|| format!("{url:?} is not a storage URL (root://, davs:// or https://)"))?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority
        .split(':')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    let path = format!("/{}", path.trim_start_matches('/'));
    if url.starts_with("https://") || url.starts_with("davs://") {
        let port = authority
            .split(':')
            .nth(1)
            .map(|p| format!(":{p}"))
            .unwrap_or_default();
        return Ok((format!("https://{host}{port}"), path));
    }
    let doors = wlcg_doors::serving(&wlcg_doors::snapshot().doors, &host);
    let door = doors.first().ok_or_else(|| {
        format!("{host} has no WebDAV door in the WLCG registry ({}), and rvr reads storage over HTTPS only: name its files by an https:// or davs:// URL of a door that serves them, or copy them somewhere that has one", wlcg_doors::snapshot().taken)
    })?;
    Ok((format!("https://{}:{}", door.host, door.port), path))
}

/// The text inside the first `<…:name>` or `<name>` element of `xml`.
fn element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let mut at = 0;
    while let Some(found) = xml[at..].find(name) {
        let start = at + found;
        let before = xml[..start].chars().last();
        let after = xml[start + name.len()..].chars().next();
        if matches!(before, Some(':') | Some('<')) && matches!(after, Some('>') | Some(' ')) {
            let open = xml[start..].find('>')? + start + 1;
            let close = xml[open..].find("</")? + open;
            return Some(&xml[open..close]);
        }
        at = start + name.len();
    }
    None
}

/// The files (not folders) of a PROPFIND answer, sorted by name.
fn entries(xml: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for block in xml.split("response>").filter(|b| b.contains("href>")) {
        let Some(href) = element(block, "href") else {
            continue;
        };
        if element(block, "resourcetype").is_some_and(|t| t.contains("collection")) {
            continue;
        }
        let name = href
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(href);
        let bytes = element(block, "getcontentlength")
            .or_else(|| element(block, "size"))
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        entries.push(Entry {
            name: name.to_string(),
            bytes,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// `location` as an absolute URL, read against the URL that answered with it (RFC 3986 §5.2,
/// without dot segments, which storage doors do not send).
fn absolute(base: &str, location: &str) -> String {
    let scheme = location.find(':').map(|i| &location[..i]);
    if scheme.is_some_and(|s| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    }) {
        return location.to_string();
    }
    let after_scheme = base.find("://").map_or(0, |i| i + 3);
    let authority_end = base[after_scheme..]
        .find(['/', '?', '#'])
        .map_or(base.len(), |i| after_scheme + i);
    let path_end = base[authority_end..]
        .find(['?', '#'])
        .map_or(base.len(), |i| authority_end + i);
    if location.starts_with("//") {
        return format!("{}{location}", &base[..after_scheme.saturating_sub(2)]);
    }
    if location.starts_with('/') {
        return format!("{}{location}", &base[..authority_end]);
    }
    if location.starts_with('?') {
        return format!("{}{location}", &base[..path_end]);
    }
    let dir = base[authority_end..path_end]
        .rsplit_once('/')
        .map_or("", |(dir, _)| dir);
    format!("{}{dir}/{location}", &base[..authority_end])
}

/// `send` asked of `url`, following up to four redirects; the URL that answered, and its answer.
/// The grid agent follows none itself (a 3xx comes back as `Ok`), so that each read can keep the
/// disk server it was sent to.
fn follow(
    url: &str,
    send: impl Fn(&str) -> Result<ureq::Response, ureq::Error>,
) -> (String, Result<ureq::Response, ureq::Error>) {
    let mut url = url.to_string();
    for _ in 0..4 {
        let answer = send(&url);
        let location = match &answer {
            Ok(r) if (300..400).contains(&r.status()) => r.header("Location"),
            _ => None,
        };
        match location {
            Some(to) => url = absolute(&url, to),
            None => return (url, answer),
        }
    }
    let answer = send(&url);
    (url, answer)
}

// Storage under load refuses requests for minutes at a time (EOS answers 403), so a press asks
// again for about 13 minutes before it gives up on a request; resolve and check, for 50 s.
/// The seconds a press waits between tries of a request that failed at a busy storage.
pub const READING: &[u64] = &[5, 15, 30, 60, 120, 240, 300];
/// The seconds `rvr samples` waits between tries of a listing or a digest.
pub const ASKING: &[u64] = &[5, 15, 30];

/// What a read answers when it stops waiting because another input failed.
pub const STOPPED: &str = "Another input failed";

/// How long a press waits with no answer from any storage before it asks about the network.
pub const QUIET: Duration = Duration::from_secs(120);
/// How long after a first probe that found no door the network is asked again.
pub const AGAIN: Duration = Duration::from_secs(15);
/// How long the doors asked are given to take a connection.
const REACH: Duration = Duration::from_secs(5);
/// What the reads of one press share while they wait: `stop` ends every wait.
#[derive(Default)]
pub struct Watch {
    pub stop: AtomicBool,
    /// For a press, what tells the network gone from one storage down.
    heard: Option<Heard>,
}

/// What a press heard: when it began and, in milliseconds after that, when a storage last
/// answered, and how many answers there were; the doors (see `Last`); the one read asking about
/// the network; and why the network was taken to be gone.
struct Heard {
    quiet: Duration,
    again: Duration,
    began: Instant,
    answer: std::sync::atomic::AtomicU64,
    answers: std::sync::atomic::AtomicU64,
    last: Mutex<Last>,
    asking: AtomicBool,
    gone: std::sync::OnceLock<String>,
}

/// Each door by whether its own last reply was an answer, with its addresses (looked up once, when
/// it first answers); and when (in milliseconds after the press began) a probe last found no door
/// that takes a connection, with how many answers there were.
#[derive(Default)]
struct Last {
    doors: std::collections::BTreeMap<String, (bool, Option<Vec<std::net::SocketAddr>>)>,
    doubt: Option<(u64, u64)>,
}

impl Watch {
    /// A press's watch: the network asked about after `quiet` with no answer, and again after
    /// `again`.
    pub fn press(quiet: Duration, again: Duration) -> Self {
        let heard = Heard {
            quiet,
            again,
            began: Instant::now(),
            answer: Default::default(),
            answers: Default::default(),
            last: Default::default(),
            asking: AtomicBool::new(false),
            gone: Default::default(),
        };
        Watch { stop: AtomicBool::new(false), heard: Some(heard) }
    }

    /// A request came back as `reply`, from the door `door` (`host:port`) itself when it is named.
    pub fn note(&self, door: Option<&str>, reply: &Result<ureq::Response, ureq::Error>) {
        self.answered(door, matches!(reply, Ok(_) | Err(ureq::Error::Status(..))));
    }

    /// A server at `host` (`host:port`) answered.
    #[cfg(test)]
    pub fn heard_from(&self, host: &str) {
        self.answered(Some(host), true);
    }

    fn answered(&self, door: Option<&str>, answered: bool) {
        let Some(heard) = &self.heard else {
            return;
        };
        if answered {
            heard.answer.fetch_max(heard.began.elapsed().as_millis() as u64, Ordering::Relaxed);
            heard.answers.fetch_add(1, Ordering::Relaxed);
        }
        let Some(door) = door else {
            return;
        };
        let lock = || heard.last.lock().unwrap_or_else(|e| e.into_inner());
        let look_up = {
            let mut last = lock();
            if !last.doors.contains_key(door) {
                last.doors.insert(door.to_owned(), Default::default());
            }
            let (was, addresses) = last.doors.get_mut(door).expect("just put there");
            *was = answered;
            let first = answered && addresses.is_none();
            if first {
                *addresses = Some(Vec::new());
            }
            first
        };
        // Looked up once it has answered (its name resolves then), outside the lock.
        if look_up {
            use std::net::ToSocketAddrs;
            let found: Vec<_> = door.to_socket_addrs().map(Iterator::collect).unwrap_or_default();
            if let Some((_, addresses)) = lock().doors.get_mut(door) {
                *addresses = Some(found);
            }
        }
    }

    /// Whether the network is gone, asked by a read that waits; when it is, every wait stops.
    pub fn lost(&self) -> bool {
        let Some(heard) = &self.heard else {
            return false;
        };
        if heard.gone.get().is_some() {
            return true;
        }
        if heard.asking.swap(true, Ordering::AcqRel) {
            return false;
        }
        // Let go however `ask` ends.
        struct Asking<'a>(&'a AtomicBool);
        impl Drop for Asking<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _asking = Asking(&heard.asking);
        self.ask(heard)
    }

    /// Once no storage has answered for `quiet`, the doors that answered are asked: those whose
    /// last reply was an answer or, when every one fails, all of them. When none takes a
    /// connection twice, `again` apart, with no answer meanwhile, the press stops: the network is
    /// gone, or, when every door is of one site, that site down cannot be told from it.
    fn ask(&self, heard: &Heard) -> bool {
        let now = || heard.began.elapsed().as_millis() as u64;
        let answers = heard.answers.load(Ordering::Relaxed);
        if now().saturating_sub(heard.answer.load(Ordering::Relaxed)) < heard.quiet.as_millis() as u64 {
            return false;
        }
        let (names, addresses, one_site): (Vec<String>, Vec<std::net::SocketAddr>, bool) = {
            let last = heard.last.lock().unwrap_or_else(|e| e.into_inner());
            // A doubt stands only while nothing has answered since.
            let doubt = last.doubt.filter(|(_, then)| *then == answers);
            if doubt.is_some_and(|(at, _)| now().saturating_sub(at) < heard.again.as_millis() as u64) {
                return false;
            }
            let known: Vec<_> = last.doors.iter().filter(|(_, (_, addresses))| addresses.is_some()).collect();
            let up: Vec<_> = known.iter().copied().filter(|(_, (up, _))| *up).collect();
            let sites: BTreeSet<&str> = known.iter().map(|(door, _)| domain(door)).collect();
            let one_site = up.is_empty() && sites.len() == 1;
            let asked = match (up.is_empty(), known.is_empty()) {
                (false, _) => up,
                (true, false) => known,
                (true, true) => return false,
            };
            let names: Vec<String> = asked.iter().map(|(door, _)| (*door).clone()).collect();
            let addresses = asked.iter().flat_map(|(_, (_, addresses))| addresses.iter().flatten().copied()).collect();
            (names, addresses, one_site)
        };
        let found = reachable(&names, &addresses, REACH);
        let mut last = heard.last.lock().unwrap_or_else(|e| e.into_inner());
        // A door that connects, or an answer while it was asked, says the network works.
        if found || heard.answers.load(Ordering::Relaxed) > answers {
            last.doubt = None;
            heard.answer.fetch_max(now(), Ordering::Relaxed);
            return false;
        }
        if last.doubt.filter(|(_, then)| *then == answers).is_none() {
            last.doubt = Some((now(), answers));
            return false;
        }
        let said = match one_site {
            true => format!(
                "no storage has answered, and the one site this press reads takes no connection ({}): rvr cannot tell that site down from the network gone, so it stops and keeps what it read",
                names.join(", ")
            ),
            false => format!(
                "no storage has answered, and none that answered earlier in this press takes a connection now ({}): the network seems to be gone",
                names.join(", ")
            ),
        };
        let _ = heard.gone.set(said);
        self.stop.store(true, Ordering::Release);
        true
    }

    /// Why the network was taken to be gone, if it was.
    pub fn gone(&self) -> Option<String> {
        self.heard.as_ref().and_then(|heard| heard.gone.get().cloned())
    }

    /// Whether a failed request may pass when asked again (see `passing`); in a press, a name that
    /// does not resolve may too, as it does while the network is gone.
    fn passing(&self, error: &ureq::Error, proxied: bool) -> Option<String> {
        let unresolved = matches!(error, ureq::Error::Transport(failure) if failure.kind() == ureq::ErrorKind::Dns);
        passing(error, proxied).or_else(|| (self.heard.is_some() && unresolved).then(|| why(error)))
    }
}

/// The `host:port` of an `https://` URL as `door` gives it.
pub fn host_port(url: &str) -> Option<String> {
    let authority = url.strip_prefix("https://")?.split('/').next()?.to_ascii_lowercase();
    match authority.contains(':') {
        true => Some(authority),
        false => Some(format!("{authority}:443")),
    }
}

/// The site a door (`host:port`) is at: the last two labels of its name (`fnal.gov`), or its address.
fn domain(door: &str) -> &str {
    let host = door.rsplit_once(':').map_or(door, |(host, _)| host);
    let numeric = host.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '[' || c == ']' || c == ':');
    match host.rmatch_indices('.').nth(1) {
        Some((at, _)) if !numeric => &host[at + 1..],
        _ => host,
    }
}

/// Whether any of `addresses`, or of the addresses `names` (`host:port`) have now, takes a TCP
/// connection within `within`, every one asked at once.
fn reachable(names: &[String], addresses: &[std::net::SocketAddr], within: Duration) -> bool {
    use std::net::ToSocketAddrs;
    let (said, answers) = std::sync::mpsc::channel();
    let connect = move |address: std::net::SocketAddr, said: std::sync::mpsc::Sender<bool>| {
        std::thread::spawn(move || {
            let _ = said.send(std::net::TcpStream::connect_timeout(&address, within).is_ok());
        });
    };
    for &address in addresses {
        connect(address, said.clone());
    }
    // A name looked up again, in case its addresses moved; one slow to resolve is left to finish.
    for name in names {
        let (name, said) = (name.clone(), said.clone());
        std::thread::spawn(move || {
            let fresh = name.to_socket_addrs().into_iter().flatten();
            for address in fresh {
                connect(address, said.clone());
            }
        });
    }
    drop(said);
    let until = Instant::now() + within;
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match answers.recv_timeout(left) {
            Ok(true) => return true,
            Ok(false) => continue,
            Err(_) => return false,
        }
    }
    false
}

/// Why a failed request may succeed when asked again (refused or failed at a busy storage, or
/// lost on the way), or `None` when it will not: a 401 or 403 without a proxy, a certificate
/// not trusted, a name that does not resolve, a URL that cannot be asked.
fn passing(error: &ureq::Error, proxied: bool) -> Option<String> {
    use ureq::ErrorKind::{BadHeader, BadStatus, ConnectionFailed, Io};
    match error {
        ureq::Error::Status(403, _) if !proxied => None,
        ureq::Error::Status(403 | 408 | 429 | 500..=599, _) => Some(why(error)),
        ureq::Error::Transport(failure)
            if grid_trust::tls_error(failure).is_none()
                && matches!(
                    failure.kind(),
                    ConnectionFailed | BadStatus | BadHeader | Io
                ) =>
        {
            Some(why(error))
        }
        _ => None,
    }
}

/// `ask` until it succeeds, retrying after each of `waits` while `passing` says it may, until the
/// watch stops.
pub fn patiently<T, E>(
    waits: &[u64],
    watch: &Watch,
    what: &str,
    passing: impl Fn(&E) -> Option<String>,
    mut ask: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut left = waits.iter().enumerate();
    loop {
        let error = match ask() {
            Ok(answer) => return Ok(answer),
            Err(error) => error,
        };
        let (Some(why), Some((tried, wait))) = (passing(&error), left.next()) else {
            return Err(error);
        };
        crate::progress::say(&format!(
            "  waiting {wait} s to ask again for {what} ({} of {} tries made): {why}",
            tried + 1,
            waits.len() + 1
        ));
        for _ in 0..*wait {
            if watch.stop.load(Ordering::Acquire) || watch.lost() {
                return Err(error);
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

/// How long `waits` waits in all, in words.
pub fn waited(waits: &[u64]) -> String {
    let seconds: u64 = waits.iter().sum();
    match seconds {
        0..120 => format!("{} tries over {seconds} s", waits.len() + 1),
        _ => format!(
            "{} tries over {} minutes",
            waits.len() + 1,
            (seconds + 30) / 60
        ),
    }
}

/// A remote read, retried while it fails with `Error::Remote`; `STOPPED` once the watch stops.
pub fn read_again<T>(
    waits: &[u64],
    watch: &Watch,
    shown: &str,
    read: impl FnMut() -> hep_root::Result<T>,
) -> hep_root::Result<T> {
    let name = shown.rsplit('/').next().unwrap_or(shown);
    let passing = |e: &hep_root::Error| match e {
        hep_root::Error::Remote(why) => Some(
            why.strip_prefix(&format!("{shown}: "))
                .unwrap_or(why)
                .into(),
        ),
        _ => None,
    };
    patiently(waits, watch, name, passing, read).map_err(|e| match e {
        _ if watch.stop.load(Ordering::Acquire) => hep_root::Error::Remote(STOPPED.into()),
        hep_root::Error::Remote(why) if !waits.is_empty() => {
            hep_root::Error::Remote(format!("{why} ({})", waited(waits)))
        }
        e => e,
    })
}

/// HTTPS to storage doors with the grid proxy, when there is one; folder listings are kept.
pub struct Storage {
    agent: ureq::Agent,
    listed: Mutex<HashMap<String, Result<Vec<Entry>, String>>>,
    pub proxy: Option<String>,
    /// How long a listing or digest that failed waits before it is asked again (`ASKING`).
    pub waits: &'static [u64],
    /// What ends those waits early (a press's, which stops when an input fails).
    pub watch: Arc<Watch>,
}

impl Storage {
    pub fn new(proxy: Option<&str>, ca: Option<&str>) -> Result<Self, String> {
        let loaded = match proxy {
            Some(p) => Some(
                voms_proxy::load(p)
                    .map_err(|(_, why)| format!("the grid proxy {p} could not be read: {why}"))?,
            ),
            None => None,
        };
        let agent =
            grid_trust::agent(grid_trust::roots(ca).0, loaded).map_err(|e| e.to_string())?;
        Ok(Storage {
            agent,
            listed: Mutex::new(HashMap::new()),
            proxy: proxy.map(str::to_owned),
            waits: ASKING,
            watch: Default::default(),
        })
    }

    /// The files of a storage folder (not its folders), listed once and then remembered.
    pub fn list(&self, folder: &str) -> Result<Vec<Entry>, String> {
        if let Some(done) = self.listed.lock().unwrap().get(folder) {
            return done.clone();
        }
        let result = self.list_now(folder);
        self.listed
            .lock()
            .unwrap()
            .insert(folder.to_string(), result.clone());
        result
    }

    fn list_now(&self, folder: &str) -> Result<Vec<Entry>, String> {
        let (origin, path) = door(folder)?;
        let path = format!("{}/", path.trim_end_matches('/'));
        let body = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/></D:prop></D:propfind>"#;
        let at = format!("{origin}{path}");
        let (proxied, door) = (self.proxy.is_some(), host_port(&origin));
        let reply = patiently(
            self.waits,
            &self.watch,
            folder,
            |e| self.watch.passing(e, proxied),
            || {
                follow(&at, |url| {
                    let reply = self
                        .agent
                        .request("PROPFIND", url)
                        .set("Depth", "1")
                        .set("Content-Type", "application/xml")
                        .send_string(body);
                    self.watch.note(door.as_deref().filter(|_| url == at), &reply);
                    reply
                })
                .1
            },
        );
        if reply.is_err() && self.watch.stop.load(Ordering::Acquire) {
            return Err(format!("{folder}: {STOPPED}"));
        }
        let text = match reply {
            Ok(r) if r.status() / 100 == 2 => r.into_string().map_err(|e| e.to_string())?,
            Ok(r) => {
                return Err(format!(
                    "{folder} could not be listed: its storage answered {} {} (redirected too \
                     often, or not with a listing). Try again later; if it persists, the folder \
                     may have moved.",
                    r.status(),
                    r.status_text()
                ))
            }
            Err(ureq::Error::Status(404, _)) => {
                return Err(format!("the folder {folder} does not exist"))
            }
            Err(ureq::Error::Status(code @ (401 | 403), _)) => {
                return Err(match (&self.proxy, code) {
                    (None, _) => format!("{folder} refused the listing ({code}). It needs a grid proxy: run rvr proxy init."),
                    (Some(_), 401) => format!("{folder} refused the listing (401): it did not accept the grid proxy. Check it with rvr proxy, and make a new one with rvr proxy init."),
                    (Some(_), _) => format!("{folder} refused the listing (403, {}). Either the grid proxy does not give access there, or the storage is refusing requests while it is busy, as EOS does under heavy load: if rvr proxy shows a valid proxy, try again later.", waited(self.waits)),
                })
            }
            Err(e) => return Err(format!("{folder} could not be listed: {}", why(&e))),
        };
        Ok(entries(&text))
    }

    /// A storage file's size and adler32 as its storage reports them, without reading it.
    pub fn digest(&self, url: &str) -> Result<Digest, Failed> {
        digest_seen(
            &self.agent,
            url,
            self.waits,
            self.proxy.is_some(),
            &self.watch,
        )
    }

    /// The connection pool, for readers that outlive this borrow.
    pub fn agent(&self) -> ureq::Agent {
        self.agent.clone()
    }
}

/// What went wrong with a request, in a few words and without its URL.
pub fn why(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, answer) => {
            format!("its storage answered {code} {}", answer.status_text())
        }
        ureq::Error::Transport(failure) => match grid_trust::tls_error(failure) {
            Some(tls) => tls.to_string(),
            None if grid_trust::timed_out(failure) => "it did not answer in time".into(),
            None => {
                let source = std::error::Error::source(failure).map(|s| format!(": {s}"));
                let said = failure.message().map(|m| format!(": {m}"));
                format!(
                    "{}{}{}",
                    failure.kind(),
                    said.unwrap_or_default(),
                    source.unwrap_or_default()
                )
            }
        },
    }
}

/// A file's size and adler32, as its storage reports them.
pub type Digest = (Option<u64>, Option<String>);

/// A storage file's size and adler32 as its storage reports them, asked with one HEAD (asked
/// again after each of `waits` while it fails at a busy storage); a failure also says what it
/// means for the file.
pub fn digest_seen(
    agent: &ureq::Agent,
    url: &str,
    waits: &[u64],
    proxied: bool,
    watch: &Watch,
) -> Result<Digest, Failed> {
    // The agreement check found every host's door: one gone since is a storage that cannot be reached.
    let (origin, path) = door(url).map_err(|why| Failed::new(Fault::Unreachable, why))?;
    let at = format!("{origin}{path}");
    let reply = patiently(waits, watch, url, |e| watch.passing(e, proxied), || head(agent, &at, watch));
    if reply.is_err() && watch.stop.load(Ordering::Acquire) {
        return Err(Failed::new(Fault::Stopped, format!("{url}: {STOPPED}")));
    }
    let seen = reply.as_ref().err().map_or(Fault::Unreachable, |e| fault(e, proxied));
    digested(url, reply).map_err(|why| Failed::new(seen, why))
}

/// What a failed request means for its file.
// Gone (404), refused (403 with a proxy), not the file's fault (401; a 403 with no proxy, which
// needs one; a certificate rvr does not trust), or unreachable.
pub fn fault(error: &ureq::Error, proxied: bool) -> Fault {
    match error {
        ureq::Error::Status(404 | 410, _) => Fault::Gone,
        ureq::Error::Status(401, _) => Fault::Save,
        ureq::Error::Status(403, _) if proxied => Fault::Refused,
        ureq::Error::Status(403, _) => Fault::Save,
        ureq::Error::Status(..) => Fault::Unreachable,
        ureq::Error::Transport(failure) if grid_trust::tls_error(failure).is_some() => Fault::Save,
        ureq::Error::Transport(_) => Fault::Unreachable,
    }
}

/// One HEAD asking for the adler32, redirects followed.
#[allow(clippy::result_large_err)] // ureq's own error, as `follow` returns it
fn head(agent: &ureq::Agent, at: &str, watch: &Watch) -> Result<ureq::Response, ureq::Error> {
    let door = host_port(at);
    follow(at, |hop| {
        let reply = agent.head(hop).set("Want-Digest", "adler32").call();
        watch.note(door.as_deref().filter(|_| hop == at), &reply);
        reply
    })
    .1
}

/// The size and adler32 a HEAD's answer gives.
fn digested(
    url: &str,
    reply: Result<ureq::Response, ureq::Error>,
) -> Result<(Option<u64>, Option<String>), String> {
    let reply = reply.map_err(|e| match e {
        ureq::Error::Status(404, _) => format!("{url} no longer exists"),
        e => format!("{url} could not be asked: {}", why(&e)),
    })?;
    if reply.status() / 100 != 2 {
        return Err(format!(
            "{url} could not be asked: its storage answered {} {} (redirected too often, or \
             not with the file). Try again later; if it persists, the file may have moved.",
            reply.status(),
            reply.status_text()
        ));
    }
    let bytes = reply.header("Content-Length").and_then(|s| s.parse().ok());
    Ok((bytes, reply.header("Digest").and_then(adler32_of)))
}

/// The adler32 a `Digest` header gives, as 8 lowercase hex digits whatever case or length the
/// storage wrote it in (`adler32=3DA0195` is `03da0195`).
fn adler32_of(digest: &str) -> Option<String> {
    digest
        .split(',')
        .find_map(|p| p.trim().strip_prefix("adler32="))
        .and_then(|s| u32::from_str_radix(s.trim(), 16).ok())
        .map(|sum| format!("{sum:08x}"))
}

/// A storage file read in byte ranges over HTTPS, for hep-root: the door's URL first, then the
/// disk server it redirects to. Only the ranges hep-root asks for are read.
pub struct HttpsFile {
    agent: ureq::Agent,
    door: String,
    url: Mutex<String>,
    /// Whether the storage may answer several ranges in one request; false once it has not.
    multipart: std::sync::atomic::AtomicBool,
    shown: String,
    len: u64,
    /// Whether a proxy is presented, so that a 403 may be the storage being busy.
    proxied: bool,
    /// Requests made and bytes read, for the timing the resolver reports.
    pub requests: std::sync::atomic::AtomicUsize,
    pub bytes: std::sync::atomic::AtomicUsize,
    /// What its last failed request said, shared with whoever reads through it.
    pub seen: Arc<Seen>,
    /// The press's, which hears each answer, and the door's `host:port` it is noted under.
    watch: Arc<Watch>,
    host: Option<String>,
}

impl HttpsFile {
    pub fn new(storage: &Storage, at: &str, len: u64) -> Result<Self, String> {
        let (origin, path) = door(at)?;
        Ok(HttpsFile {
            agent: storage.agent.clone(),
            door: format!("{origin}{path}"),
            multipart: true.into(),
            url: Mutex::new(format!("{origin}{path}")),
            shown: at.to_string(),
            len,
            proxied: storage.proxy.is_some(),
            requests: Default::default(),
            bytes: Default::default(),
            seen: Default::default(),
            watch: storage.watch.clone(),
            host: host_port(&origin),
        })
    }

    /// The same, noting its failed requests in `seen`.
    pub fn seeing(mut self, seen: &Arc<Seen>) -> Self {
        self.seen = seen.clone();
        self
    }
}

/// The most ranges one request asks for, the gap a range is joined across when each request is
/// one range, and how many requests run at once (measured on FNAL EOS, 2026-10-07).
const SPANS: usize = 256;
const GAP: u64 = 512 * 1024;
const FETCHERS: usize = 6;

/// `ask` of every item, `FETCHERS` at a time, in the items' order.
fn at_once<T: Sync, R: Send>(items: &[T], ask: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|scope| {
        for _ in 0..FETCHERS.min(items.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(item) = items.get(i) else { return };
                let answer = ask(item);
                done.lock().unwrap().push((i, answer));
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// The most bytes one joined range holds (as hep-root's `read_spans`).
const MOST: u64 = 32 * 1024 * 1024;

/// Sorted or not, ranges joined where they lie within `gap` of each other, up to `MOST` bytes
/// each, sorted.
pub fn joined(ranges: &[(u64, usize)], gap: u64) -> Vec<(u64, usize)> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (at, len) in sorted {
        let end = at + len as u64;
        match out.last_mut() {
            Some((first, last)) if at <= *last + gap && end.max(*last) - *first <= MOST => {
                *last = end.max(*last)
            }
            _ => out.push((at, end)),
        }
    }
    out.into_iter()
        .map(|(s, e)| (s, (e - s) as usize))
        .collect()
}

impl HttpsFile {
    /// A failed request as hep-root takes it: `Remote` when asking again may mend it (see
    /// `read_again`), the file gone, or any other failure, which waiting does not mend.
    fn failed(&self, error: &ureq::Error) -> hep_root::Error {
        self.seen.note(fault(error, self.proxied));
        match error {
            ureq::Error::Status(404 | 410, _) => {
                hep_root::Error::SourceChanged(format!("{} no longer exists", self.shown))
            }
            _ if self.watch.passing(error, self.proxied).is_some() => {
                hep_root::Error::Remote(format!("{}: {}", self.shown, why(error)))
            }
            _ => self.lasting(why(error)),
        }
    }

    /// A failure asking again would not mend.
    fn lasting(&self, why: String) -> hep_root::Error {
        hep_root::Error::Io(std::io::Error::other(format!("{}: {why}", self.shown)))
    }

    /// Ranges joined across gaps and asked one per request, `FETCHERS` at once.
    fn one_at_a_time(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        let joined = joined(ranges, GAP);
        let read = at_once(&joined, |&(at, len)| {
            let mut buf = vec![0; len];
            self.read_at(at, &mut buf).map(|()| buf)
        });
        let read = read
            .into_iter()
            .collect::<hep_root::Result<Vec<Vec<u8>>>>()?;
        Ok(ranges
            .iter()
            .map(|&(at, len)| {
                let (start, bytes) = joined
                    .iter()
                    .zip(&read)
                    .map(|((s, _), b)| (*s, b))
                    .rfind(|(s, _)| *s <= at)
                    .expect("every range lies in a joined one");
                let from = (at - start) as usize;
                bytes[from..from + len].to_vec()
            })
            .collect())
    }

    /// Several ranges in one request; `None` when the storage does not answer them as
    /// `multipart/byteranges`, so that they are asked one at a time instead.
    fn fetch_many(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Option<Vec<Vec<u8>>>> {
        // An empty range needs no asking, and has no place in a Range header.
        let asked: Vec<(u64, usize)> = ranges.iter().copied().filter(|(_, len)| *len > 0).collect();
        if asked.len() < ranges.len() {
            let mut read = self.fetch_many(&asked)?.map(Vec::into_iter);
            return Ok(ranges
                .iter()
                .map(|(_, len)| match len {
                    0 => Some(Vec::new()),
                    _ => read.as_mut().and_then(Iterator::next),
                })
                .collect());
        }
        if ranges.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if let [(at, len)] = ranges {
            let mut buf = vec![0; *len];
            return self.read_at(*at, &mut buf).map(|()| Some(vec![buf]));
        }
        let fail = |e: String| hep_root::Error::Remote(format!("{}: {e}", self.shown));
        let asked: Vec<String> = ranges
            .iter()
            .filter(|(_, len)| *len > 0)
            .map(|(at, len)| format!("{at}-{}", at + *len as u64 - 1))
            .collect();
        let header = format!("bytes={}", asked.join(","));
        let ask = |url: &str| {
            follow(url, |url| {
                self.requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let reply = self.agent.get(url).set("Range", &header).call();
                self.watch.note(self.host.as_deref().filter(|_| url == self.door), &reply);
                reply
            })
        };
        let start = self.url.lock().unwrap().clone();
        let (mut to, mut reply) = ask(&start);
        // As for one range: a disk server kept from an earlier read may have expired.
        if start != self.door && !matches!(&reply, Ok(r) if r.status() == 206) {
            (to, reply) = ask(&self.door);
        }
        let reply = match reply {
            Ok(reply) => reply,
            // A storage that refuses so many ranges at once is asked one at a time.
            Err(ureq::Error::Status(400 | 416, _)) => return Ok(None),
            Err(e) => return Err(self.failed(&e)),
        };
        *self.url.lock().unwrap() = to;
        let boundary = reply
            .header("Content-Type")
            .and_then(super::multipart::boundary)
            .filter(|_| reply.status() == 206 && reply.content_type() == "multipart/byteranges");
        let total: u64 = ranges.iter().map(|(_, len)| *len as u64).sum();
        let parts = match boundary {
            Some(boundary) => {
                super::multipart::parts(reply.into_reader(), &boundary, total).map_err(fail)?
            }
            // A storage may join every range asked into one: one part, its place in Content-Range.
            None if reply.status() == 206 => {
                let Some((at, len)) = reply
                    .header("Content-Range")
                    .and_then(super::multipart::range)
                    .map(|(at, len, _)| (at, len))
                else {
                    return Ok(None);
                };
                if len > total.saturating_mul(2).saturating_add(1 << 20) {
                    return Ok(None);
                }
                let mut bytes = vec![0; len as usize];
                std::io::Read::read_exact(&mut reply.into_reader(), &mut bytes)
                    .map_err(|e| fail(e.to_string()))?;
                vec![super::multipart::Part {
                    at,
                    whole: None,
                    bytes,
                }]
            }
            None => return Ok(None),
        };
        self.bytes
            .fetch_add(total as usize, std::sync::atomic::Ordering::Relaxed);
        if std::env::var_os("RVR_TRACE").is_some() {
            eprintln!(
                "  GET {} ranges ({total} bytes) of {}",
                ranges.len(),
                self.shown.rsplit('/').next().unwrap_or("")
            );
        }
        ranges
            .iter()
            .map(|&(at, len)| {
                // A storage may answer two ranges as one part; any part holding the range serves.
                parts
                    .iter()
                    .find_map(|part| {
                        let from = usize::try_from(at.checked_sub(part.at)?).ok()?;
                        part.bytes.get(from..from.checked_add(len)?)
                    })
                    .map(<[u8]>::to_vec)
            })
            // A storage that answered only some of the ranges is asked again a range at a time.
            .collect::<Option<Vec<_>>>()
            .map_or(Ok(None), |read| Ok(Some(read)))
    }
}

impl ByteSource for HttpsFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let range = format!("bytes={offset}-{}", offset + buf.len() as u64 - 1);
        let ask = |url: &str| {
            follow(url, |url| {
                self.requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let reply = self.agent.get(url).set("Range", &range).call();
                self.watch.note(self.host.as_deref().filter(|_| url == self.door), &reply);
                reply
            })
        };
        let start = self.url.lock().unwrap().clone();
        let (mut to, mut reply) = ask(&start);
        // A disk server kept from an earlier read can expire (its token) or go down: ask the door again.
        let failed = !matches!(&reply, Ok(r) if r.status() == 206);
        if failed && start != self.door {
            (to, reply) = ask(&self.door);
        }
        *self.url.lock().unwrap() = to;
        match reply {
            Ok(reply) if reply.status() == 206 => {
                use std::io::Read;
                self.bytes
                    .fetch_add(buf.len(), std::sync::atomic::Ordering::Relaxed);
                if std::env::var_os("RVR_TRACE").is_some() {
                    eprintln!(
                        "  GET {range} ({} bytes) of {}",
                        buf.len(),
                        self.shown.rsplit('/').next().unwrap_or("")
                    );
                }
                reply
                    .into_reader()
                    .read_exact(buf)
                    .map_err(|e| {
                        self.seen.note(Fault::Unreachable);
                        hep_root::Error::Remote(format!("{}: {e}", self.shown))
                    })
            }
            Ok(reply) if (300..400).contains(&reply.status()) => {
                self.seen.note(Fault::Unreachable);
                Err(hep_root::Error::Remote(format!(
                    "{}: its storage redirected more than four times, or without saying where to",
                    self.shown
                )))
            }
            Ok(reply) => Err({
                self.seen.note(Fault::Unreachable);
                self.lasting(format!(
                    "asked for bytes {range}, got status {} (the storage ignored the range)",
                    reply.status()
                ))
            }),
            Err(e) => Err(self.failed(&e)),
        }
    }

    fn describe(&self) -> String {
        self.shown.clone()
    }

    /// Several ranges: up to 256 in one request where the storage answers
    /// `multipart/byteranges` (EOS does), six such requests at once; otherwise joined across
    /// gaps and asked for one range a request, six at once.
    fn read_ranges(&self, ranges: &[(u64, usize)]) -> hep_root::Result<Vec<Vec<u8>>> {
        if !self.multipart.load(std::sync::atomic::Ordering::Relaxed) {
            return self.one_at_a_time(ranges);
        }
        let batches: Vec<&[(u64, usize)]> = ranges.chunks(SPANS).collect();
        let answers = at_once(&batches, |b| self.fetch_many(b));
        let mut out: Vec<Option<Vec<u8>>> = Vec::with_capacity(ranges.len());
        for (batch, answer) in batches.iter().zip(answers) {
            match answer? {
                Some(read) => out.extend(read.into_iter().map(Some)),
                None => out.extend(batch.iter().map(|_| None)),
            }
        }
        // What was not answered as asked: every such range, and every later read, a range at a
        // time, joined across batches.
        let missing: Vec<usize> = (0..ranges.len()).filter(|&i| out[i].is_none()).collect();
        if !missing.is_empty() {
            self.multipart
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let asked: Vec<(u64, usize)> = missing.iter().map(|&i| ranges[i]).collect();
            for (i, read) in missing.into_iter().zip(self.one_at_a_time(&asked)?) {
                out[i] = Some(read);
            }
        }
        Ok(out
            .into_iter()
            .map(|r| r.expect("every range read"))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_keeps_files_by_resourcetype() {
        let xml = r#"<D:multistatus xmlns:D="DAV:">
<D:response><D:href>/store/collections/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>
<D:response><D:href>/store/collections/sub/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>
<D:response><D:href>/store/collections/collection_2024.root</D:href><D:propstat><D:prop><D:resourcetype/><D:getcontentlength>12</D:getcontentlength></D:prop></D:propstat></D:response>
<D:response><D:href>/store/collections/a%2Bb%20c.root</D:href><D:propstat><D:prop><D:resourcetype></D:resourcetype><D:getcontentlength>7</D:getcontentlength><D:getlastmodified>Thu, 25 Jun 2026 23:05:06 GMT</D:getlastmodified></D:prop></D:propstat></D:response>
</D:multistatus>"#;
        let found = entries(xml);
        let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a%2Bb%20c.root", "collection_2024.root"]);
        assert_eq!(found[0].bytes, 7);
        assert_eq!(adler32_of("md5=x, adler32=3DA0195").as_deref(), Some("03da0195"));
        assert_eq!(adler32_of("adler32=not-hex"), None);
    }

    #[test]
    fn ranges_join_across_gaps_but_not_past_the_most_one_holds() {
        assert_eq!(
            joined(&[(100, 10), (0, 10), (15, 5)], 10),
            [(0, 20), (100, 10)]
        );
        let big = (MOST / 2) as usize;
        assert_eq!(
            joined(&[(0, big), (big as u64, big), (2 * big as u64, 1)], 0).len(),
            2
        );
    }

    /// A read that fails its first `failing` tries with `error`, counting its tries.
    fn flaky(
        failing: usize,
        error: fn() -> hep_root::Error,
        tries: &std::cell::Cell<usize>,
    ) -> impl FnMut() -> hep_root::Result<()> + '_ {
        move || {
            tries.set(tries.get() + 1);
            match tries.get() <= failing {
                true => Err(error()),
                false => Ok(()),
            }
        }
    }


    #[test]
    fn a_read_refused_by_a_busy_storage_is_asked_again_until_the_waits_run_out() {
        let shown = "https://door.example/f.root";
        let busy = || {
            hep_root::Error::Remote(
                "https://door.example/f.root: its storage answered 403 Forbidden".into(),
            )
        };
        let never = Watch::default();
        let tries = std::cell::Cell::new(0);
        assert!(read_again(&[0, 0, 0], &never, shown, flaky(2, busy, &tries)).is_ok());
        assert_eq!(tries.get(), 3);

        let tries = std::cell::Cell::new(0);
        let error = read_again(&[0, 0], &never, shown, flaky(9, busy, &tries)).unwrap_err();
        assert_eq!(tries.get(), 3);
        let said = error.to_string();
        assert!(said.ends_with("403 Forbidden (3 tries over 0 s)"), "{said}");

        // A failure waiting does not mend is not asked again.
        let lasting = || hep_root::Error::Io(std::io::Error::other("UnknownIssuer"));
        let tries = std::cell::Cell::new(0);
        assert!(read_again(&[0, 0], &never, shown, flaky(1, lasting, &tries)).is_err());
        assert_eq!(tries.get(), 1);

        // Another input failing ends the waiting.
        let stop = Watch::default();
        stop.stop.store(true, Ordering::Relaxed);
        let tries = std::cell::Cell::new(0);
        let error = read_again(&[1, 1], &stop, shown, flaky(9, busy, &tries)).unwrap_err();
        assert_eq!(
            (tries.get(), error.to_string().ends_with(STOPPED)),
            (1, true)
        );
    }

    #[test]
    fn a_refusal_is_asked_again_only_where_it_may_pass() {
        let refused = |code| ureq::Error::Status(code, ureq::Response::new(code, "", "").unwrap());
        assert!(passing(&refused(403), true).is_some());
        assert!(
            passing(&refused(403), false).is_none(),
            "no proxy: it needs one"
        );
        assert!(passing(&refused(503), false).is_some());
        assert!(passing(&refused(401), true).is_none());
        assert!(passing(&refused(404), true).is_none());
        // A URL that cannot be asked fails before any request is made.
        let unaskable = ureq::get("https://exa mple/").call().unwrap_err();
        assert!(passing(&unaskable, true).is_none(), "{unaskable}");
    }

    #[test]
    fn the_network_is_gone_only_when_no_host_that_answered_can_be_reached() {
        assert_eq!(host_port("https://eos.example:9000/store/f.root").as_deref(), Some("eos.example:9000"));
        assert_eq!(host_port("https://Door.Example").as_deref(), Some("door.example:443"));
        assert_eq!((domain("cmseos.fnal.gov:443"), domain("eos.cms.rcac.purdue.edu:9000")), ("fnal.gov", "purdue.edu"));
        assert_eq!((domain("127.0.0.1:0"), domain("[::1]:0")), ("127.0.0.1", "[::1]"));
        // A door that takes connections, and two that never do (port 0).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let door = listener.local_addr().unwrap().to_string();
        let (closed, shut) = ("127.0.0.1:0", "[::1]:0");
        let unanswered = ureq::get("https://exa mple/").call();
        let busy = || hep_root::Error::Remote("https://door.example/f.root: its storage answered 503".into());
        let shown = "https://door.example/f.root";
        let watch = |quiet| Watch::press(quiet, Duration::ZERO);
        // Something answered moments ago: the network is not asked about.
        let recent = watch(Duration::from_secs(60));
        recent.heard_from(&door);
        assert!(!recent.lost());
        // No door that answered: no telling the network from one storage down, and the files
        // become holes after their waits.
        let (silent, tries) = (watch(Duration::ZERO), std::cell::Cell::new(0));
        assert!(read_again(&[0, 0], &silent, shown, flaky(9, busy, &tries)).is_err());
        assert!(!silent.lost() && silent.gone().is_none() && tries.get() == 3);
        // One site whose doors all fail and take no connection, twice: that site down cannot be
        // told from the network gone, so the press stops and keeps what it read.
        let cut = watch(Duration::ZERO);
        cut.heard_from(closed);
        cut.note(Some(closed), &unanswered);
        assert!(!cut.lost(), "a first probe that finds none is asked again");
        assert!(cut.lost());
        let why = cut.gone().unwrap();
        assert!(why.contains("the one site this press reads takes no connection (127.0.0.1:0): rvr cannot tell that site down from the network gone"), "{why}");
        // Doors that never answered have no addresses to ask: no telling either.
        let never = watch(Duration::ZERO);
        never.note(Some(closed), &unanswered);
        never.note(Some(shut), &unanswered);
        assert!(!never.lost() && !never.lost() && never.gone().is_none());
        // A door that answered and takes a connection says the network works.
        let works = watch(Duration::ZERO);
        works.heard_from(&door);
        works.note(Some(closed), &unanswered);
        assert!(!works.lost() && !works.stop.load(Ordering::Acquire));
        // Two doors of one site, both failing, whose names no longer resolve: that site down
        // cannot be told from the network gone, so the press stops and keeps what it read.
        let site = watch(Duration::ZERO);
        for door in ["a.site.example:0", "b.site.example:0"] {
            site.heard_from(door);
            site.note(Some(door), &unanswered);
        }
        assert!(!site.lost(), "a first probe that finds none is asked again");
        assert!(site.lost() && site.gone().unwrap().contains("cannot tell that site down"));
        // Two doors of two sites, both failing, neither taking a connection twice: the network
        // is gone, and a wait ends then. An answer between two probes would have cleared the first.
        let gone = watch(Duration::ZERO);
        for door in [closed, shut] {
            gone.heard_from(door);
            gone.note(Some(door), &unanswered);
        }
        assert!(!gone.lost(), "a first probe that finds none is asked again");
        gone.heard_from(closed);
        gone.note(Some(closed), &unanswered);
        assert!(!gone.lost(), "an answer since: asked again from the start");
        let (tries, started) = (std::cell::Cell::new(0), Instant::now());
        assert!(read_again(&[30, 30], &gone, shown, flaky(9, busy, &tries)).is_err());
        assert!(started.elapsed() < Duration::from_secs(10) && tries.get() == 1);
        assert!(gone.stop.load(Ordering::Acquire));
        let why = gone.gone().unwrap();
        assert!(why.contains("none that answered earlier in this press takes a connection now (") && why.contains(closed), "{why}");
        // In a press, a name that does not resolve waits as a refused connection does (where this
        // machine's resolver says so).
        let unresolved = ureq::get("https://nowhere.invalid/").call().unwrap_err();
        drop(listener);
        if matches!(&unresolved, ureq::Error::Transport(f) if f.kind() == ureq::ErrorKind::Dns) {
            assert!(Watch::default().passing(&unresolved, true).is_none());
            assert!(works.passing(&unresolved, true).is_some());
        } else {
            eprintln!("not checked here: nowhere.invalid did not fail as a name that does not resolve ({unresolved})");
        }
    }

    #[test]
    fn waits_are_said_in_seconds_or_minutes() {
        assert_eq!(waited(ASKING), "4 tries over 50 s");
        assert_eq!(waited(READING), "8 tries over 13 minutes");
    }

    #[test]
    fn redirects_are_read_against_the_url_that_sent_them() {
        let base = "https://door.example:1094/eos/a/file.root?x=1";
        assert_eq!(
            absolute(base, "https://disk.example:8443/f?tok=2"),
            "https://disk.example:8443/f?tok=2"
        );
        assert_eq!(
            absolute(base, "/eos/b/file.root?tok=2"),
            "https://door.example:1094/eos/b/file.root?tok=2"
        );
        assert_eq!(
            absolute(base, "other.root"),
            "https://door.example:1094/eos/a/other.root"
        );
    }
}
