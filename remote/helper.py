"""RowVer remote-columns helper: a local daemon the page fetches columns through.

Why it exists (measured, LOG.md R0/R1): the browser cannot reach CMS storage itself —
grid-CA TLS, VOMS-proxy client auth, no CORS, capability redirects — but THIS process
can, over xrootd with the grid proxy, and selective branch reads cost ~0.6% of the
file's bytes. The xrootd auth handshake costs ~60 s ONCE PER PROCESS, so the helper is
a resident daemon that pays it once and then serves opens in well under a second.

Run, from a clone of the repo (any python3 with numpy + uproot + fsspec-xrootd; it
prints the install one-liner itself if an import is missing):

    python3 remote/helper.py [port]

Everything machine-specific is discovered: the grid proxy (X509_USER_PROXY, else
/tmp/x509up_u$UID; OPTIONAL — public eospublic data works anonymously), the CA bundle
(X509_CERT_DIR, else the standard grid/cvmfs locations), the port (arg or
ROWVER_HELPER_PORT, default 8271), the eos endpoint (ROWVER_EOS). On a cluster node
(recommended: credentials and CAs already live there, eos is local) reach it from your
laptop's browser with:  ssh -L 8271:localhost:8271 <cluster-host>

Endpoints (all CORS-open, meant for the RowVer page on THIS machine only):
    GET  /ping                          liveness + proxy expiry
    GET  /files?dataset=/A/B/C          DBS: dataset -> [{url, size, events}]
    GET  /files?eos=/store/user/...     eos: directory -> [{url, size}]
    GET  /branches?file=URL&tree=NAME   branch inventory of one tree
    POST /build   (json spec)           start building a .rvr; -> {job, cached}
    GET  /status?job=ID                 {phase, done, total, note | error}
    GET  /fetch?job=ID                  the finished .rvr bytes

The build spec is what a data-less .rvr carries under META.remote (make_dataless.py
writes them): fileset (coffea shape; an entry is either {dataset, tree, limit} — a DAS
name resolved at build time, smallest files first — or {files: {url: tree}} — an
explicit list, which is also how a chosen file SUBSET is represented), branches
{column: branch}, optional cut string (numpy expression over the columns, evaluated
here so the page never swallows raw event counts), optional labels/weights,
tabs = [{name, view}] passed to page.state. Every freshly built .rvr records its
provenance under the optional META key `origin`: the spec as submitted AND the
resolved {url: tree} fileset it was actually read from (no timestamp, so identical
specs still build byte-identical files).
Finished files are cached in remote/cache/<sha16>.rvr keyed by the whole spec, so a
second drop of the same file is served in milliseconds. Per-file wire bytes and
timings are recorded in remote/metrics/ — the study's numbers come from there.
"""
from __future__ import annotations

import contextlib
import errno
import hashlib
import json
import os
import socket
import ssl
import sys
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

try:
    # all three, not just numpy: a half-installed stack used to start cleanly and then
    # fail as an HTTP 500 in the browser, where the missing package is invisible
    import numpy as np
    import uproot                                # noqa: F401
    import XRootD.client                         # noqa: F401
except ImportError as e:
    sys.exit(f"helper.py cannot import {e.name or e} — it needs numpy, uproot and "
             "fsspec-xrootd:\n    python3 -m pip install --user numpy uproot fsspec-xrootd")

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))          # the repo root, for page.py
try:
    import page                               # noqa: E402
except ImportError:
    sys.exit("helper.py needs page.py from the repo root one directory up — run it "
             "from a clone:\n    git clone https://gitlab.cern.ch/dgrove/rowver\n"
             "    python3 rowver/remote/helper.py")

#: everything machine-specific is DISCOVERED or overridable — nothing here names a
#: particular person or laptop, so the same file runs on any machine or cluster node.
if len(sys.argv) > 1 and not sys.argv[1].isdigit():
    print("usage: python3 remote/helper.py [port]      (default 8271, or "
          "ROWVER_HELPER_PORT)\n"
          "  reach it from your laptop's browser with:\n"
          "      ssh -L 8271:localhost:8271 <cluster-host>\n"
          "  other knobs: ROWVER_EOS (which lab's eos to browse), X509_USER_PROXY, "
          "X509_CERT_DIR")
    sys.exit(0 if sys.argv[1] in ("-h", "--help", "help") else 2)
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else int(os.environ.get("ROWVER_HELPER_PORT", "8271"))
#: openssl's short names, so the subject reads the way voms-proxy-info prints it
RDN_SHORT = {"domainComponent": "DC", "countryName": "C", "organizationName": "O",
             "organizationalUnitName": "OU", "commonName": "CN",
             "stateOrProvinceName": "ST", "localityName": "L"}


def decode_cert(path: str) -> dict:
    """A PEM certificate's notAfter and subject, with no subprocess (see above)."""
    try:
        import _ssl
        cert = _ssl._test_decode_cert(path)
        rdns = [(RDN_SHORT.get(k, k), v) for rdn in cert.get("subject", ()) for k, v in rdn]
        cns = [v for k, v in rdns if k == "CN"]
        return {"expires": cert.get("notAfter", "unreadable"),
                "subject": "".join(f"/{k}={v}" for k, v in rdns) or "unreadable",
                "cn": cns[0] if cns else "unreadable"}
    except Exception as e:
        return {"expires": "unreadable", "cn": "unreadable",
                "subject": f"unreadable ({type(e).__name__}: {e})"}


def expires_in(path: str) -> float | None:
    """Seconds until this proxy expires; None if it will not decode at all."""
    from datetime import datetime, timezone
    try:
        exp = datetime.strptime(decode_cert(path)["expires"], "%b %d %H:%M:%S %Y %Z")
    except ValueError:
        return None
    return exp.replace(tzinfo=timezone.utc).timestamp() - time.time()


