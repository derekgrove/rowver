"""Build the ninth frozen golden: a DATA-LESS .rvr (the 0.28.0 remote key).

Run ONCE at 0.28.0, committed, then never re-run — the same rule as every golden.
What it pins that the eight before it cannot: a rowver-4 container with an EMPTY
buffer and the optional `remote` key. A future reader must (a) still open every OLD
file exactly as before (the eight other goldens), and (b) recognise THIS file as
remote — the page's pure remoteSpecCheck must read the same summary out of it, and
must still REFUSE a spec with no branches. The spec below uses only the public demo
vocabulary (CMS open data names), holds one verbatim-files entry AND one named-dataset
entry (both shapes the check accepts), and never resolves — no helper runs in the
battery; the pin is the FORMAT, not the network.
"""
import hashlib
import sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE.parent / "remote"))
import make_dataless  # noqa: E402

OUT = HERE / "golden_0.28.0_dataless.rvr"

SPEC = {
    "title": "golden_dataless",
    "fileset": {
        "MC · ZZ→4mu": {"files": {
            "root://eospublic.cern.ch//eos/opendata/example_zz4mu.root": "Events"}},
        "MC · ZZ→4e": {"dataset": "/ZZTo4e/example-campaign/NANOAODSIM",
                       "tree": "Events", "limit": 2},
    },
    "branches": {"M4L": "M4L", "MZ1": "MZ1", "MZ2": "MZ2"},
    "cut": "M4L >= 70",
    "weights": None,
    "port": 8271,
    "start": "python remote/helper.py",
    "tabs": [{"name": "zz", "view": {"lumi": 1}}],
}


def main():
    out = make_dataless.write(OUT, SPEC)
    sha = hashlib.sha256(out.read_bytes()).hexdigest()
    print(f"{out.name}  {out.stat().st_size} B")
    print(f"sha256: {sha}")
    print("pin summary: datasets=2 files=1 columns=3 cut='M4L >= 70' port=8271")


if __name__ == "__main__":
    main()
