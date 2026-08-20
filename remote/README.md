# Remote studies: build a `.rvr` straight from storage

The RowVer page at https://rowver.web.cern.ch can build a study for you: drop a
**data-less `.rvr`** (a ~0.5 kB file carrying only the study's *specification* — files,
branches, cut) and the page acquires the columns itself, reading only the branches it
needs (measured cost: 0.006–0.7 % of the files' bytes).

A browser cannot reach CMS storage on its own — grid TLS trust, certificate
authentication and missing CORS wall it off, even for public data (all four walls
measured; see `STUDY.md`). So the page talks to a tiny **helper**: `remote/helper.py`
from this repo, a one-command local daemon that speaks xrootd on the page's behalf and
serves it on `127.0.0.1:8271`. Everything in this directory works from a plain clone:

    git clone https://gitlab.cern.ch/dgrove/rowver

## Option 2 — run the helper on a cluster node (recommended)

Your cluster account has the credentials, the grid CA bundle, and the data close by;
your laptop needs nothing but ssh and a browser. ONE terminal, top to bottom —
substitute your own username and host (examples: `username@cmslpc-el9.fnal.gov`,
`username@lxplus.cern.ch`):

    # 1 — from your laptop: log in, forwarding the page's port to the node
    ssh -L 8271:localhost:8271 username@cmslpc-el9.fnal.gov

    # 2 — now ON THE NODE: get the code and its two python packages
    #     (the pip line is harmless if they are already there)
    git clone https://gitlab.cern.ch/dgrove/rowver
    python3 -m pip install --user numpy uproot fsspec-xrootd
    cd rowver && python3 remote/helper.py

    # 3 — back on your laptop: open https://rowver.web.cern.ch in a browser and
    #     drop your data-less .rvr — or click "build the open-data quickstart"

Leave the terminal open — the helper lives in it. **Stop it with Ctrl-C** when you are
done. It also retires itself, because a login node is shared and a forgotten helper is
worse than an absent one (see *What it is allowed to do* below): it exits the moment
its terminal proves gone, and after **30 idle minutes** with nothing asking it
anything. An open browser tab pings it every ~1.5 s, so simply having the page open
keeps it alive; `ROWVER_IDLE_MIN=0` disables the idle half, and `ROWVER_IDLE_MIN=120`
lengthens it.

Measured from a cmslpc node: the public quickstart in 6 s; a 3.15-million-event
private-ntuple study in 9.4 s — and on cmslpc your kerberos ticket alone reads the
FNAL eos areas, no `voms-proxy-init` needed. Measured from an lxplus node: the same
quickstart in ~2 s (eospublic is CERN-local). Two lxplus notes: logging in from
outside CERN asks for a second factor (OTP) after your Kerberos step — that is CERN
2FA, not an error; and a CERN Kerberos ticket alone reads `/eos/user` areas
(measured: even a laptop with a ticket opens `root://eosuser.cern.ch` files, no proxy
involved). The helper points itself at the right eos for the host it is on —
`root://eosuser.cern.ch/` on a `.cern.ch` machine, `root://cmseos.fnal.gov/`
otherwise — and prints which one on startup; `ROWVER_EOS=root://…/` overrides it.

### "Address already in use"

A login node is shared, and a helper outlives the ssh session that started it, so
sooner or later step 2 lands on a port that is taken. The helper says so in a
sentence and does the sensible thing:

    $ python3 remote/helper.py
    A rowver helper is ALREADY RUNNING on port 8271 — v0.33.0, up 41 min, proxy dgrove.
      If that one is yours you need nothing else: go back to the page.
      To replace it:  pkill -f 'remote/helper.py'   (then run this again)

If the port is held by something that is *not* a rowver helper it moves to the next
free one and prints the tunnel line to match, e.g. `ssh -L 8271:localhost:8272 <host>`
— your laptop still uses 8271, so nothing else changes. (Opening `console.html?port=N`
points the builder at a different local port if you ever need that.)

## What it is allowed to do — read this before running it on a shared node

The helper acts **with your credentials**. Whatever you can read, it will read for
whoever asks it. It listens on `127.0.0.1`, which keeps it off the network — but
`127.0.0.1` is *per machine, not per user*, so on a login node shared with a hundred
other people, any one of them can talk to it. And the `KEEP` cut is evaluated as a
Python expression: it is restricted, but restricted `eval` is not a security boundary
(confirmed 2026-08-20 — numpy's own module globals hand back the interpreter's real
builtins), so treat "can reach the helper" as "can run code as me".

Until that is closed properly, the practical rules are:

- **Prefer your own laptop** (Option 1) when you can. A single-user machine has no
  exposure: anything running as you can already read `/tmp/x509up_u$UID` directly, so
  the helper grants nothing new.
- **On a shared node, run it while you are using it and let it retire.** That is why
  the timeouts above exist — they shrink the window from "forever" to "while you work".
- **Check who else is on the node** if you care: `who`. And `pkill -f
  'remote/helper.py'` before you log out.

The durable fix, measured to work on cmslpc (OpenSSH 9.9; the feature needs 6.7+):
serve on a **Unix socket** in a 0700 directory instead of a TCP port, and forward to
*that* — `ssh -L 8271:/tmp/rowver-$UID/helper.sock <host>`. Your laptop still talks to
plain `http://127.0.0.1:8271`, nothing in the browser changes, and file permissions
(kernel-enforced) replace the honour system. Not implemented yet.

## Option 1 — run the helper locally

    python3 -m pip install --user numpy uproot fsspec-xrootd
    cd rowver && python3 remote/helper.py

- **Public open data needs no credentials at all** — eospublic answers anonymously.
- Private CMS data needs a valid grid proxy (`voms-proxy-init -voms cms`); the helper
  finds it at `$X509_USER_PROXY` or `/tmp/x509up_u$UID`, and finds the CA bundle in
  the standard grid/cvmfs locations (`$X509_CERT_DIR` overrides). It says at startup
  exactly what it found. After renewing an expired proxy, restart the helper.

## No proxy? No problem — the quickstart

`remote/quickstart.rvr` is a data-less spec against **CMS open data** (one public
2016 NanoAOD file on eospublic, two flat branches, one cut). With any helper running,
drop it on the page and ~5–10 s later you are looking at 52,911 real dimuon-skim
events. This is the smoke test: if it works, your whole pipeline works, and private
data is only a proxy (or option 2) away.

Starting from nothing instead, the page's drop screen has **"start the guided
open-data build"** — that opens the builder pre-filled with the same public example
and walks you through it; press **build .rvr** there, then drop what it hands you back
on the RowVer page. (Measured 2026-08-20: 52,911 events, 7.4 s cold and 0.5 s from
cache, through a helper on a cmslpc node with no grid proxy at all.)

## The console

`remote/console.html` (open it in your browser, helper running) is the study builder:
search CMS DAS by dataset name, browse eos, inspect trees and branches, tick a subset
of files, and watch the exact JSON spec grow as you click — then build the `.rvr`, or
download the data-less spec to share. Every number it shows is a live measurement
from the helper.

## Firefox note

Firefox may ask permission for `rowver.web.cern.ch` to access your **local network**
(the page talks only to the helper on `127.0.0.1`). Allow it, then retry the drop.

## Format and provenance

The spec rides in the `.rvr`'s META under the optional `remote` key
(`make_dataless.py` writes them; the helper docstring documents the shape). Every
freshly built file records its provenance under META `origin`: the spec as submitted
plus the exact resolved `{url: tree}` list it was read from. Identical specs build
byte-identical files.

## Local proxy, step by step (Option 1 with private data)

**One-time setup — getting a working proxy on your own laptop.** Follow this
walkthrough, which is the one that actually works end to end (the conda-forge route;
it also drops a grid CA bundle inside the env, which you need and which nothing else
installs for you):

> **https://hackmd.io/@andrzejnovak/SJxscCZvD**

You will also need your CERN grid certificate in `~/.globus` (`usercert.pem`,
`userkey.pem`), which that page covers.

**After installing and setting up your proxy**, each session (or weekly):

    voms-proxy-init -voms cms --valid 192:00

This writes `/tmp/x509up_u$(id -u)` — the first place the helper looks, so usually no
environment variable is needed.

### The scary red error at the end of `voms-proxy-init` is nothing

`voms-proxy-init` almost always ends with a wall of certificate complaints. It looks
like the proxy failed. **It did not.** The real output looks like this:

    $ voms-proxy-init -voms cms --valid 192:00
    Enter GRID pass phrase:
    Your identity: /DC=ch/DC=cern/OU=Organic Units/OU=Users/CN=<you>/CN=<id>/CN=<Your Name>
    Cannot open fileFilename=/Users/<you>/.rnd
    Function:
    Creating temporary proxy ................................................ Done
    Contacting  voms-cms-auth.cern.ch:443 [...] "cms" Done
    Creating proxy .......................................................... Done

    Your proxy is valid until Thu Aug 27 16:45:50 2026

    Error: Certificate verification failed.
    outdated CRL found, revoking all certs till you get new CRL
    Function:
    certificate validation error: CRL has expired [subject=/C=ch/O=CERN/CN=CERN Root
    Certification Authority 2,issuer=/C=ch/O=CERN/CN=CERN Root Certification Authority 2]
    Function:

Three things in there are noise, in increasing order of alarming:

- `Cannot open fileFilename=~/.rnd` — openssl looking for a random-seed file that has
  not been needed for years.
- `outdated CRL found` / `CRL has expired` — the certificate *revocation lists* that
  ship inside the conda grid CA bundle are stale (measured here: 154 CRLs in the
  bundle, most of them expired months ago). Real sites refresh them with `fetch-crl`
  on a cron job; a laptop env never does.
- `Error: Certificate verification failed.` — this is `voms-proxy-init` reporting that
  its own *post-creation* check could not validate against those stale lists.

**The line that matters is `Your proxy is valid until …`.** If you see it, the proxy
exists and works. Verified here on 2026-08-20 with exactly the output above: the
helper read a private CMS ntuple on FNAL eos over xrootd with that proxy and built a
real study from it (285 branches inventoried, 8,391 events, ~1 s). Ignore the red and
carry on. You can confirm any time with:

    openssl x509 -in /tmp/x509up_u$(id -u) -noout -enddate

### Run the builder

    git clone https://gitlab.cern.ch/dgrove/rowver
    python3 -m pip install --user numpy uproot fsspec-xrootd
    cd rowver && python3 remote/helper.py

The helper's first lines announce which proxy, CA bundle and eos endpoint it found —
that is the "did it work" check, and it names your certificate's CN if the proxy is
readable. If it says anonymous:
`export X509_USER_PROXY=<the path voms-proxy-init printed>`; if CAs are missing:
`export X509_CERT_DIR=<your env>/etc/grid-security/certificates`; restart the helper.

Then open https://rowver.web.cern.ch → the builder → eos mode →
`/store/user/your_eos_dir` → browse, tick files, build.

Notes: allow Firefox's local-network permission prompt on first use; the first file
open can pay a one-time xrootd handshake (labeled on the page); after renewing an
expired proxy, restart the helper.