def find_proxy() -> str:
    """Where voms-proxy-init ACTUALLY left it. The env override always wins; otherwise
    look in every place it is known to land — most sites write /tmp, but cmslpc wrote
    $HOME (measured 2026-08-20, and the old /tmp-only guess simply missed it).
    A proxy that is expired or unreadable must not mask a good one further down the
    list, so candidates are ranked before they are chosen rather than taken in order."""
    env = os.environ.get("X509_USER_PROXY")
    if env:
        return env
    uid = os.getuid()
    here = [p for p in (f"/tmp/x509up_u{uid}", str(Path.home() / f"x509up_u{uid}"),
                        f"/tmp/x509up_p{uid}", str(Path.home() / ".globus" / f"x509up_u{uid}"))
            if Path(p).is_file()]
    for path in here:
        left = expires_in(path)
        if left is not None and left > 0:
            return path
    return here[0] if here else f"/tmp/x509up_u{uid}"


def find_certs() -> str | None:
    """The grid CA bundle: the env override first, then the standard locations."""
    if os.environ.get("X509_CERT_DIR"):
        return os.environ["X509_CERT_DIR"]
    for c in ("/etc/grid-security/certificates",
              "/cvmfs/grid.cern.ch/etc/grid-security/certificates",
              "/cvmfs/oasis.opensciencegrid.org/mis/certificates",
              *sorted(Path.home().glob(
                  ".local/share/mamba/envs/*/etc/grid-security/certificates"))):
        if Path(c).is_dir():
            return str(c)
    return None

PROXY = find_proxy()
CERTS = find_certs()
HAVE_PROXY = Path(PROXY).exists()
if HAVE_PROXY:
    # exported because uproot and the xrootd client read the ENV, not this variable
    os.environ["X509_USER_PROXY"] = PROXY
if CERTS:
    os.environ["X509_CERT_DIR"] = CERTS

DBS = "https://cmsweb.cern.ch/dbs/prod/global/DBSReader"
REDIRECTOR = os.environ.get("ROWVER_REDIRECTOR", "root://cmsxrootd.fnal.gov/")
#: eos means a different machine at each lab, and browsing the wrong one answers "no
#: such file or directory" for a path that plainly exists — so follow the host we are
#: actually running on, and let ROWVER_EOS override for anywhere else.
EOS = os.environ.get("ROWVER_EOS") or (
    "root://eosuser.cern.ch/" if socket.getfqdn().endswith(".cern.ch")
    else "root://cmseos.fnal.gov/")
CACHE = HERE / "cache"
METRICS = HERE / "metrics"
CACHE.mkdir(exist_ok=True)
METRICS.mkdir(exist_ok=True)

JOBS: dict = {}
JOBS_LOCK = threading.Lock()
START = time.time()
#: A helper must not outlive the person running it. It holds a FIXED port on a machine
#: that is usually SHARED, and it answers with whatever credentials that person has —
#: so an orphan is both a blocked port for them and a live proxy for everyone else on
#: the node. It retires two ways: the moment its terminal proves gone (an unwritable
#: log), and after this many minutes with nobody asking it anything. The page polls
#: /ping every ~1.5 s per open tab, so an open browser IS the heartbeat; nothing asking
#: means the tunnel, the tab, or the person is gone. 0 disables the idle half.
#: How many worker PROCESSES read files at once. "auto" races serial against a small
#: pool on the job's own files and keeps the winner; a number forces it, and 1 forces
#: the old serial path. Forcing exists because auto-tuning is a measurement, and a
#: measurement can be wrong on a link nobody has tried yet — measured 2026-08-20, the
#: right answer swings from 4.3x FASTER (cmslpc, CPU-bound) to no gain at all (laptop
#: over WAN), and no cheap property of the link predicts which.
WORKERS = os.environ.get("ROWVER_WORKERS", "auto").strip().lower()
PROBE_ENTRIES = int(os.environ.get("ROWVER_PROBE_ENTRIES", "20000"))
TUNED: dict = {}                    # endpoint -> {"workers": n, "why": str}
IDLE_MIN = float(os.environ.get("ROWVER_IDLE_MIN", "30"))
LAST_SEEN = time.time()
SRV = None
#: the process that started us — the remote shell an ssh session owns. When that session
#: ends the shell dies and we are reparented to init, which is the ONLY prompt signal
#: available: a dead tunnel means no more requests, so waiting to notice a failed log
#: write never fires (measured 2026-08-20 — the helper outlived its killed tunnel).
#: Already-1 at startup means someone detached us deliberately (setsid); leave them be
#: and let the idle timer do the work.
PARENT = os.getppid()
#: the once-per-process xrootd auth handshake, observed from the first real open
HANDSHAKE = {"done": False, "first_open_s": None}


#: NOTHING in this process may fork. XRootD registers a pthread_atfork handler that
#: stops its poller, and a fork raced against an in-flight xrootd operation deadlocks
#: the interpreter with the GIL held — the helper goes silent and only SIGKILL ends it.
#: Measured 2026-08-20: /ping used to shell out to `openssl` twice, and the page polls
#: /ping every ~1.5 s per open tab; a browse-while-polling stress froze the helper in
#: 22 s (macOS) and Derek's own helper wedged after five minutes. So the proxy is
#: decoded IN-PROCESS, and cached by the file's mtime so a renewal is still noticed.
#: ONE thread in xrootd at a time, and every handle closed on the thread that opened
#: it. XRootD's python layer shares a file-handle cache that is NOT thread-safe: with
#: two threads finishing files at once, one closes a handle the other just invalidated
#: and the request dies with "Failed to submit File.close". MEASURED 2026-08-20 on the
#: real helper: one browsing tab + a build = 0 failures in 15 calls, but TWO tabs = 12
#: failures in 62 (19%), three tabs = 27%. Every open here was also relying on garbage
#: collection to close, so closes fired on whatever thread the GC ran on — which is the
#: race. Held per OPERATION and per FILE, never for a whole build, so a browse waits at
#: most one file rather than the length of the job.
XROOTD = threading.RLock()


