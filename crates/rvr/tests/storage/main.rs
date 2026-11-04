//! rvr against storage over HTTPS: a test door on localhost (server.rs), trusted through
//! `$X509_CERT_DIR`, answering as it is scripted to.

#[path = "../common/mod.rs"]
mod common;
mod server;

use common::{rvr_with, Study};
use server::{Answer, Server};
use std::path::Path;

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
        (Answer::Status(503), false, "its storage answered 503"),
        (Answer::Drop, false, ""),
        (Answer::Short, false, ""),
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

// SPEC 7.4.3
#[test]
fn some_answers_are_taken_at_once() {
    for (answer, kind) in [(Answer::Status(404), "gone"), (Answer::Status(410), "gone"), (Answer::Whole, "unreachable"), (Answer::Status(418), "unreachable")] {
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
