//! A storage for the tests: an HTTPS server on localhost that answers as a WebDAV door answers
//! rvr (PROPFIND of a folder, HEAD with the adler32, GET of one or many byte ranges), with the
//! certificate of fixtures/tls, every request recorded and every answer open to a script.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// One request as the server read it.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub range: Option<String>,
    /// Which request of its connection it is: 1 for the first, 2 for one sent on a kept connection.
    pub nth: usize,
}

impl Request {
    /// How many byte ranges a GET asks for.
    pub fn ranges(&self) -> usize {
        self.range.as_deref().map_or(0, |r| r.split(',').count())
    }
}

/// How the server answers one request.
#[derive(Clone, Debug)]
pub enum Answer {
    /// As a door does.
    Serve,
    /// This status, with no body.
    Status(u16),
    /// A `302` to this path or URL.
    Redirect(String),
    /// The connection closed, nothing answered.
    Drop,
    /// The headers of the answer, then half its body, then the connection closed.
    Short,
    /// A `200` with the whole file, the range passed over.
    Whole,
    /// Every range asked joined into one part, as a single `206`.
    OnePart,
    /// To several ranges, a `multipart/byteranges` answer whose parts do not say where they lie.
    Placeless,
    /// To several ranges, a `multipart/byteranges` answer whose body is not laid out as parts.
    Garbled,
    /// The answer, after a wait.
    Late(Duration, Box<Answer>),
    /// The server gone: this connection closed, and no other taken.
    Vanish,
    /// The request taken and never answered, its connection held open until the server stops.
    Silent,
    /// The answer, once this many requests have come (or after 30 s, so a test never hangs).
    #[allow(dead_code)] // remote.rs's tests only
    Together(usize, Box<Answer>),
}

type Script = Box<dyn FnMut(&Request) -> Answer + Send>;

/// A running server, stopped when dropped.
pub struct Server {
    pub port: u16,
    files: Arc<Mutex<BTreeMap<String, Arc<Vec<u8>>>>>,
    log: Arc<Mutex<Vec<Request>>>,
    script: Arc<Mutex<Script>>,
    multipart: Arc<Mutex<bool>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    busy: Arc<(AtomicUsize, AtomicUsize, AtomicUsize)>,
    connections: Arc<AtomicUsize>,
    /// The thread taking connections, joined on drop so its port is free again after.
    accepting: Option<std::thread::JoinHandle<()>>,
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls").join(name)).unwrap()
}

/// The adler32 of `bytes`, as 8 lowercase hex digits.
pub fn adler32(bytes: &[u8]) -> String {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    format!("{:08x}", (b << 16) | a)
}

impl Server {
    /// A server with the trusted certificate.
    pub fn start() -> Self {
        Self::on("server", 0)
    }

    /// A server on `port` (any, for 0) whose certificate (`server` or `stranger`) is signed by
    /// the fixture's authority or another.
    pub fn on(certificate: &str, port: u16) -> Self {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let chain = CertificateDer::pem_slice_iter(&fixture(&format!("{certificate}.pem"))).collect::<Result<Vec<_>, _>>().unwrap();
        let key = PrivateKeyDer::from_pem_slice(&fixture(&format!("{certificate}.key"))).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        let config = Arc::new(config);
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let script: Script = Box::new(|_| Answer::Serve);
        let mut server = Server {
            port,
            files: Default::default(),
            log: Default::default(),
            script: Arc::new(Mutex::new(script)),
            multipart: Arc::new(Mutex::new(true)),
            stop: Default::default(),
            busy: Default::default(),
            connections: Default::default(),
            accepting: None,
        };
        let shared = (server.files.clone(), server.log.clone(), server.script.clone(), server.multipart.clone(), server.stop.clone(), server.busy.clone());
        let connections = server.connections.clone();
        server.accepting = Some(std::thread::spawn(move || {
            for stream in listener.incoming() {
                if shared.4.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                connections.fetch_add(1, Ordering::SeqCst);
                let (config, files, log, script, multipart, stop, busy) = (config.clone(), shared.0.clone(), shared.1.clone(), shared.2.clone(), shared.3.clone(), shared.4.clone(), shared.5.clone());
                std::thread::spawn(move || {
                    let Ok(tls) = rustls::ServerConnection::new(config) else { return };
                    let vanished = serve(rustls::StreamOwned::new(tls, stream), &files, &log, &script, &multipart, &busy, &stop);
                    if matches!(vanished, Ok(true)) {
                        stop.store(true, Ordering::Relaxed);
                        let _ = TcpStream::connect(("127.0.0.1", port));
                    }
                });
            }
        }));
        server
    }