@contextlib.contextmanager
def xrd_open(url: str, timeout: int = 300):
    """uproot.open, serialised, and closed deterministically inside the lock."""
    import uproot
    with XROOTD:
        f = uproot.open(url, timeout=timeout)
        try:
            yield f
        finally:
            try:
                f.close()
            except Exception:
                pass


PROXY_CACHE: dict = {}


def proxy_info() -> dict:
    """Identity + expiry, for the console's countdown and its dead-proxy banner."""
    out = {"path": PROXY, "expires": "no proxy", "seconds_left": None,
           "subject": "none — anonymous (public storage only)", "cn": "anonymous",
           "expired": False}
    try:
        stamp = Path(PROXY).stat().st_mtime_ns
    except OSError:
        return out
    if PROXY_CACHE.get("stamp") != stamp:
        PROXY_CACHE.update(stamp=stamp, **decode_cert(PROXY))
    out.update({k: PROXY_CACHE[k] for k in ("expires", "subject", "cn")})
    try:
        from datetime import datetime, timezone
        exp = datetime.strptime(out["expires"], "%b %d %H:%M:%S %Y %Z")
        out["seconds_left"] = int(exp.replace(tzinfo=timezone.utc).timestamp() - time.time())
        out["expired"] = out["seconds_left"] <= 0
    except ValueError:
        pass
    return out


def note_first_open(seconds: float):
    if not HANDSHAKE["done"]:
        HANDSHAKE["done"] = True
        HANDSHAKE["first_open_s"] = round(seconds, 2)


def dbs_get(path_qs: str):
    # asked LIVE, not from the startup snapshot: the documented path is to start with
    # no proxy (the open-data quickstart), then run voms-proxy-init — and the old gate
    # went on refusing afterwards, telling you to do the thing you had just done
    if not Path(PROXY).exists():
        raise RuntimeError(f"DAS/DBS needs a grid proxy and none exists at {PROXY} — "
                           "run voms-proxy-init -voms cms (public eos browsing still "
                           "works without one)")
    os.environ["X509_USER_PROXY"] = PROXY
    ctx = ssl.create_default_context(capath=CERTS)
    ctx.load_cert_chain(PROXY, PROXY)
    with urllib.request.urlopen(f"{DBS}/{path_qs}", context=ctx, timeout=120) as r:
        return json.load(r)


def dbs_datasets(pattern: str) -> list:
    """DAS-style dataset NAME search — DBS is the discovery service; rucio would add
    replica LOCATION, not name search (STUDY.md assessment)."""
    if not pattern.startswith("/"):
        pattern = "/" + pattern
    q = urllib.parse.urlencode({"dataset": pattern, "dataset_access_type": "VALID"})
    rows = dbs_get(f"datasets?{q}")
    return [x["dataset"] for x in rows]


def dbs_summary(dataset: str) -> dict:
    rows = dbs_get("filesummaries?" + urllib.parse.urlencode({"dataset": dataset}))
    if not rows:
        return {"dataset": dataset}
    s = rows[0]
    return {"dataset": dataset, "files": s.get("num_file"),
            "events": s.get("num_event"), "bytes": s.get("file_size")}


def dbs_files(dataset: str) -> list:
    """DBS REST with the proxy as client cert — dataset -> file list with sizes."""
    ctx = ssl.create_default_context(capath=CERTS)
    ctx.load_cert_chain(PROXY, PROXY)
    q = urllib.parse.urlencode({"dataset": dataset, "validFileOnly": 1, "detail": 1})
    with urllib.request.urlopen(f"{DBS}/files?{q}", context=ctx, timeout=120) as r:
        rows = json.load(r)
    return [{"url": REDIRECTOR + f["logical_file_name"],
             "size": f["file_size"], "events": f["event_count"]} for f in rows]


def eos_ls(path: str) -> dict:
    """One eos directory as the console browses it: subdirectories AND files."""
    from XRootD import client
    fs = client.FileSystem(EOS)
    status, listing = fs.dirlist(path, flags=client.flags.DirListFlags.STAT)
    if not status.ok:
        raise RuntimeError(f"eos dirlist {EOS}{path.lstrip('/')}: {status.message}"
                           f"  (this helper browses {EOS} — set ROWVER_EOS and restart "
                           f"it to browse a different lab's eos)")
    dirs, files = [], []
    for e in listing:
        if e.statinfo and (e.statinfo.flags & client.flags.StatInfoFlags.IS_DIR):
            dirs.append(e.name)
        else:
            files.append({"name": e.name,
                          "url": f"{EOS}/{path.rstrip('/')}/{e.name}",
                          "size": e.statinfo.size if e.statinfo else None})
    return {"path": path, "dirs": sorted(dirs), "files": sorted(files, key=lambda f: f["name"])}


def eos_files(path: str) -> list:
    """The original flat file listing (kept for compatibility with existing callers)."""
    got = eos_ls(path)
    return [{"url": f["url"], "size": f["size"]} for f in got["files"]]


def list_trees(url: str) -> dict:
    """Every TTree in one file, so a wrong tree name can be corrected against reality."""
    t0 = time.time()
    with xrd_open(url, 180) as f:
        trees = sorted({k.split(";")[0] for k, cls in f.classnames().items()
                        if cls == "TTree"})
        note_first_open(time.time() - t0)
        return {"file": url, "trees": trees}


