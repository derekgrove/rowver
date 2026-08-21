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
PROXY = os.environ.get("X509_USER_PROXY", f"/tmp/x509up_u{os.getuid()}")

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

CERTS = find_certs()
HAVE_PROXY = Path(PROXY).exists()
if HAVE_PROXY:
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
    """Every TTree in one file, so the console can offer the tree choice."""
    import uproot
    t0 = time.time()
    f = uproot.open(url, timeout=180)
    note_first_open(time.time() - t0)
    out = []
    for k, cls in f.classnames().items():
        if cls == "TTree":
            name = k.split(";")[0]
            try:
                out.append({"name": name, "entries": int(f[name].num_entries)})
            except Exception:
                out.append({"name": name, "entries": None})
    seen, trees = set(), []
    for t in out:
        if t["name"] not in seen:
            seen.add(t["name"])
            trees.append(t)
    return {"url": url, "trees": trees}


def branch_inventory(url: str, tree: str) -> dict:
    import uproot
    t0 = time.time()
    t = uproot.open(url, timeout=180)[tree]
    note_first_open(time.time() - t0)
    return {"entries": int(t.num_entries),
            "branches": [{"name": b.name, "type": str(b.interpretation)}
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


def load_dataset(name: str, entry: dict, branches: dict, cut: str | None, job: dict):
    """All requested branches of one dataset, cut applied, as {column: array}.
    Reads in CHUNK-entry slices and updates the job's LIVE counters after each — every
    number the console shows is sampled from uproot's own byte meter, never invented."""
    import uproot
    want = sorted(set(branches.values()))
    parts: dict = {c: [] for c in branches}
    for url, tree in entry["files"].items():
        base = url.rstrip("/").split("/")[-1]
        cold = not HANDSHAKE["done"]
        job["phase"] = "opening"
        job["current"] = {"dataset": name, "file": base, "entries_total": None,
                          "entries_read": 0, "wire_MB": 0.0, "file_MB": None}
        job["note"] = f"opening {base}" + (
            " — FIRST open in this helper process: the one-time ~60 s xrootd handshake"
            if cold else "")
        t0 = time.time()
        f = uproot.open(url, timeout=300)
        note_first_open(time.time() - t0)
        t = f[tree]
        missing = [b for b in want if b not in t]
        if missing:
            raise RuntimeError(f"{name}: {url} has no branch(es) {missing}")
        n_tot = int(t.num_entries)
        job["disk_bytes"] += f.file.source.num_bytes
        job["current"]["entries_total"] = n_tot
        job["current"]["file_MB"] = round(f.file.source.num_bytes / 1e6, 2)
        job["phase"] = "reading"
        job["note"] = f"{name}: {base}"
        read = 0
        checked = False
        for arrs in t.iterate(want, step_size=CHUNK, library="np"):
            if not checked:
                for b in want:
                    if arrs[b].dtype == object or arrs[b].ndim != 1:
                        raise RuntimeError(
                            f"{name}: branch {b} is not one number per event — the "
                            f".rvr format is flat; reduce it in a skim first")
                checked = True
            got = len(arrs[want[0]])
            read += got
            job["entries_read"] += got
            cols = {c: arrs[b] for c, b in branches.items()}
            if cut:
                keep = eval_cut(cut, cols)
                cols = {c: v[keep] for c, v in cols.items()}
            job["entries_kept"] += len(next(iter(cols.values())))
            for c, v in cols.items():
                parts[c].append(v)
            job["current"]["entries_read"] = read
            job["current"]["wire_MB"] = round(f.file.source.num_requested_bytes / 1e6, 3)
            job["wire_bytes"] = job["_wire_done"] + f.file.source.num_requested_bytes
        if read == 0:
            for c in branches:                       # an empty tree still needs arrays
                parts[c].append(np.array([], np.float64))
        job["_wire_done"] += f.file.source.num_requested_bytes
        job["wire_bytes"] = job["_wire_done"]
        job["file_log"].append({
            "dataset": name, "url": url, "entries": n_tot,
            "file_MB": round(f.file.source.num_bytes / 1e6, 2),
            "wire_MB": round(f.file.source.num_requested_bytes / 1e6, 3),
            "seconds": round(time.time() - t0, 2)})
        job["done_files"] += 1
        # this file is now COUNTED, so it must stop counting as work in progress too —
        # leaving it here made the bar read (done + 1)/total, i.e. 114% on a 7-file build
        job["current"] = None
        job["note"] = f"{name}: {job['done_files']}/{job['total_files']} files"
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
            f = uproot.open(url, timeout=300)
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
          f"(proxy: {'EXPIRED ' + info['expires'] if info['expired'] else info['expires']}; "
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
    threading.Thread(target=watchdog, daemon=True).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nhelper stopped.", flush=True)


if __name__ == "__main__":
    main()