    /// The URL of `path` on this server.
    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.port)
    }

    /// Serve `bytes` at `path`.
    pub fn put(&self, path: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), Arc::new(bytes));
    }

    /// Answer each request as `script` says.
    pub fn script(&self, script: impl FnMut(&Request) -> Answer + Send + 'static) {
        *self.script.lock().unwrap() = Box::new(script);
    }

    /// Whether requests for several ranges are answered in parts (else with a `416`).
    pub fn multipart(&self, answers: bool) {
        *self.multipart.lock().unwrap() = answers;
    }

    /// The most requests answered at once so far.
    pub fn most_at_once(&self) -> usize {
        self.busy.1.load(Ordering::SeqCst)
    }

    /// The connections taken so far, a request sent on them or not.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Every request so far, and none kept after.
    pub fn taken(&self) -> Vec<Request> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Joined only when woken: a server gone already is not listening.
        if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
            if let Some(accepting) = self.accepting.take() {
                let _ = accepting.join();
            }
        }
    }
}

type Tls = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

/// Answer the requests of one connection, until it closes; true when the server is to vanish.
fn serve(
    stream: Tls,
    files: &Mutex<BTreeMap<String, Arc<Vec<u8>>>>,
    log: &Mutex<Vec<Request>>,
    script: &Mutex<Script>,
    multipart: &Mutex<bool>,
    busy: &(AtomicUsize, AtomicUsize, AtomicUsize),
    stop: &std::sync::atomic::AtomicBool,
) -> std::io::Result<bool> {
    let mut reader = BufReader::new(stream);
    for nth in 1.. {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(false);
        }
        let mut words = line.split_whitespace();
        let (method, path) = (words.next().unwrap_or("").to_owned(), words.next().unwrap_or("").to_owned());
        let (mut range, mut length) = (None, 0usize);
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap_or((header, ""));
            match name.to_ascii_lowercase().as_str() {
                "range" => range = value.trim().strip_prefix("bytes=").map(str::to_owned),
                "content-length" => length = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        let request = Request { method, path, range, nth };
        log.lock().unwrap().push(request.clone());
        let answer = (script.lock().unwrap())(&request);
        let now = busy.0.fetch_add(1, Ordering::SeqCst) + 1;
        busy.1.fetch_max(now, Ordering::SeqCst);
        busy.2.fetch_add(1, Ordering::SeqCst);
        let mut answer = answer;
        let answer = loop {
            answer = match answer {
                Answer::Late(wait, then) => {
                    std::thread::sleep(wait);
                    *then
                }
                Answer::Together(n, then) => {
                    let since = std::time::Instant::now();
                    while busy.2.load(Ordering::SeqCst) < n && since.elapsed() < Duration::from_secs(30) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    *then
                }
                answer => break answer,
            };
        };
        busy.0.fetch_sub(1, Ordering::SeqCst);
        let file = files.lock().unwrap().get(&request.path).cloned();
        let several = *multipart.lock().unwrap();
        let out = reader.get_mut();
        match answer {
            Answer::Silent => {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                return Ok(false);
            }
            Answer::Drop | Answer::Vanish => {
                let _ = out.sock.shutdown(std::net::Shutdown::Both);
                return Ok(matches!(answer, Answer::Vanish));
            }
            Answer::Status(code) => reply(out, code, &[], b"")?,
            Answer::Redirect(to) => reply(out, 302, &[("Location", to)], b"")?,
            Answer::Serve | Answer::Short | Answer::Whole | Answer::OnePart | Answer::Placeless | Answer::Garbled => {
                let short = matches!(answer, Answer::Short);
                let (code, headers, bytes) = served(&request, files, file.as_deref().map(Vec::as_slice), &answer, several);
                if short {
                    let head = head(code, &headers, bytes.len());
                    out.write_all(head.as_bytes())?;
                    out.write_all(&bytes[..bytes.len() / 2])?;
                    out.flush()?;
                    let _ = out.sock.shutdown(std::net::Shutdown::Both);
                    return Ok(false);
                }
                reply(out, code, &headers, &bytes)?
            }
            Answer::Late(..) | Answer::Together(..) => unreachable!(),
        }
    }
    Ok(false)
}