def branch_inventory(url: str, tree: str) -> dict:
    t0 = time.time()
    with xrd_open(url, 180) as f:
        t = f[tree]
        note_first_open(time.time() - t0)
        return {"entries": int(t.num_entries),
                "branches": [{"name": b.name, "type": str(b.interpretation),
                              # the page greys out what the flat .rvr format cannot hold
                              "flat": not any(k in str(b.interpretation)
                                              for k in ("AsJagged", "AsObjects", "AsStrings"))}
                             for b in t.branches]}


def cache_inventory() -> list:
    now = time.time()
    out = []
    for f in sorted(CACHE.glob("*.rvr"), key=lambda p: -p.stat().st_mtime):
        out.append({"name": f.name, "sha": f.stem, "MB": round(f.stat().st_size / 1e6, 3),
                    "age_s": int(now - f.stat().st_mtime),
                    "metrics": (METRICS / f"{f.stem}.json").exists()})
    return out


def metrics_inventory() -> list:
    out = []
    for f in sorted(METRICS.glob("*.json"), key=lambda p: -p.stat().st_mtime):
        try:
            out.append(json.load(open(f)))
        except Exception:
            pass
    return out


def spec_sha(spec: dict) -> str:
    return hashlib.sha256(
        json.dumps(spec, sort_keys=True, separators=(",", ":")).encode()).hexdigest()[:16]


def eval_cut(cut: str, cols: dict) -> np.ndarray:
    """The optional KEEP cut, a numpy expression over the loaded columns. Evaluated
    with no builtins; the spec comes from a file Derek wrote on this machine."""
    space = dict(cols)
    space["np"] = np
    space["abs"] = np.abs
    mask = eval(cut, {"__builtins__": {}}, space)
    return np.asarray(mask, bool)


#: entries per read chunk — the honest granularity of every live counter below
CHUNK = 500_000


def _warm(_):
    """Spin a worker all the way up — fork, interpreter, uproot import — so that
    one-time cost is not charged to whichever wave happens to time it."""
    import uproot                                   # noqa: F401
    return True


def pool_ctx():
    """A context that forks from the FORKSERVER, never from us. This process holds live
    XRootD state and an atfork handler that deadlocks if we fork while a read is in
    flight (that is the bug efc3285 fixed), so the forkserver is started at startup
    while nothing is happening and every later pool is forked by IT."""
    import multiprocessing as mp
    return mp.get_context("forkserver" if "forkserver" in mp.get_all_start_methods()
                          else "spawn")


def plan_workers(files) -> tuple:
    """How many workers, and whether we still need to find out. The override always
    wins; a verdict already measured against this endpoint is reused; otherwise we go
    in CALIBRATING and let the build itself answer the question."""
    endpoint = files[0][0].split("//")[1].split("/")[0] if files else "?"
    if WORKERS.isdigit():
        return max(1, int(WORKERS)), endpoint, False
    if endpoint in TUNED:
        return TUNED[endpoint]["workers"], endpoint, False
    cap = min(8, os.cpu_count() or 2, len(files))
    wave = min(3, cap)
    if cap < 2 or len(files) < 2 * wave:
        return 1, endpoint, False        # too small for the answer to be worth having
    return cap, endpoint, True


def read_file(task: tuple) -> tuple:
    """One file, start to finish: open, check, read, cut. TOP-LEVEL and picklable on
    purpose — the identical function runs inline for a serial build and inside a worker
    process for a parallel one, so the two paths cannot drift apart. Workers cannot
    touch the job dict, so everything the parent needs comes back in `stats`."""
    import uproot
    idx, name, url, tree, want, branches, cut, limit = task
    t0 = time.time()
    with xrd_open(url, 300) as f:
        return _read_open(f, task, t0)


def _read_open(f, task, t0):
    idx, name, url, tree, want, branches, cut, limit = task
    t = f[tree]
    missing = [b for b in want if b not in t]
    if missing:
        raise RuntimeError(f"{name}: {url} has no branch(es) {missing}")
    jag = [b for b in want if "AsJagged" in str(t[b].interpretation)
           or "AsObjects" in str(t[b].interpretation)]
    if jag:
        raise RuntimeError(
            f"{name}: branch(es) {', '.join(jag)} are not one number per event — the "
            f".rvr format is flat. Reduce them in a skim, or drop them from the columns.")
    n_tot = int(t.num_entries)
    parts: dict = {c: [] for c in branches}
    read = kept = 0
    for arrs in t.iterate(want, step_size=CHUNK, library="np",
                          entry_stop=limit if limit else None):
        read += len(arrs[want[0]])
        cols = {c: arrs[b] for c, b in branches.items()}
        if cut:
            keep = eval_cut(cut, cols)
            cols = {c: v[keep] for c, v in cols.items()}
        kept += len(next(iter(cols.values())))
        for c, v in cols.items():
            parts[c].append(v)
    out = {c: (np.concatenate(v) if v else np.array([], np.float64))
           for c, v in parts.items()}
    stats = {"idx": idx, "dataset": name, "url": url, "entries": n_tot,
             "read": read, "kept": kept,
             "file_MB": round(f.file.source.num_bytes / 1e6, 2),
             "disk_bytes": f.file.source.num_bytes,
             "wire_bytes": f.file.source.num_requested_bytes,
             "wire_MB": round(f.file.source.num_requested_bytes / 1e6, 3),
             "seconds": round(time.time() - t0, 2)}
    return out, stats


