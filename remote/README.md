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

Leave the terminal open — the helper lives in it, and closing the ssh session shuts
everything down cleanly. Measured from a cmslpc node: the public quickstart in 6 s; a
3.15-million-event private-ntuple study in 9.4 s — and on cmslpc your kerberos ticket
alone reads the FNAL eos areas, no `voms-proxy-init` needed. Measured from an lxplus
node: the same quickstart in ~2 s (eospublic is CERN-local). Two lxplus notes:
logging in from outside CERN asks for a second factor (OTP) after your Kerberos
step — that is CERN 2FA, not an error; and a CERN Kerberos ticket alone reads
`/eos/user` areas (measured: even a laptop with a ticket opens
`root://eosuser.cern.ch` files, no proxy involved).

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
drop it on the page — or just click **"build the open-data quickstart"** on the page's
drop screen — and ~5–10 s later you are looking at 52,911 real dimuon-skim events.
This is the smoke test: if it works, your whole pipeline works, and private data is
only a proxy (or option 2) away.

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

One-time setup:

1. Put your CERN grid certificate in `~/.globus` (`usercert.pem`, `userkey.pem`).
2. Install the voms tools per https://hackmd.io/@andrzejnovak/SJxscCZvD (the
   conda-forge route; it also provides a grid CA bundle inside the env).

Each session (or weekly):

    voms-proxy-init -voms cms --valid 192:00

This writes `/tmp/x509up_u$(id -u)` — the first place the helper looks, so usually
no environment variable is needed.

Run the builder:

    git clone https://gitlab.cern.ch/dgrove/rowver
    python3 -m pip install --user numpy uproot fsspec-xrootd
    cd rowver && python3 remote/helper.py

The helper's first lines announce which proxy and CA bundle it found — that is the
"did it work" check. If it says anonymous:
`export X509_USER_PROXY=<the path voms-proxy-init printed>`; if CAs are missing:
`export X509_CERT_DIR=<your env>/etc/grid-security/certificates`; restart the helper.

Then open https://rowver.web.cern.ch → the builder → eos mode →
`/store/user/your_eos_dir` → browse, tick files, build.

Notes: allow Firefox's local-network permission prompt on first use; the first file
open pays a one-time ~1 minute xrootd handshake (labeled on the page); after
renewing an expired proxy, restart the helper.
