//! Storage over HTTPS: any `root://`, `davs://` or `https://` location, through the WebDAV door
//! the WLCG registry (grid-doors) lists for its host. Folders are listed with one PROPFIND, and a
//! file's adler32 is asked with one HEAD (`Want-Digest`); nothing is read from any file.

use std::collections::HashMap;
use std::sync::Mutex;

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
    let doors = grid_doors::serving(&grid_doors::snapshot().doors, &host);
    let door = doors.first().ok_or_else(|| {
        format!("{host} has no WebDAV door in the WLCG registry ({}), so its folders cannot be listed from here. Name its files one by one, or copy them somewhere listable.", grid_doors::snapshot().taken)
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

/// HTTPS to storage doors with the grid proxy, when there is one; folder listings are kept.
pub struct Storage {
    agent: ureq::Agent,
    listed: Mutex<HashMap<String, Result<Vec<Entry>, String>>>,
    pub proxy: Option<String>,
}

impl Storage {
    pub fn new(proxy: Option<&str>, ca: Option<&str>) -> Result<Self, String> {
        let loaded = match proxy {
            Some(p) => Some(
                grid_proxy::load(p)
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
        let reply = self
            .agent
            .request("PROPFIND", &format!("{origin}{path}"))
            .set("Depth", "1")
            .set("Content-Type", "application/xml")
            .send_string(body);
        let text = match reply {
            Ok(r) => r.into_string().map_err(|e| e.to_string())?,
            Err(ureq::Error::Status(404, _)) => {
                return Err(format!("the folder {folder} does not exist"))
            }
            Err(ureq::Error::Status(code @ (401 | 403), _)) => {
                return Err(format!(
                    "{folder} refused the listing ({code}). {}",
                    if self.proxy.is_some() {
                        "The grid proxy may not give access there."
                    } else {
                        "It needs a grid proxy: run voms-proxy-init -voms cms -valid 192:00."
                    }
                ))
            }
            Err(e) => return Err(format!("{folder} could not be listed: {e}")),
        };
        let mut entries = Vec::new();
        for block in text.split("response>").filter(|b| b.contains("href>")) {
            let Some(href) = element(block, "href") else {
                continue;
            };
            if block.contains("collection") {
                continue;
            }
            let name = href
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(href)
                .to_string();
            let bytes = element(block, "getcontentlength")
                .or_else(|| element(block, "size"))
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            entries.push(Entry {
                name,
                bytes,
                modified: element(block, "getlastmodified").and_then(rfc3339),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// A storage file's size and adler32 as its storage reports them, without reading it.
    pub fn digest(&self, url: &str) -> Result<(u64, Option<String>), String> {
        let (origin, path) = door(url)?;
        let reply = self
            .agent
            .head(&format!("{origin}{path}"))
            .set("Want-Digest", "adler32")
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(404, _) => format!("{url} no longer exists"),
                e => format!("{url} could not be asked: {e}"),
            })?;
        let bytes = reply
            .header("Content-Length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let adler = reply.header("Digest").and_then(|d| {
            d.split(',')
                .find_map(|p| p.trim().strip_prefix("adler32="))
                .map(|s| s.to_ascii_lowercase())
        });
        Ok((bytes, adler))
    }
}

/// A storage file read in byte ranges over HTTPS, for hep-root: the door's URL first, then the
/// disk server it redirects to. Only the ranges hep-root asks for are read.
pub struct HttpsFile {
    agent: ureq::Agent,
    url: Mutex<String>,
    shown: String,
    len: u64,
    /// Requests made and bytes read, for the timing the resolver reports.
    pub requests: std::sync::atomic::AtomicUsize,
    pub bytes: std::sync::atomic::AtomicUsize,
}

impl HttpsFile {
    pub fn new(storage: &Storage, at: &str, len: u64) -> Result<Self, String> {
        let (origin, path) = door(at)?;
        Ok(HttpsFile {
            agent: storage.agent.clone(),
            url: Mutex::new(format!("{origin}{path}")),
            shown: at.to_string(),
            len,
            requests: Default::default(),
            bytes: Default::default(),
        })
    }
}

impl hep_root::ByteSource for HttpsFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> hep_root::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let range = format!("bytes={offset}-{}", offset + buf.len() as u64 - 1);
        for _ in 0..4 {
            let url = self.url.lock().unwrap().clone();
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match self.agent.get(&url).set("Range", &range).call() {
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
                    return reply
                        .into_reader()
                        .read_exact(buf)
                        .map_err(|e| hep_root::Error::Remote(format!("{}: {e}", self.shown)));
                }
                Ok(reply) if (301..=308).contains(&reply.status()) => {
                    let to = reply.header("Location").ok_or_else(|| {
                        hep_root::Error::Remote(format!(
                            "{}: a redirect without a Location",
                            self.shown
                        ))
                    })?;
                    *self.url.lock().unwrap() = to.to_string();
                }
                Ok(reply) => {
                    return Err(hep_root::Error::Remote(format!(
                    "{}: asked for bytes {range}, got status {} (the storage ignored the range)",
                    self.shown,
                    reply.status()
                )))
                }
                Err(ureq::Error::Status(code, reply)) if (301..=308).contains(&code) => {
                    let to = reply.header("Location").ok_or_else(|| {
                        hep_root::Error::Remote(format!(
                            "{}: a redirect without a Location",
                            self.shown
                        ))
                    })?;
                    *self.url.lock().unwrap() = to.to_string();
                }
                Err(e) => return Err(hep_root::Error::Remote(format!("{}: {e}", self.shown))),
            }
        }
        Err(hep_root::Error::Remote(format!(
            "{}: too many redirects",
            self.shown
        )))
    }

    fn describe(&self) -> String {
        self.shown.clone()
    }
}