def load_dataset(name: str, entry: dict, branches: dict, cut: str | None, job: dict):
    """All requested branches of one dataset, cut applied, as {column: array}.
    Files are read serially or across worker processes — tune_workers decides which,
    and the SAME read_file does the work either way. Results are re-ordered by file
    index before concatenation, so a parallel build is byte-identical to a serial one."""
    want = sorted(set(branches.values()))
    files = list(entry["files"].items())
    cap, endpoint, calibrating = plan_workers(files)
    nproc = 1 if calibrating else cap
    job["workers"] = nproc
    job["tuning"] = ("measuring on the first files…" if calibrating
                     else (TUNED.get(endpoint, {}).get("why")
                           or (f"forced by ROWVER_WORKERS={WORKERS}" if WORKERS.isdigit()
                               else "too few files to be worth tuning")))
    tasks = [(i, name, u, tr, want, branches, cut, None)
             for i, (u, tr) in enumerate(files)]
    got: dict = {}

    def absorb(cols, st):
        got[st["idx"]] = cols
        job["entries_read"] += st["read"]
        job["entries_kept"] += st["kept"]
        job["disk_bytes"] += st["disk_bytes"]
        job["_wire_done"] += st["wire_bytes"]
        job["wire_bytes"] = job["_wire_done"]
        job["file_log"].append({k: st[k] for k in
                                ("dataset", "url", "entries", "file_MB", "wire_MB", "seconds")})
        job["done_files"] += 1
        job["current"] = None
        job["note"] = (f"{name}: {job['done_files']}/{job['total_files']} files"
                       + (f" · {job['workers']} workers" if job["workers"] > 1 else ""))

    def run_serial(batch):
        for task in batch:
            base = task[2].rstrip("/").split("/")[-1]
            job["phase"] = "opening"
            job["current"] = {"dataset": name, "file": base, "entries_total": None,
                              "entries_read": 0, "wire_MB": 0.0, "file_MB": None}
            job["note"] = f"opening {base}"
            cols, st = read_file(task)
            job["phase"] = "reading"
            absorb(cols, st)

    #: ONE pool for the whole build. Creating one per wave charged the parallel wave a
    #: fork it did not owe and the serial wave nothing, which made the calibration lie:
    #: it read 1.1x on cmslpc where the true figure is 1.8x, and chose serial.
    POOL = {"p": None}

    def run_pool(batch, n):
        if POOL["p"] is None:
            POOL["p"] = pool_ctx().Pool(n)
        # imap_unordered so counters move as files land; `idx` in stats restores order
        for cols, st in POOL["p"].imap_unordered(read_file, batch):
            absorb(cols, st)

    job["phase"] = "reading"
    if not calibrating:
        (run_serial if nproc <= 1 else lambda b: run_pool(b, nproc))(tasks)
    else:
        #: CALIBRATE ON REAL WORK. A separate probe has to open files twice, and on a
        #: latency-bound link opening IS the cost — measured, a taste-first probe turned
        #: a 24 s build into 40 s. So the first wave runs serially and the second in
        #: parallel, both keeping their data, and the measured per-file rate picks the
        #: remainder. Nothing is read twice and nothing is thrown away.
        wave = min(3, cap)
        job["tuning"] = "assessing the fastest setup for this storage…"
        #: compare ENTRIES PER SECOND, not seconds per file. The fileset is sorted
        #: smallest-first, so wave two is strictly bigger than wave one and a per-file
        #: comparison is invalid — measured, it read 1.1x on cmslpc where forcing the
        #: workers actually gives 1.8x, and so it talked itself into staying serial.
        n0 = job["entries_read"]; t0 = time.time()
        run_serial(tasks[:wave])
        ser_rate = (job["entries_read"] - n0) / max(time.time() - t0, 1e-9)
        POOL["p"] = pool_ctx().Pool(cap)
        POOL["p"].map(_warm, range(cap))            # started and warm BEFORE we time it
        n0 = job["entries_read"]; t0 = time.time()
        run_pool(tasks[wave:2 * wave], cap)
        par_rate = (job["entries_read"] - n0) / max(time.time() - t0, 1e-9)
        gain = par_rate / ser_rate if ser_rate > 0 else 1.0
        nproc, why = ((cap, f"{gain:.1f}x faster with {cap} workers") if gain >= 1.25
                      else (1, f"serial — {cap} workers measured only {gain:.1f}x"))
        TUNED[endpoint] = {"workers": nproc, "why": why}
        job["workers"], job["tuning"] = nproc, why
        rest = tasks[2 * wave:]
        if rest:
            (run_serial if nproc <= 1 else lambda b: run_pool(b, nproc))(rest)
    if POOL["p"] is not None:
        POOL["p"].close(); POOL["p"].join()

    parts = {c: [got[i][c] for i in sorted(got)] for c in branches}
    for c in parts:
        if not any(len(v) for v in parts[c]):
            parts[c] = [np.array([], np.float64)]
    out = {}
    for c, chunks in parts.items():
        v = np.concatenate(chunks) if chunks else np.array([], np.float64)
        if v.dtype == bool:
            v = v.astype(np.int32)
        elif np.issubdtype(v.dtype, np.integer):
            lo = int(v.min()) if len(v) else 0
            hi = int(v.max()) if len(v) else 0
            v = v.astype(np.int32) if -2**31 <= lo and hi < 2**31 else v.astype(np.float64)
        else:
            v = v.astype(np.float64)
        out[c] = v
    return out