/// What a door answers: a listing, a digest, the file, or byte ranges of it.
fn served(
    request: &Request,
    files: &Mutex<BTreeMap<String, Arc<Vec<u8>>>>,
    file: Option<&[u8]>,
    answer: &Answer,
    several: bool,
) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
    if request.method == "PROPFIND" {
        let folder = request.path.trim_end_matches('/').to_owned() + "/";
        let files = files.lock().unwrap();
        let inside: Vec<_> = files.iter().filter(|(p, _)| p.starts_with(&folder) && !p[folder.len()..].contains('/')).collect();
        if inside.is_empty() {
            return (404, vec![], vec![]);
        }
        let mut xml = format!(r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:href>{folder}</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>"#);
        for (path, bytes) in inside {
            xml += &format!("<D:response><D:href>{path}</D:href><D:propstat><D:prop><D:resourcetype/><D:getcontentlength>{}</D:getcontentlength></D:prop></D:propstat></D:response>", bytes.len());
        }
        xml += "</D:multistatus>";
        return (207, vec![("Content-Type", "application/xml".into())], xml.into_bytes());
    }
    let Some(file) = file else { return (404, vec![], vec![]) };
    if request.method == "HEAD" {
        let headers = vec![("Content-Length", file.len().to_string()), ("Digest", format!("adler32={}", adler32(file)))];
        return (200, headers, vec![]);
    }
    let whole = || (200, vec![], file.to_vec());
    let Some(asked) = request.range.as_deref() else { return whole() };
    if matches!(answer, Answer::Whole) {
        return whole();
    }
    let ranges: Vec<(usize, usize)> = asked
        .split(',')
        .map(|r| {
            let (a, b) = r.trim().split_once('-').unwrap();
            let (a, b) = (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap());
            (a, b.min(file.len() - 1))
        })
        .collect();
    let len = file.len();
    let part = |(a, b): (usize, usize)| (format!("bytes {a}-{b}/{len}"), file[a..=b].to_vec());
    if ranges.len() == 1 || matches!(answer, Answer::OnePart) {
        let span = (ranges.iter().map(|r| r.0).min().unwrap(), ranges.iter().map(|r| r.1).max().unwrap());
        let (range, bytes) = part(span);
        return (206, vec![("Content-Range", range)], bytes);
    }
    if !several {
        return (416, vec![], vec![]);
    }
    let mut body = Vec::new();
    for &r in &ranges {
        let (range, bytes) = part(r);
        let place = match answer {
            Answer::Placeless => String::new(),
            _ => format!("Content-Range: {range}\r\n"),
        };
        body.extend(format!("\r\n--RVRTEST\r\nContent-Type: application/octet-stream\r\n{place}\r\n").bytes());
        body.extend(bytes);
    }
    if matches!(answer, Answer::Garbled) {
        body = [&b"these are not parts\r\n"[..], &body].concat();
    }
    body.extend(b"\r\n--RVRTEST--\r\n");
    (206, vec![("Content-Type", "multipart/byteranges; boundary=RVRTEST".into())], body)
}

/// A status line and headers, with the body's length.
fn head(code: u16, headers: &[(&str, String)], length: usize) -> String {
    let mut head = format!("HTTP/1.1 {code} Answer\r\n");
    if !headers.iter().any(|(name, _)| *name == "Content-Length") {
        head += &format!("Content-Length: {length}\r\n");
    }
    for (name, value) in headers {
        head += &format!("{name}: {value}\r\n");
    }
    head + "\r\n"
}

fn reply(out: &mut Tls, code: u16, headers: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    out.write_all(head(code, headers, body.len()).as_bytes())?;
    out.write_all(body)?;
    out.flush()
}
