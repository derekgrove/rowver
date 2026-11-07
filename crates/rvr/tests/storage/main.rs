//! rvr against storage over HTTPS: a test door on localhost (server.rs), trusted through
//! `$X509_CERT_DIR`, answering as it is scripted to.

#[path = "../common/mod.rs"]
mod common;
mod server;

use common::{rvr_with, Study};
use server::{Answer, Server};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const RECIPE: &str = r#"format = "rvr-recipe"
version = 1

[define]

[keep]

[columns]
branches = ["nJet", "Jet_pt"]

[totals]

[derived]
"#;

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn ntuple() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uproot_flat.root")).unwrap()
}

/// A study of one sample whose files are `files` (a TOML value), and a server holding the ntuple
/// at `/store/flat.root`.
fn remote(name: &str, files: impl Fn(&Server) -> String) -> (Study, Server) {
    let server = Server::start();
    server.put("/store/flat.root", ntuple());
    let samples = format!(
        "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nfiles = {}\ntree = \"analysis\"\n",
        files(&server)
    );
    let dir = common::study(name, &samples, RECIPE);
    (dir, server)
}

/// Run rvr in `dir` trusting the test authority, with no grid proxy (or the one at `proxy`), and
/// each second of waiting a millisecond.
fn rvr(dir: &Path, proxy: Option<&Path>, args: &[&str]) -> (i32, String) {
    rvr_also(dir, proxy, &[], args)
}

/// The same, with more variables set.
fn rvr_also(dir: &Path, proxy: Option<&Path>, more: &[(&str, &str)], args: &[&str]) -> (i32, String) {
    let authority = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls/authority");
    let none = dir.join("no-proxy");
    let proxy = proxy.unwrap_or(&none);
    let set = [("X509_CERT_DIR", authority.to_str().unwrap()), ("X509_USER_PROXY", proxy.to_str().unwrap()), ("RVR_SECOND_MS", "1")];
    rvr_with(dir, &[&set[..], more].concat(), args)
}

/// The file a press of the study in `dir` writes.
fn file(dir: &Path) -> String {
    format!("{}.rvr", dir.file_name().unwrap().to_str().unwrap())
}

/// A grid proxy of the fixture's user, valid for `seconds`, written in `dir`.
fn proxy(dir: &Path, seconds: i64) -> std::path::PathBuf {
    let tls = fixtures().join("tls");
    let user = voms_proxy::user::Source::Pem { certificate: tls.join("user.pem"), key: tls.join("user.key") };
    let made = voms_proxy::make::proxy(&user.read("").unwrap(), None, voms_proxy::now(), seconds).unwrap();
    let path = dir.join("x509up");
    voms_proxy::make::write(&path, &made.text).unwrap();
    path
}

/// The header of the file a press of the study in `dir` wrote.
fn header(dir: &Path) -> serde_json::Value {
    let (code, said) = rvr(dir, None, &["header", &file(dir)]);
    assert_eq!(code, 0, "{said}");
    serde_json::from_str(&said).unwrap()
}

/// The kind of the one hole of the study's one file, if it has one.
fn hole(dir: &Path) -> Option<String> {
    let header = header(dir);
    let files = &header["made"]["files"]["flat"]["files"];
    let (_, file) = files.as_object().unwrap().iter().next().unwrap();
    file["holes"][0]["kind"].as_str().map(str::to_owned)
}