def build_rvr(spec: dict, job: dict) -> Path:
    """The whole build: load every dataset, pack with page.py, write the cache file."""
    sha = job["sha"]
    submitted = json.loads(json.dumps(spec))   # the spec as received, for provenance
    fileset = spec["fileset"]
    branches = spec["branches"]
    cut = spec.get("cut")
    labels = {c: {int(k): v for k, v in m.items()}
              for c, m in (spec.get("labels") or {}).items()} or None
    weights = spec.get("weights")

    # a fileset entry may name a DAS dataset instead of frozen urls — resolved HERE,
    # at build time, smallest files first (honest for a demo; a real study lists all)
    job["phase"] = "resolving datasets"
    for name, entry in fileset.items():
        if "files" not in entry and "dataset" in entry:
            rows = sorted(dbs_files(entry["dataset"]), key=lambda f: f["size"])
            if entry.get("limit"):
                rows = rows[:int(entry["limit"])]
            entry["files"] = {f["url"]: entry.get("tree", "Events") for f in rows}

    # EVERY distinct (tree, first-file) pair is checked BEFORE any loading starts, so
    # a wrong tree (the classic: NanoAOD's 'Events' against a custom ntuple) fails in
    # seconds with the file's actual inventory, not minutes into a build.
    job["phase"] = "checking trees"
    import uproot
    for name, entry in fileset.items():
        first_of = {}
        for url, tree in entry["files"].items():
            first_of.setdefault(tree, url)
        for tree, url in first_of.items():
            t0 = time.time()
            with xrd_open(url, 300) as f:
                note_first_open(time.time() - t0)
                trees = sorted({k.split(";")[0] for k, cls in f.classnames().items()
                                if cls == "TTree"})
            if tree not in trees:
                raise RuntimeError(
                    f"{name}: the spec asks for tree '{tree}' but "
                    f"{url.rsplit('/', 1)[-1]} has no such tree. Its trees: "
                    f"{', '.join(trees) or 'none'}. Set the spec's tree to one of "
                    f"those — a custom ntuple is usually not 'Events'.")

    job["total_files"] = sum(len(e["files"]) for e in fileset.values())
    job["phase"] = "loading"
    t0 = time.time()
    # SERIAL on purpose. A ThreadPool over datasets was tried and MEASURED (LOG R3/R4):
    # it bought 1.2x on the wire-bound big-file case (~7 MB/s home network) and then
    # RACED inside the XRootD python client on an 8-file build ("Failed to submit
    # File.close ... Invalid operation"). skim_driver's proven answer is processes,
    # not threads — revisit with a ProcessPool if parallel ever matters here.
    columns, dropped = {}, []
    for name, entry in fileset.items():
        cols = load_dataset(name, entry, branches, cut, job)
        first = next(iter(cols.values()))
        if len(first) == 0:
            dropped.append(name)
            continue
        columns[name] = cols
    if not columns:
        raise RuntimeError("every dataset came back empty after the cut")
    load_s = time.time() - t0

    job["phase"] = "packing"
    meta, buf = page.pack(columns, labels, weights)
    meta["made_by"] = page.VERSION
    tabs = []
    for t in spec.get("tabs") or [{"name": "remote", "view": {}}]:
        view = dict(t.get("view") or {})
        plots = view.pop("plots", None) or [{"varx": meta["cols"][0]["name"], "nb": 40}]
        tabs.append({"name": t.get("name", "remote"),
                     "state": page.state(meta, plots=plots, **view)})
    meta["tabs"], meta["tab"] = tabs, int(spec.get("tab", 0))
    grouping = page.page_grouping(tabs[0]["state"])
    if grouping:
        meta["grouping"] = grouping
    meta["title"] = spec.get("title", "remote") + ".rvr"
    # PROVENANCE (Derek's question, 2026-08-20): the built file records BOTH the spec as
    # submitted (dataset names, subsets, cut, branches, weights, tabs) and the RESOLVED
    # fileset it was actually read from — post-DAS-resolution, subset included, exactly
    # the {url: tree} map load_dataset consumed. `origin` is an optional META key; the
    # format stays rowver 4, readers ignore it, old files are untouched. Deliberately no
    # timestamp: two builds of one spec stay byte-identical (the R3 determinism property)
    # unless DAS itself changed.
    meta["origin"] = {"spec": submitted,
                      "resolved": {name: entry["files"]
                                   for name, entry in fileset.items()}}
    header = json.dumps(meta).encode()
    import gzip as gz
    out = CACHE / f"{sha}.rvr"
    tmp = out.with_suffix(".tmp")
    tmp.write_bytes(gz.compress(
        len(header).to_bytes(4, "little") + header + buf, 6, mtime=0))
    tmp.rename(out)

    job["metrics"] = {
        "spec_sha": sha, "title": spec.get("title", ""),
        "datasets": len(columns), "dropped_empty": dropped,
        "files": job["total_files"], "events_kept": int(meta["n"]),
        "wire_MB": round(job["wire_bytes"] / 1e6, 3),
        "load_seconds": round(load_s, 1),
        "total_seconds": round(time.time() - t0, 1),
        "rvr_MB": round(out.stat().st_size / 1e6, 3),
        "per_file": job["file_log"],
    }
    (METRICS / f"{sha}.json").write_text(json.dumps(job["metrics"], indent=1))
    return out


def run_job(job_id: str, spec: dict):
    job = JOBS[job_id]
    try:
        out = build_rvr(spec, job)
        job["path"] = str(out)
        job["phase"] = "done"
    except Exception as e:                      # surfaced to the page, not swallowed
        job["phase"] = "error"
        job["error"] = f"{type(e).__name__}: {e}"


