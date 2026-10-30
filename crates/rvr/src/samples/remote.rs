//! Storage over HTTPS: any `root://`, `davs://` or `https://` location, through the WebDAV door
//! the WLCG registry (wlcg-doors) lists for its host. Folders are listed with one PROPFIND, and a
//! file's adler32 is asked with one HEAD (`Want-Digest`); nothing is read from any file.

use hep_root::ByteSource;
use std::collections::HashMap;
use crate::holes::{Failed, Fault, Seen};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One file in a listed folder.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    pub modified: Option<String>,
}

/// The HTTPS door and path of a storage location, or why it has none.
pub fn door(url: &str) -> Result<(String, String), String> {
    let rest = ["root://", "davs://", "https://", "xroot://"]
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
        format!("{host} has no WebDAV door in the WLCG registry ({}), so its folders cannot be listed from here. Name its files one by one, or copy them somewhere listable.", wlcg_doors::snapshot().taken)
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Thu, 25 Jun 2026 23:05:06 GMT" as "2026-06-25T23:05:06Z".
fn rfc3339(http_date: &str) -> Option<String> {
    let parts: Vec<&str> = http_date.split_whitespace().collect();
    let [_, day, month, year, time, ..] = parts.as_slice() else {
        return None;
    };
    let month = MONTHS.iter().position(|m| m == month)? + 1;
    Some(format!(
        "{year}-{month:02}-{:02}T{time}Z",
        day.parse::<u32>().ok()?
    ))
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
            modified: element(block, "getlastmodified").and_then(rfc3339),
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

/// What the reads of one press share while they wait: `stop` ends every wait.
#[derive(Default)]
pub struct Watch {
    pub stop: AtomicBool,
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
        println!(
            "  waiting {wait} s to ask again for {what} ({} of {} tries made): {why}",
            tried + 1,
            waits.len() + 1
        );
        for _ in 0..*wait {
            if watch.stop.load(Ordering::Acquire) {
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
        let body = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/><D:getlastmodified/></D:prop></D:propfind>"#;
        let at = format!("{origin}{path}");
        let proxied = self.proxy.is_some();
        let reply = patiently(
            self.waits,
            &self.watch,
            folder,
            |e| passing(e, proxied),
            || {
                follow(&at, |url| {
                    self.agent
                        .request("PROPFIND", url)
                        .set("Depth", "1")
                        .set("Content-Type", "application/xml")
                        .send_string(body)
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
    pub fn digest(&self, url: &str) -> Result<(Option<u64>, Option<String>), String> {
        digest(
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

/// A storage file's size and adler32 as its storage reports them, asked with one HEAD (asked
/// again after each of `waits` while it fails at a busy storage).
pub fn digest(
    agent: &ureq::Agent,
    url: &str,
    waits: &[u64],
    proxied: bool,
    watch: &Watch,
) -> Result<Digest, String> {
    digest_seen(agent, url, waits, proxied, watch).map_err(|failed| failed.why)
}

/// A file's size and adler32, as its storage reports them.
pub type Digest = (Option<u64>, Option<String>);

/// As `digest`, a failure also saying what it means for the file.
pub fn digest_seen(
    agent: &ureq::Agent,
    url: &str,
    waits: &[u64],
    proxied: bool,
    watch: &Watch,
) -> Result<Digest, Failed> {
    let (origin, path) = door(url).map_err(Failed::from)?;
    let at = format!("{origin}{path}");
    let reply = patiently(waits, watch, url, |e| passing(e, proxied), || head(agent, &at));
    if reply.is_err() && watch.stop.load(Ordering::Acquire) {
        return Err(Failed::new(Fault::Stopped, format!("{url}: {STOPPED}")));
    }
    let seen = reply.as_ref().err().map_or(Fault::Unreachable, |e| fault(e, proxied));
    digested(url, reply).map_err(|why| Failed::new(seen, why))
}

/// Why a file was not read, what that means for it, and whether asking again may pass.
#[derive(Clone, Debug)]
pub struct Refusal {
    /// What failed, in words.
    pub why: String,
    /// What it means for the file.
    pub fault: Fault,
    /// Whether asking again may pass.
    pub busy: bool,
}

impl Refusal {
    /// A refusal that asking again does not mend, answered.
    pub fn lasting(fault: Fault, why: impl Into<String>) -> Self {
        Refusal { why: why.into(), fault, busy: false }
    }
}

/// `digest` asked once with a proxy; a failure says what it means for the file.
pub fn digest_once(agent: &ureq::Agent, url: &str) -> Result<Digest, Refusal> {
    let (origin, path) = door(url).map_err(|why| Refusal::lasting(Fault::Mistake, why))?;
    let reply = head(agent, &format!("{origin}{path}"));
    let fault = reply.as_ref().err().map_or(Fault::Unreachable, |e| fault(e, true));
    let busy = reply.as_ref().is_err_and(|e| passing(e, true).is_some());
    digested(url, reply).map_err(|why| Refusal { why, fault, busy })
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
fn head(agent: &ureq::Agent, at: &str) -> Result<ureq::Response, ureq::Error> {
    follow(at, |at| {
        agent.head(at).set("Want-Digest", "adler32").call()
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
    let adler = reply.header("Digest").and_then(|d| {
        d.split(',')
            .find_map(|p| p.trim().strip_prefix("adler32="))
            .map(|s| s.to_ascii_lowercase())
    });
    Ok((bytes, adler))
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
            _ if passing(error, self.proxied).is_some() => {
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
                self.agent.get(url).set("Range", &header).call()
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
                self.agent.get(url).set("Range", &range).call()
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
        assert_eq!(found[0].modified.as_deref(), Some("2026-06-25T23:05:06Z"));
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