/// Resolve the study in `dir` while its storage serves every request.
fn resolved(dir: &Path) {
    let (code, said) = rvr(dir, None, &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
}

/// A study of the ntuple on storage, resolved, then pressed while the server answers as
/// `script` says: the press's exit status and what it said.
fn pressed(name: &str, proxied: bool, script: impl FnMut(&server::Request) -> Answer + Send + 'static) -> (Study, Server, i32, String) {
    let (dir, server) = remote(name, |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    server.script(script);
    server.taken();
    let proxy = proxied.then(|| proxy(&dir, 86400));
    let (code, said) = rvr(&dir, proxy.as_deref(), &["press"]);
    (dir, server, code, said)
}

/// Every GET, answered `answer` its first `failing` times, then served.
fn failing_gets(failing: usize, answer: Answer) -> impl FnMut(&server::Request) -> Answer + Send + 'static {
    let mut seen = 0;
    move |request| match request.method.as_str() {
        "GET" if seen < failing => {
            seen += 1;
            answer.clone()
        }
        _ => Answer::Serve,
    }
}

// SPEC 1.2, 3.8.1, 6.2.3, 7.2.5
#[test]
fn a_study_on_storage_is_resolved_and_pressed_over_https_and_davs() {
    let (dir, server) = remote("https", |s| format!("[{:?}, {:?}]", s.url("/store/flat.root"), s.url("/store/copy.root").replace("https://", "davs://")));
    server.put("/store/copy.root", ntuple());
    resolved(&dir);
    let asked = server.taken();
    let methods: Vec<&str> = asked.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(&methods[..2], ["PROPFIND", "HEAD"], "{asked:?}");
    assert!(asked.iter().all(|r| ["PROPFIND", "HEAD", "GET"].contains(&r.method.as_str())), "{asked:?}");
    let lock = std::fs::read_to_string(dir.join("samples.lock")).unwrap();
    assert!(lock.contains(&format!("at = {:?}", s_url(&server, "davs", "/store/copy.root"))), "{lock}");
    assert!(lock.contains(&format!("adler32 = {:?}", server::adler32(&ntuple()))), "{lock}");
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!(code, 0, "{said}");
    // The baskets a run reads, far apart, are asked in one request and answered in parts.
    assert!(server.taken().iter().any(|r| r.ranges() > 1));
    let (code, said) = rvr(&dir, None, &["validate", &file(&dir)]);
    assert_eq!(code, 0, "{said}");
    assert_eq!(header(&dir)["data"]["rows"], 10);
}

fn s_url(server: &Server, scheme: &str, path: &str) -> String {
    server.url(path).replace("https", scheme)
}

// SPEC 7.2.4
#[test]
fn a_read_follows_four_redirects_and_no_more() {
    // Each hop sends the read one step on; the last serves the file.
    let hops = |most: usize| {
        move |request: &server::Request| match request.path.strip_prefix("/hop").and_then(|p| p.split('/').next()?.parse::<usize>().ok()) {
            _ if request.method != "GET" => Answer::Serve,
            None => Answer::Redirect("/hop1/flat.root".into()),
            Some(n) if n < most => Answer::Redirect(format!("/hop{}/flat.root", n + 1)),
            Some(_) => Answer::Serve,
        }
    };
    let (dir, server) = remote("hops", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    for n in 1..=5 {
        server.put(&format!("/hop{n}/flat.root"), ntuple());
    }
    server.script(hops(4));
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!((code, hole(&dir)), (0, None), "{said}");
    let (dir, server) = remote("five-hops", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    for n in 1..=5 {
        server.put(&format!("/hop{n}/flat.root"), ntuple());
    }
    server.script(hops(5));
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("redirected more than four times"), "{said}");
    assert_eq!(hole(&dir).as_deref(), Some("unreachable"));
}

// SPEC 7.2.5
#[test]
fn a_storage_that_refuses_many_ranges_is_asked_one_range_a_request() {
    let (dir, server) = remote("one-range", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    server.multipart(false);
    server.taken();
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!((code, hole(&dir)), (0, None), "{said}");
    let asked = server.taken();
    let refused = asked.iter().position(|r| r.ranges() > 1).expect("a request for several ranges");
    assert!(asked[refused + 1..].iter().all(|r| r.ranges() <= 1), "{asked:?}");
    // One answer in one part covering every range asked is taken as it is.
    let (dir, server) = remote("one-part", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    server.script(|_| Answer::OnePart);
    server.taken();
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!((code, hole(&dir)), (0, None), "{said}");
    let asked = server.taken();
    let many = asked.iter().position(|r| r.ranges() > 1).expect("a request for several ranges");
    let again: Vec<_> = asked[many + 1..].iter().filter(|r| r.range.as_deref().is_some_and(|r| asked[many].range.as_deref().unwrap().contains(r))).collect();
    assert!(again.is_empty(), "{asked:?}");
}

// SPEC 7.2.6
#[test]
fn a_fetch_ahead_that_fails_is_neither_waited_for_nor_asked_again() {
    let mut failed = false;
    let (dir, server, code, said) = pressed("ahead", false, move |request| match request.ranges() > 1 && !failed {
        true => {
            failed = true;
            Answer::Status(503)
        }
        false => Answer::Serve,
    });
    assert_eq!((code, hole(&dir)), (0, None), "{said}");
    assert!(!said.contains("waiting"), "{said}");
    let asked = server.taken();
    let ahead = asked.iter().position(|r| r.ranges() > 1).unwrap();
    assert!(asked[ahead + 1..].iter().all(|r| r.range != asked[ahead].range), "{asked:?}");
}

// SPEC 7.4.1
#[test]
fn a_busy_or_lost_answer_is_asked_again_after_each_wait() {
    for (answer, proxied, why) in [
        (Answer::Status(403), true, "its storage answered 403"),
        (Answer::Status(408), false, "its storage answered 408"),
        (Answer::Status(429), false, "its storage answered 429"),
        (Answer::Status(500), false, "its storage answered 500"),
        (Answer::Status(502), false, "its storage answered 502"),
        (Answer::Status(503), false, "its storage answered 503"),
        (Answer::Status(504), false, "its storage answered 504"),
        (Answer::Drop, false, "the connection to its storage broke ("),
        (Answer::Short, false, "its storage's answer ended before every byte asked came (peer closed connection"),
    ] {
        let (dir, _server, code, said) = pressed("busy", proxied, failing_gets(2, answer.clone()));
        assert_eq!((code, hole(&dir)), (0, None), "{answer:?}: {said}");
        assert!(said.contains(&format!("  waiting 5 s to ask again for flat.root (1 of 8 tries made): {why}")), "{answer:?}: {said}");
        assert!(said.contains("  waiting 15 s to ask again for flat.root (2 of 8 tries made): "), "{answer:?}: {said}");
        assert!(!said.contains("3 of 8"), "{answer:?}: {said}");
    }
}

// SPEC 7.4.2, 7.3.1
#[test]
fn a_read_whose_eight_tries_fail_is_a_hole() {
    let (dir, _server, code, said) = pressed("unreachable", false, failing_gets(usize::MAX, Answer::Status(503)));
    assert_eq!((code, hole(&dir).as_deref()), (3, Some("unreachable")), "{said}");
    for (k, wait) in [5, 15, 30, 60, 120, 240, 300].iter().enumerate() {
        assert!(said.contains(&format!("  waiting {wait} s to ask again for flat.root ({} of 8 tries made)", k + 1)), "{said}");
    }
    let (dir, _server, code, said) = pressed("refused", true, failing_gets(usize::MAX, Answer::Status(403)));
    assert_eq!((code, hole(&dir).as_deref()), (3, Some("refused")), "{said}");
}

// SPEC 7.2.4, 7.3.1
#[test]
fn a_location_to_a_plain_http_server_is_a_hole_naming_only_its_host_and_port() {
    // A port answering in plain HTTP whatever it is sent, as a pool without TLS there does.
    let plain = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = plain.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in plain.incoming().flatten() {
            let _ = std::io::Read::read(&mut stream, &mut [0; 4096]);
            let _ = std::io::Write::write_all(&mut stream, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    // Every read sent there, its query holding a storage's signed token, as EOS and dCache send.
    let location = format!("http://127.0.0.1:{port}/eos/flat.root?authz=SECRET-TOKEN&cap.sym=XYZ");
    let (dir, _server, code, said) = pressed("plain", false, move |request| match request.method.as_str() {
        "GET" => Answer::Redirect(location.clone()),
        _ => Answer::Serve,
    });
    assert_eq!((code, hole(&dir).as_deref()), (3, Some("unreachable")), "{said}");
    assert!(!said.contains("waiting"), "asked again: {said}");
    let header = header(&dir).to_string();
    for shown in [&said, &header] {
        assert!(shown.contains(&format!("http://127.0.0.1:{port}, a plain HTTP server")), "{shown}");
        assert!(!shown.contains("SECRET") && !shown.contains("cap.sym") && !shown.contains("/eos/flat.root"), "{shown}");
    }
}

// SPEC 7.4.3
#[test]
fn some_answers_are_taken_at_once() {
    let unreachable = [Answer::Whole, Answer::Status(400), Answer::Status(416), Answer::Status(418)].map(|answer| (answer, "unreachable"));
    for (answer, kind) in [(Answer::Status(404), "gone"), (Answer::Status(410), "gone")].into_iter().chain(unreachable) {
        let (dir, _server, code, said) = pressed("at-once", false, failing_gets(usize::MAX, answer.clone()));
        assert_eq!((code, hole(&dir).as_deref()), (3, Some(kind)), "{answer:?}: {said}");
        assert!(!said.contains("waiting"), "{answer:?}: {said}");
    }
    // Every file there would fail the same: the press stops.
    for (answer, proxied) in [(Answer::Status(401), true), (Answer::Status(403), false)] {
        let (dir, _server, code, said) = pressed("stops", proxied, failing_gets(usize::MAX, answer.clone()));
        assert_eq!(code, 1, "{answer:?}: {said}");
        assert!(!dir.join(file(&dir)).exists() && !said.contains("waiting"), "{answer:?}: {said}");
    }
    // The same door, now with a certificate no authority rvr trusts signed.
    let (dir, server) = remote("untrusted", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    let port = server.port;
    drop(server);
    let stranger = Server::on("stranger", port);
    stranger.put("/store/flat.root", ntuple());
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("UnknownIssuer") && !said.contains("waiting"), "{said}");
    assert!(!dir.join(file(&dir)).exists());
}

/// A study of two copies of the ntuple on storage, resolved.
fn two_files(name: &str) -> (Study, Server) {
    let (dir, server) = remote(name, |s| format!("[{:?}, {:?}]", s.url("/store/flat.root"), s.url("/store/copy.root")));
    server.put("/store/copy.root", ntuple());
    resolved(&dir);
    server.taken();
    (dir, server)
}

// SPEC 7.4.4
#[test]
fn waits_end_when_the_press_stops_for_another_reason() {
    let (dir, server) = two_files("ends-waits");
    server.script(|request| match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/store/flat.root") => Answer::Status(503),
        ("GET", _) => Answer::Late(std::time::Duration::from_millis(300), Box::new(Answer::Status(401))),
        _ => Answer::Serve,
    });
    let started = std::time::Instant::now();
    let (code, said) = rvr_also(&dir, None, &[("RVR_SECOND_MS", "1000")], &["press", "--workers", "2"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("waiting 5 s to ask again for flat.root"), "{said}");
    assert!(started.elapsed() < std::time::Duration::from_secs(4), "{:?}: {said}", started.elapsed());
}

// SPEC 7.4.5
#[test]
fn a_storage_gone_quiet_at_its_one_site_stops_the_press_keeping_what_it_read() {
    let (dir, server) = two_files("outage");
    server.script(|request| match request.path.as_str() {
        "/store/copy.root" => Answer::Vanish,
        _ => Answer::Serve,
    });
    let (code, said) = rvr(&dir, None, &["press", "--workers", "1"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("rvr cannot tell that site down from the network gone, so it stops and keeps what it read"), "{said}");
    assert!(!dir.join(file(&dir)).exists() && dir.join(file(&dir) + ".partial").exists(), "{said}");
}

// SPEC 7.7.4
#[test]
fn rvr_trace_prints_every_byte_range_request_with_what_answered_it() {
    let (dir, _server) = remote("traced", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    let (code, said) = rvr_also(&dir, None, &[("RVR_TRACE", "1")], &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("  GET bytes=0-2047 (2048 bytes) of flat.root: 206"), "{said}");
    assert!(said.contains(" ranges (") && said.contains(" bytes) of flat.root: 206"), "{said}");
}

// SPEC 6.2.6, 10.6
#[test]
fn resolve_asks_a_busy_storage_again_and_warns_of_a_proxy_ending_soon() {
    let (dir, server) = remote("busy-listing", |s| format!("{:?}", s.url("/store/flat.root")));
    let mut busy = std::collections::HashMap::new();
    server.script(move |request| {
        let tries = busy.entry(request.method.clone()).or_insert(0);
        *tries += 1;
        match request.method.as_str() {
            "PROPFIND" | "HEAD" if *tries <= 2 => Answer::Status(503),
            _ => Answer::Serve,
        }
    });
    let proxy = proxy(&dir, 3600);
    let (code, said) = rvr(&dir, Some(&proxy), &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("your grid proxy (or its VOMS attributes) ends in "), "{said}");
    for what in [server.url("/store"), server.url("/store/flat.root")] {
        assert!(said.contains(&format!("  waiting 5 s to ask again for {what} (1 of 4 tries made): its storage answered 503")), "{said}");
        assert!(said.contains(&format!("  waiting 15 s to ask again for {what} (2 of 4 tries made)")), "{said}");
    }
    let (code, said) = rvr(&dir, Some(&proxy), &["press"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("your grid proxy (or its VOMS attributes) ends in "), "{said}");
    // A day left: no warning.
    let proxy = self::proxy(&dir, 86400);
    let (code, said) = rvr(&dir, Some(&proxy), &["samples", "resolve", "samples.toml"]);
    assert!(code == 0 && !said.contains("ends in"), "{said}");
}

// SPEC 10.6
#[test]
fn the_test_authority_is_trusted_only_through_x509_cert_dir() {
    let (dir, _server) = remote("trust", |s| format!("{:?}", s.url("/store/flat.root")));
    let (code, said) = rvr_with(&dir, &[("X509_CERT_DIR", ""), ("X509_USER_PROXY", "none")], &["samples", "resolve", "samples.toml"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("UnknownIssuer") && !dir.join("samples.lock").exists(), "{said}");
}

// SPEC 6.3.1, 6.3.2
#[test]
fn check_asks_storage_for_each_file_with_one_head() {
    let (dir, server) = two_files("check");
    let (code, said) = rvr(&dir, None, &["samples", "check", "samples.lock"]);
    assert_eq!(code, 0, "{said}");
    assert!(server.taken().iter().all(|r| r.method == "HEAD"));
    let mut changed = ntuple();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    server.put("/store/copy.root", changed);
    let (code, said) = rvr(&dir, None, &["samples", "check", "samples.lock"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("copy.root") && !said.contains("flat.root"), "{said}");
}

// SPEC 10.4
#[test]
fn proxy_shows_a_usable_proxy_and_exits_0() {
    let (dir, _server) = remote("proxy", |s| format!("{:?}", s.url("/store/flat.root")));
    let proxy = proxy(&dir, 7200);
    let (code, said) = rvr(&dir, Some(&proxy), &["proxy"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains(proxy.to_str().unwrap()) && said.contains("Test User"), "{said}");
    assert!(said.contains("hour"), "{said}");
}

// SPEC 7.2.4
#[test]
#[ignore = "waits two minutes for a read that is never answered"]
fn a_read_waits_120_s_for_its_answer() {
    let started = std::time::Instant::now();
    let late = Answer::Late(std::time::Duration::from_secs(150), Box::new(Answer::Serve));
    let (dir, _server, code, said) = pressed("read-timeout", false, failing_gets(1, late));
    let took = started.elapsed().as_secs();
    assert_eq!((code, hole(&dir)), (0, None), "{said}");
    assert!(said.contains("(1 of 8 tries made): it did not answer in time"), "{said}");
    assert!((120..135).contains(&took), "{took} s: {said}");
}

/// rvr in `dir` as `rvr` runs it, started: the child, and the file it says what it says in.
fn spawned(dir: &Path, args: &[&str]) -> (std::process::Child, std::path::PathBuf) {
    spawned_with(dir, &[], args)
}

/// The same, with more variables set.
fn spawned_with(dir: &Path, more: &[(&str, &str)], args: &[&str]) -> (std::process::Child, std::path::PathBuf) {
    let said = dir.join("said.txt");
    let out = std::fs::File::create(&said).unwrap();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_rvr"))
        .args(args)
        .current_dir(dir)
        .env("RVR_CONFIG", dir.join(".rvr-config"))
        .env("X509_CERT_DIR", fixtures().join("tls/authority"))
        .env("X509_USER_PROXY", dir.join("no-proxy"))
        .env("RVR_SECOND_MS", "1")
        .envs(more.iter().copied())
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    (child, said)
}

/// rvr in `dir`, killed should it run `limit` seconds: its exit status (`None` when killed), what
/// it said, and the seconds it took.
fn rvr_within(dir: &Path, limit: u64, args: &[&str]) -> (Option<i32>, String, u64) {
    let started = std::time::Instant::now();
    let (mut child, said) = spawned(dir, args);
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        if started.elapsed().as_secs() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    (code, std::fs::read_to_string(said).unwrap(), started.elapsed().as_secs())
}

/// Every request served but the first sent on a kept connection (its second or later), never answered.
fn silent_on_a_kept_connection() -> impl FnMut(&server::Request) -> Answer + Send + 'static {
    let mut silenced = false;
    move |request| match request.nth >= 2 && !silenced {
        true => {
            silenced = true;
            Answer::Silent
        }
        false => Answer::Serve,
    }
}

// SPEC 7.2.4
#[test]
#[ignore = "waits two minutes for an answer never given on a kept connection"]
fn a_read_on_a_kept_connection_waits_120_s_for_its_answer() {
    let (dir, server) = remote("kept-read", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    server.script(silent_on_a_kept_connection());
    server.taken();
    let (code, said, took) = rvr_within(&dir, 200, &["press"]);
    assert!(server.taken().iter().any(|r| r.nth >= 2), "the press sent no request on a kept connection: {said}");
    assert_eq!(code, Some(0), "{took} s: {said}");
    assert_eq!(hole(&dir), None, "{said}");
    assert!(said.contains("it did not answer in time"), "{said}");
    assert!((120..135).contains(&took), "{took} s: {said}");
}

// SPEC 7.2.4, 6.2.6
#[test]
#[ignore = "waits two minutes for an answer never given on a kept connection"]
fn a_listing_on_a_kept_connection_waits_120_s_for_its_answer() {
    let (dir, server) = remote("kept-resolve", |s| format!("{:?}", s.url("/store/flat.root")));
    server.script(silent_on_a_kept_connection());
    let (code, said, took) = rvr_within(&dir, 200, &["samples", "resolve", "samples.toml"]);
    assert!(server.taken().iter().any(|r| r.nth >= 2), "resolve sent no request on a kept connection: {said}");
    assert_eq!(code, Some(0), "{took} s: {said}");
    assert!(dir.join("samples.lock").exists(), "{said}");
    assert!((120..135).contains(&took), "{took} s: {said}");
}

// SPEC 9.5
#[test]
fn a_press_stopped_before_it_finished_a_file_is_shown_by_status() {
    let (dir, server) = remote("stopped-early", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    // A press stuck on a door that never answers, stopped once its journal is on disk.
    server.script(|request| match request.method.as_str() {
        "GET" => Answer::Silent,
        _ => Answer::Serve,
    });
    let (mut child, said) = spawned(&dir, &["press"]);
    let journal = dir.join(format!("{}.partial/journal.jsonl", file(&dir)));
    let started = std::time::Instant::now();
    while std::fs::read(&journal).map_or(true, |bytes| !bytes.ends_with(b"\n")) {
        assert!(started.elapsed().as_secs() < 30, "the press wrote no journal: {}", std::fs::read_to_string(&said).unwrap_or_default());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let (code, said) = rvr(&dir, None, &["status"]);
    assert_eq!(code, 1, "{said}");
    assert!(said.contains(".rvr.partial: a press stopped there before it finished a file (journal from ") && said.contains("); rvr press starts it over."), "{said}");
    assert!(said.contains(" is not there yet: rvr press makes it."), "{said}");
    // When a press would refuse, nothing is said of starting over.
    let recipe = std::fs::read_to_string(dir.join("recipe.toml")).unwrap();
    std::fs::write(dir.join("recipe.toml"), recipe.replace("version = 1", "version = 2")).unwrap();
    let (_, said) = rvr(&dir, None, &["status"]);
    assert!(said.contains("before it finished a file (journal from ") && said.contains("rvr press would refuse") && !said.contains("starts it over"), "{said}");
    std::fs::write(dir.join("recipe.toml"), &recipe).unwrap();
    // Nor beside a file that is current, when rvr press has nothing to do.
    let stopped = std::fs::read(&journal).unwrap();
    server.script(|_| Answer::Serve);
    let (code, said) = rvr(&dir, None, &["press"]);
    assert_eq!(code, 0, "{said}");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(&journal, stopped).unwrap();
    let (code, said) = rvr(&dir, None, &["status"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("before it finished a file (journal from ") && said.contains("It is current") && !said.contains("starts it over"), "{said}");
}

// SPEC 7.2.4
#[test]
#[ignore = "waits a minute for connections that never open (10.255.255.1 must drop packets, as most networks do)"]
fn a_connection_waits_15_s_to_open() {
    let dir = common::study("connect-timeout", "format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nfiles = \"https://10.255.255.1/store/flat.root\"\ntree = \"analysis\"\n", RECIPE);
    let started = std::time::Instant::now();
    let (code, said) = rvr(&dir, None, &["samples", "resolve", "samples.toml"]);
    let took = started.elapsed().as_secs();
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("(1 of 4 tries made): it did not answer in time"), "{said}");
    // Four tries, each 15 s, and the waits between them a millisecond a second.
    assert!((60..70).contains(&took), "{took} s: {said}");
}

/// Bytes asked by GETs among `asked`.
fn bytes(asked: &[server::Request]) -> u64 {
    let span = |r: &str| r.split_once('-').map_or(0, |(a, b)| b.parse::<u64>().unwrap() - a.parse::<u64>().unwrap() + 1);
    asked.iter().filter_map(|r| r.range.as_deref()).flat_map(|r| r.split(',')).map(span).sum()
}

// SPEC 6.2.5
#[test]
fn resolve_works_on_12_samples_at_once_and_lists_a_folder_once() {
    let server = Server::start();
    let mut body = String::new();
    for n in 0..13 {
        server.put(&format!("/store/s{n:02}.root"), ntuple());
        body += &format!("[[sample]]\nname = \"s{n:02}\"\nfiles = {:?}\n", server.url(&format!("/store/s{n:02}.root")));
    }
    for n in 0..3 {
        server.put(&format!("/store/own/f{n}.root"), ntuple());
    }
    body += &format!("[[sample]]\nname = \"three\"\nfiles = {:?}\n", server.url("/store/own/f*.root"));
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n{body}");
    let dir = common::study("twelve", &samples, RECIPE);
    server.script(|r| match r.method.as_str() {
        "HEAD" => Answer::Late(std::time::Duration::from_millis(300), Box::new(Answer::Serve)),
        _ => Answer::Serve,
    });
    resolved(&dir);
    assert_eq!(server.most_at_once(), 12);
    let asked = server.taken();
    let listed: Vec<&str> = asked.iter().filter(|r| r.method == "PROPFIND").map(|r| r.path.as_str()).collect();
    assert_eq!(listed, ["/store/", "/store/own/"], "each folder listed once");
    // One sample's files one after another.
    let one = Server::start();
    for n in 0..3 {
        one.put(&format!("/store/own/f{n}.root"), ntuple());
    }
    let dir = common::study("one-by-one", "", RECIPE);
    std::fs::write(dir.join("samples.toml"), format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n[[sample]]\nname = \"three\"\nfiles = {:?}\n", one.url("/store/own/f*.root"))).unwrap();
    one.script(|_| Answer::Late(std::time::Duration::from_millis(100), Box::new(Answer::Serve)));
    resolved(&dir);
    assert_eq!(one.most_at_once(), 1);
}

// SPEC 7.2.1, 7.3.4
#[test]
fn a_press_asks_only_for_the_branches_the_recipe_reads() {
    let (dir, server) = remote("branches-read", |s| format!("{:?}", s.url("/store/flat.root")));
    resolved(&dir);
    std::fs::write(dir.join("recipe.toml"), RECIPE.replace("[\"nJet\", \"Jet_pt\"]", "[\"nJet\"]")).unwrap();
    server.taken();
    assert_eq!(rvr(&dir, None, &["press"]).0, 0);
    let one = bytes(&server.taken());
    std::fs::write(dir.join("recipe.toml"), RECIPE).unwrap();
    assert_eq!(rvr(&dir, None, &["press", "--full"]).0, 0);
    let two = bytes(&server.taken());
    assert!(one < two, "{one} bytes for nJet, {two} for nJet and Jet_pt");
    // A press with holes says how they are read again.
    let (_dir, _server, code, said) = pressed("fill-said", false, failing_gets(usize::MAX, Answer::Status(404)));
    assert!(code == 3 && said.contains("rvr press --fill reads them again once they can be read."), "{said}");
}

// SPEC 6.2.1, 6.2.7
#[test]
fn a_recipe_that_does_not_fit_stops_resolve_at_each_samples_first_file() {
    let server = Server::start();
    for n in 0..3 {
        server.put(&format!("/store/own/f{n}.root"), ntuple());
    }
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\ntree = \"analysis\"\n\n[eras]\n\n[[sample]]\nname = \"three\"\nfiles = {:?}\n", server.url("/store/own/f*.root"));
    let dir = common::study("early-refusal", &samples, &RECIPE.replace("\"Jet_pt\"", "\"Jet_ptt\""));
    let (code, said) = rvr(&dir, None, &["samples", "resolve", "samples.toml"]);
    assert!(code == 1 && said.contains("  FAIL  [columns] Jet_ptt: ") && !said.contains("FAIL  three"), "{said}");
    assert!(said.contains("samples.lock was not written") && !dir.join("samples.lock").exists(), "{said}");
    let asked: Vec<String> = server.taken().iter().filter(|r| r.method != "PROPFIND").map(|r| r.path.clone()).collect();
    assert!(asked.iter().all(|p| p == "/store/own/f0.root"), "only the first file is asked: {asked:?}");
}

// SPEC 5.4.1
#[test]
fn branches_before_a_lock_opens_the_first_file_and_asks_no_digest() {
    let (dir, server) = remote("branches-remote", |s| format!("{:?}", s.url("/store/f*.root")));
    server.put("/store/flat.root", ntuple());
    server.put("/store/flat2.root", ntuple());
    let (code, said) = rvr(&dir, None, &["branches"]);
    assert!(code == 0 && said.contains("flat: flat.root, tree analysis, 17 branches"), "{said}");
    let asked = server.taken();
    assert!(asked.iter().all(|r| r.method != "HEAD" && r.path != "/store/flat2.root"), "only the listing and the first file's bytes: {asked:?}");
    assert!(!dir.join("samples.lock").exists());
}

/// Every hole of the file a press of the study in `dir` wrote: its file and its kind.
fn holes(dir: &Path) -> Vec<(String, String)> {
    let header = header(dir);
    let samples = header["made"]["files"].as_object().unwrap().values();
    let files = samples.flat_map(|sample| sample["files"].as_object().unwrap().iter());
    files.flat_map(|(at, file)| file["holes"].as_array().into_iter().flatten().map(|hole| (at.clone(), hole["kind"].as_str().unwrap().to_owned()))).collect()
}

// SPEC 7.4.1, 7.4.2
#[test]
fn a_read_sent_to_a_name_that_does_not_resolve_is_asked_again_after_each_wait() {
    // A label of 64 letters, one more than a DNS name holds: no resolver can send it, so the name
    // fails on this computer and no query leaves it.
    let nowhere = format!("https://{}.invalid/store/flat.root", "a".repeat(64));
    let (dir, _server, code, said) = pressed("unresolved", false, move |request| match request.method.as_str() {
        "GET" => Answer::Redirect(nowhere.clone()),
        _ => Answer::Serve,
    });
    assert_eq!((code, hole(&dir).as_deref()), (3, Some("unreachable")), "{said}");
    for k in 1..=7 {
        assert!(said.contains(&format!("({k} of 8 tries made): its storage's name could not be looked up")), "{said}");
    }
}

// SPEC 7.2.4, 7.4.1, 7.4.2
#[test]
fn a_read_redirected_round_and_round_is_asked_again_after_each_wait() {
    let (dir, _server, code, said) = pressed("round", false, |request| match request.method.as_str() {
        "GET" => Answer::Redirect("/store/flat.root".into()),
        _ => Answer::Serve,
    });
    assert_eq!((code, hole(&dir).as_deref()), (3, Some("unreachable")), "{said}");
    assert!(said.contains("  waiting 300 s to ask again for flat.root (7 of 8 tries made): its storage redirected more than four times"), "{said}");
}

// SPEC 7.2.5
#[test]
fn a_multipart_answer_not_laid_out_as_the_parts_asked_is_asked_again_one_range_a_request() {
    for answer in [Answer::Placeless, Answer::Garbled] {
        let shown = format!("{answer:?}");
        let (dir, server, code, said) = pressed("malformed", false, move |_| answer.clone());
        assert_eq!((code, hole(&dir)), (0, None), "{shown}: {said}");
        assert!(!said.contains("waiting"), "{shown}: {said}");
        let asked = server.taken();
        let many = asked.iter().position(|r| r.ranges() > 1).expect("a request for several ranges");
        assert!(asked[many + 1..].iter().all(|r| r.ranges() <= 1), "{shown}: {asked:?}");
    }
}

// SPEC 7.2.3, 7.3.1
#[test]
fn a_file_changed_on_its_storage_since_the_lock_is_a_changed_hole() {
    let mut longer = ntuple();
    longer.push(0);
    let mut other = ntuple();
    let middle = other.len() / 2;
    other[middle] ^= 1;
    let (size, sum) = (ntuple().len(), server::adler32(&ntuple()));
    let now = [format!("it is now {} bytes, adler32 {}", size + 1, server::adler32(&longer)), format!("it is now {size} bytes, adler32 {}", server::adler32(&other))];
    for (bytes, now) in [longer, other].into_iter().zip(now) {
        let (dir, server) = remote("changed-there", |s| format!("{:?}", s.url("/store/flat.root")));
        resolved(&dir);
        server.put("/store/flat.root", bytes);
        let (code, said) = rvr(&dir, None, &["press"]);
        assert_eq!((code, holes(&dir).len(), hole(&dir).as_deref()), (3, 1, Some("changed")), "{said}");
        assert!(said.contains(&format!("flat.root: it was {size} bytes, adler32 {sum}; {now}. It has changed since samples.lock was written")), "{said}");
    }
}

/// A study of two copies of the ntuple at two doors, the first at `localhost` and the second at
/// `host` (`localhost` again: a door of the same site; `127.0.0.1`: of another), resolved.
fn two_doors(name: &str, host: &str) -> (Study, Server, Server) {
    let (first, second) = (Server::start(), Server::start());
    first.put("/store/a.root", ntuple());
    second.put("/store/b.root", ntuple());
    let files = format!("[{:?}, {:?}]", first.url("/store/a.root"), second.url("/store/b.root").replace("localhost", host));
    let samples = format!("format = \"rvr-samples\"\nversion = 1\n\n[places]\n\n[defaults]\n\n[eras]\n\n[[sample]]\nname = \"flat\"\nfiles = {files}\ntree = \"analysis\"\n");
    let dir = common::study(name, &samples, RECIPE);
    resolved(&dir);
    (dir, first, second)
}

/// The second door's script: its first GET is served, so that it too has answered in the press;
/// its second takes the first door away (its file read by then), and is answered `answer`, as
/// are those after it until `served` is set.
fn second_door(first: Server, answer: Answer, served: Arc<AtomicBool>) -> impl FnMut(&server::Request) -> Answer + Send + 'static {
    let (mut first, mut gets) = (Some(first), 0);
    move |request| match request.method.as_str() {
        "GET" if !served.load(Ordering::SeqCst) => {
            gets += 1;
            if gets == 1 {
                return Answer::Serve;
            }
            first.take();
            answer.clone()
        }
        _ => Answer::Serve,
    }
}

// SPEC 2.6, 7.4.5, 7.5.3, 7.5.4
#[test]
fn every_door_gone_quiet_stops_the_press_and_the_next_carries_on() {
    for (name, host, why) in [
        ("quiet-two-sites", "127.0.0.1", "none that answered earlier in this press takes a connection now"),
        ("quiet-one-site", "localhost", "rvr cannot tell that site down from the network gone"),
    ] {
        let (dir, first, second) = two_doors(name, host);
        let ports = (first.port, second.port);
        second.script(second_door(first, Answer::Vanish, Default::default()));
        let (code, said) = rvr(&dir, None, &["press", "--workers", "1"]);
        assert_eq!(code, 1, "{name}: {said}");
        assert!(said.contains(why) && !said.contains("hole ("), "{name}: {said}");
        assert!(said.contains("no file was made a hole of it: fix it, then press again, which carries on from the 1 of 2 files done"), "{name}: {said}");
        assert!(!dir.join(file(&dir)).exists(), "{name}: {said}");
        drop(second);
        // Both doors back on their ports: the next press reads only the second file, and writes
        // what a press of both writes.
        let (first, second) = (Server::on("server", ports.0), Server::on("server", ports.1));
        first.put("/store/a.root", ntuple());
        second.put("/store/b.root", ntuple());
        let (code, said) = rvr(&dir, None, &["press", "--workers", "1"]);
        assert!(code == 0 && said.contains("where it stopped: 1 of 2 files done") && !said.contains("1/2"), "{name}: {said}");
        let picked = header(&dir)["data"]["sha256"].clone();
        assert_eq!(rvr(&dir, None, &["press", "--full"]).0, 0);
        assert_eq!(header(&dir)["data"]["sha256"], picked, "{name}");
    }
}

// SPEC 7.4.2, 7.4.5
#[test]
fn a_storage_down_while_another_sites_door_takes_connections_makes_holes_after_the_waits() {
    let (dir, _first, second) = two_doors("one-down", "127.0.0.1");
    let lost = second.url("/store/b.root").replace("localhost", "127.0.0.1");
    second.script(|request| match request.method.as_str() {
        "GET" => Answer::Vanish,
        _ => Answer::Serve,
    });
    let (code, said) = rvr(&dir, None, &["press", "--workers", "1"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("(7 of 8 tries made)") && !said.contains("network") && !said.contains("cannot tell"), "{said}");
    assert_eq!(holes(&dir), [(lost, "unreachable".to_owned())], "{said}");
}

// SPEC 7.4.5
#[test]
fn the_network_back_between_two_probes_lets_the_press_go_on() {
    let (dir, first, second) = two_doors("back", "127.0.0.1");
    let port = first.port;
    let served = Arc::new(AtomicBool::new(false));
    let since = Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    let mut script = second_door(first, Answer::Drop, served.clone());
    let failed = since.clone();
    second.script(move |request| {
        let answer = script(request);
        if matches!(answer, Answer::Drop) {
            failed.lock().unwrap().get_or_insert_with(std::time::Instant::now);
        }
        answer
    });
    // A second of waiting is 200 ms: no answer for 120 s is 24 s, a probe's 15 s 3 s. The first
    // probe finds no door; the first door comes back 1.5 s later, and the second probe finds it.
    let (mut child, said) = spawned_with(&dir, &[("RVR_SECOND_MS", "200")], &["press", "--workers", "1"]);
    let started = std::time::Instant::now();
    let quiet = loop {
        if let Some(at) = *since.lock().unwrap() {
            break at;
        }
        assert!(started.elapsed().as_secs() < 30, "the second file was never asked for");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    std::thread::sleep((quiet + std::time::Duration::from_millis(25_500)).saturating_duration_since(std::time::Instant::now()));
    let again = Server::on("server", port);
    again.put("/store/a.root", ntuple());
    served.store(true, Ordering::SeqCst);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed().as_secs() > 90 {
            let _ = child.kill();
            panic!("the press did not end: {}", std::fs::read_to_string(&said).unwrap());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let said = std::fs::read_to_string(said).unwrap();
    assert_eq!(status.code(), Some(0), "{said}");
    assert!(again.connections() > 0, "no probe reached the first door once it was back: {said}");
    assert!(said.contains("(5 of 8 tries made)") && !said.contains("network") && holes(&dir).is_empty(), "{said}");
}

// SPEC 7.4.2, 7.4.5
#[test]
fn a_press_no_door_ever_answered_makes_holes_after_the_waits() {
    let (dir, server) = two_files("never-heard");
    drop(server);
    let (code, said) = rvr(&dir, None, &["press", "--workers", "2"]);
    assert_eq!(code, 3, "{said}");
    assert!(said.contains("(7 of 8 tries made)") && !said.contains("network") && !said.contains("cannot tell"), "{said}");
    let kinds: Vec<String> = holes(&dir).into_iter().map(|(_, kind)| kind).collect();
    assert_eq!(kinds, ["unreachable", "unreachable"], "{said}");
}