class Handler(BaseHTTPRequestHandler):
    def _head(self, code=200, ctype="application/json", length=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        if length is not None:
            self.send_header("Content-Length", str(length))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "content-type")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.end_headers()

    def _json(self, obj, code=200):
        body = json.dumps(obj).encode()
        self._head(code, length=len(body))
        self.wfile.write(body)

    def do_OPTIONS(self):
        self._head(204)

    def log_message(self, fmt, *args):
        # An unwritable log used to take the RESPONSE down with it while the process
        # lived on (measured 2026-08-20: an orphan on cmslpc345 answered every request
        # with an empty reply for two hours while holding the port). It must not — but
        # it is also the earliest proof the terminal is gone, so it is what retires us.
        try:
            sys.stderr.write("%s %s\n" % (time.strftime("%H:%M:%S"), fmt % args))
        except OSError:
            die("its terminal went away")

    def handle_one_request(self):
        global LAST_SEEN
        LAST_SEEN = time.time()
        return super().handle_one_request()

    def do_GET(self):
        try:
            u = urllib.parse.urlparse(self.path)
            q = dict(urllib.parse.parse_qsl(u.query))
            if u.path == "/ping":
                pinfo = proxy_info()
                return self._json({"ok": True, "helper": "rowver-remote",
                                   "version": page.VERSION, "port": PORT, "eos": EOS,
                                   "workers": WORKERS, "tuned": TUNED,
                                   "proxy_expires": ("EXPIRED " + pinfo["expires"])
                                                    if pinfo["expired"] else pinfo["expires"],
                                   "proxy": pinfo,
                                   "uptime_s": int(time.time() - START),
                                   "handshake": HANDSHAKE,
                                   "jobs": {k: v.get("phase") for k, v in JOBS.items()}})
            if u.path == "/datasets":
                return self._json(dbs_datasets(q["pattern"]))
            if u.path == "/datasetinfo":
                return self._json(dbs_summary(q["dataset"]))
            if u.path == "/ls":
                return self._json(eos_ls(q["eos"]))
            if u.path == "/trees":
                return self._json(list_trees(q["file"]))
            if u.path == "/cache":
                return self._json(cache_inventory())
            if u.path == "/metrics":
                return self._json(metrics_inventory())
            if u.path == "/files":
                if "dataset" in q:
                    return self._json(dbs_files(q["dataset"]))
                if "eos" in q:
                    return self._json(eos_files(q["eos"]))
                return self._json({"error": "need ?dataset= or ?eos="}, 400)
            if u.path == "/branches":
                return self._json(branch_inventory(q["file"], q.get("tree", "Events")))
            if u.path == "/status":
                job = JOBS.get(q.get("job", ""))
                if not job:
                    return self._json({"error": "no such job"}, 404)
                keys = ("phase", "done_files", "total_files", "note", "error",
                        "workers", "tuning",
                        "current", "entries_read", "entries_kept")
                out = {k: job.get(k) for k in keys}
                # elapsed on the HELPER's clock, wire from uproot's own meter — the
                # console derives rates from these, it never invents a number
                out["elapsed_s"] = (round(time.time() - job["t0"], 1)
                                    if job.get("t0") else None)
                out["wire_MB"] = round(job.get("wire_bytes", 0) / 1e6, 3)
                out["disk_MB_seen"] = round(job.get("disk_bytes", 0) / 1e6, 1)
                out["chunk_entries"] = CHUNK
                return self._json(out)
            if u.path == "/fetch":
                job = JOBS.get(q.get("job", ""))
                if not job or job.get("phase") != "done":
                    return self._json({"error": "job not finished"}, 409)
                data = Path(job["path"]).read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(data)))
                # a cross-origin download attribute is ignored by the browser, so the
                # filename must come from here or the save is literally called "fetch"
                fname = (job.get("title") or job["sha"]) + ".rvr"
                self.send_header("Content-Disposition", f'attachment; filename="{fname}"')
                self.send_header("Access-Control-Allow-Origin", "*")
                self.end_headers()
                return self.wfile.write(data)
            return self._json({"error": f"no endpoint {u.path}"}, 404)
        except Exception as e:
            return self._json({"error": f"{type(e).__name__}: {e}"}, 500)

    def do_POST(self):
        try:
            u = urllib.parse.urlparse(self.path)
            if u.path == "/dataless":
                # the spec back as a data-less .rvr, for the console's download button
                import make_dataless
                import tempfile
                n = int(self.headers.get("Content-Length", "0"))
                spec = json.loads(self.rfile.read(n))
                with tempfile.TemporaryDirectory() as td:
                    out = make_dataless.write(
                        Path(td) / (spec.get("title", "remote") + ".rvr"), spec)
                    data = out.read_bytes()
                self._head(200, "application/octet-stream", len(data))
                return self.wfile.write(data)
            if u.path != "/build":
                return self._json({"error": f"no endpoint {u.path}"}, 404)
            n = int(self.headers.get("Content-Length", "0"))
            spec = json.loads(self.rfile.read(n))
            sha = spec_sha(spec)
            cached = CACHE / f"{sha}.rvr"
            title = spec.get("title", "")
            with JOBS_LOCK:
                if sha in JOBS and JOBS[sha]["phase"] not in ("error",):
                    done = JOBS[sha]["phase"] == "done"
                    # a done job whose cache file was deleted must rebuild, not 500
                    if not done or Path(JOBS[sha].get("path", "")).exists():
                        return self._json({"job": sha, "cached": done})
                    del JOBS[sha]
                if cached.exists():
                    JOBS[sha] = {"sha": sha, "title": title, "phase": "done",
                                 "path": str(cached),
                                 "done_files": 0, "total_files": 0, "note": "cached",
                                 "wire_bytes": 0, "file_log": []}
                    return self._json({"job": sha, "cached": True})
                JOBS[sha] = {"sha": sha, "title": title, "phase": "starting",
                             "done_files": 0, "total_files": 0, "note": "",
                             "wire_bytes": 0, "_wire_done": 0, "disk_bytes": 0,
                             "entries_read": 0, "entries_kept": 0, "current": None,
                             "t0": time.time(), "file_log": []}
            threading.Thread(target=run_job, args=(sha, spec), daemon=True).start()
            return self._json({"job": sha, "cached": False})
        except Exception as e:
            return self._json({"error": f"{type(e).__name__}: {e}"}, 500)


