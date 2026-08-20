"""Write a DATA-LESS .rvr: the study's identity without its columns.

The file is a normal rowver-4 container whose buffer is empty (n=0, cols=[]) and whose
META carries ONE optional key, `remote` — everything the local helper needs to build
the real file: fileset, branch map, optional cut, labels/weights, and the tabs (as
page.state kwargs, applied AFTER the pack when column metadata exists). Old files are
untouched by construction: no reader ever required the key, and the page only enters
the remote path when it sees it.

    from make_dataless import write
    write("my_study.rvr", spec)

`spec["start"]` should hold the exact command that starts the helper — the page prints
it verbatim when no helper answers, so the failure mode tells Derek what to run.
"""
import gzip
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import page  # noqa: E402

START = "cd rowver && python3 remote/helper.py"   # from a clone of the repo


def write(path: str | Path, spec: dict) -> Path:
    """The data-less file: full standard META (so every existing check passes), no
    bytes, plus the remote spec."""
    spec = dict(spec)
    spec.setdefault("start", START)
    for key in ("fileset", "branches"):
        assert key in spec, f"a remote spec needs {key!r}"
    meta = {
        "rowver": page.ROWVER,
        "n": 0,
        "bytes": 0,
        "datasets": list(spec["fileset"]),
        "ds_offset": 0,
        "weights": False,
        "cols": [],
        "made_by": page.VERSION,
        "title": Path(path).name,
        "remote": spec,
    }
    header = json.dumps(meta).encode()
    out = Path(path)
    out.write_bytes(gzip.compress(
        len(header).to_bytes(4, "little") + header, 6, mtime=0))
    return out