def die(why: str):
    """Retire the helper and give the port back. Idempotent, and safe from any thread:
    serve_forever() must not be shut down from inside its own loop, so a fresh thread
    does it while this one returns."""
    global SRV
    srv, SRV = SRV, None
    if srv is None:
        return
    try:
        print(f"\nhelper exiting — {why}. The port is free; start a new one when you "
              f"need it.", flush=True)
    except OSError:
        pass                                   # the terminal is exactly what went away
    threading.Thread(target=srv.shutdown, daemon=True).start()


def watchdog():
    """Retire the helper the moment it stops being reachable, and again if it is merely
    forgotten. Two signals because neither covers the other: reparenting catches a
    dropped ssh session within seconds and needs no traffic to notice, while the idle
    timer catches the shell that outlives its usefulness — a tmux window, a laptop
    asleep with the tunnel up — where the parent never dies."""
    while SRV is not None:
        time.sleep(5)
        if SRV is None:
            return
        if PARENT > 1 and os.getppid() != PARENT:
            return die("the session that started it ended")
        if IDLE_MIN > 0 and time.time() - LAST_SEEN > IDLE_MIN * 60:
            return die(f"nothing asked it anything for {IDLE_MIN:g} minutes")


def whoever_has(port: int) -> dict | None:
    """Whatever is already listening on `port` — a rowver helper answers /ping."""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/ping", timeout=2) as r:
            got = json.loads(r.read())
        return got if got.get("helper") == "rowver-remote" else {}
    except Exception:
        return {}


def bind(first: int) -> ThreadingHTTPServer:
    """Bind BEFORE anything is announced, and make a taken port a sentence rather than
    a traceback. A login node is shared and the helper outlives the ssh session that
    started it, so `Address already in use` is the single likeliest first-run failure —
    Derek hit it on cmslpc345, where a helper from an earlier test still held 8271."""
    for port in range(first, first + 20):
        try:
            return ThreadingHTTPServer(("127.0.0.1", port), Handler)
        except OSError as e:
            if e.errno not in (errno.EADDRINUSE, errno.EACCES):
                raise
            mine = whoever_has(port)
            if mine and port == first:
                print(f"A rowver helper is ALREADY RUNNING on port {port} — v"
                      f"{mine.get('version')}, up {mine.get('uptime_s', 0) // 60} min, "
                      f"proxy {(mine.get('proxy') or {}).get('cn', '?')}.\n"
                      "  If that one is yours you need nothing else: go back to the page.\n"
                      "  To replace it:  pkill -f 'remote/helper.py'   (then run this again)",
                      flush=True)
                sys.exit(0)
            print(f"port {port} is busy"
                  + (" (another rowver helper — a login node is shared)" if mine else "")
                  + ", trying the next one", flush=True)
    sys.exit(f"ports {first}-{first + 19} are all busy; pick one with "
             f"ROWVER_HELPER_PORT=<port> or a first argument")


def main():
    global PORT, SRV
    SRV = srv = bind(PORT)
    PORT = srv.server_address[1]
    info = proxy_info()
    if not HAVE_PROXY:
        print(f"NOTE: no grid proxy at {PROXY} — PUBLIC storage (eospublic open data) "
              "works anonymously; for CMS data run voms-proxy-init -voms cms first",
              flush=True)
    elif info["expired"]:
        print(f"WARNING: the grid proxy at {PROXY} EXPIRED {info['expires']} — "
              "private CMS data will FAIL until you renew it "
              "(voms-proxy-init -voms cms) and RESTART this helper; "
              "public open data still works", flush=True)
    if not CERTS:
        print("NOTE: no grid CA bundle found (X509_CERT_DIR unset, no standard "
              "location present) — authenticated xrootd will fail its TLS step; "
              "public endpoints may still work", flush=True)
    print(f"rowver remote helper on http://127.0.0.1:{PORT}  "
          f"(proxy: {'EXPIRED ' + info['expires'] if info['expired'] else info['expires']}"
          f"{' at ' + PROXY if HAVE_PROXY else ''}; "
          f"CAs: {CERTS or 'none'}; eos: {EOS}; cache {CACHE})",
          flush=True)
    if PORT != 8271:
        print(f"  NOTE: not the usual 8271, so the page must still reach 8271 on your "
              f"laptop — tunnel with  ssh -L 8271:localhost:{PORT} <this-host>", flush=True)
    print("  Leave this running; Ctrl-C when you are done. It also retires itself when "
          "the session that started it ends" +
          (f", or after {IDLE_MIN:g} idle minutes (ROWVER_IDLE_MIN)." if IDLE_MIN > 0
           else " — the idle timeout is OFF (ROWVER_IDLE_MIN=0).") +
          " A helper left running holds this port and answers with YOUR credentials, "
          "and a login node is shared.", flush=True)
    # Start the forkserver NOW, while this process is quiescent. Every worker pool is
    # forked by it rather than by us, so a pool created mid-build cannot race the XRootD
    # atfork handler. Stress-verified on Mac and cmslpc: six pools created while the
    # parent was actively reading, no deadlock.
    if WORKERS != "1":
        try:
            import multiprocessing.forkserver as _fs
            import multiprocessing as _mp
            if "forkserver" in _mp.get_all_start_methods():
                _fs.ensure_running()
        except Exception as e:
            print(f"  NOTE: no forkserver ({type(e).__name__}) — reads stay serial", flush=True)
    threading.Thread(target=watchdog, daemon=True).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nhelper stopped.", flush=True)


if __name__ == "__main__":
    main()
