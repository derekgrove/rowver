"""Generate the deterministic local fixture for the parallel-press study."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import uproot


STUDY = '''from rowver import Cart


def PREPARE(events):
    return events[events.valid]


def KEEP(events):
    return (events.x >= 0) & (events.x % 3 == 0)


def SCALED(events, metadata):
    return events.y * metadata["scale"]


def HALF_X(events):
    return events.x // 2


def WEIGHT(events, metadata):
    return events.genWeight * metadata["norm"]


CART = Cart(
    fileset={
        "data": {
            "files": {"data-large.root": "Events", "empty.root": "Events",
                      "zero-kept.root": "Events"},
            "metadata": {"weights": None, "scale": 1.0},
        },
        "signal": {
            "files": {"signal-small.root": "Events", "signal-large.root": "Events"},
            "metadata": {"weights": WEIGHT, "norm": 0.25, "scale": 2.0},
        },
        "background": {
            "files": {"background.root": "Events", "background-tail.root": "Events"},
            "metadata": {"weights": "genWeight", "scale": 0.5},
        },
    },
    prepare=PREPARE,
    keep=KEEP,
    columns={"scaled": SCALED, "x": "x", "half_x": HALF_X},
    title="Parallel press local benchmark",
)
'''


def _arrays(rows: int, *, negative: bool = False) -> dict[str, np.ndarray]:
    x = np.arange(rows, dtype=np.int32)
    if negative:
        x = -x - 1
    return {
        "x": x,
        "y": x.astype(np.float64) / 7.0,
        "genWeight": np.where(np.arange(rows) % 2, -0.5, 1.5),
        "valid": np.arange(rows) % 11 != 0,
    }


def _data_hash(arrays: dict[str, np.ndarray]) -> str:
    digest = hashlib.sha256()
    for name, values in arrays.items():
        digest.update(name.encode())
        digest.update(values.dtype.str.encode())
        digest.update(values.tobytes())
    return digest.hexdigest()


def _file_hash(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def generate(directory: Path, base_rows: int = 500_000) -> dict:
    if base_rows < 1:
        raise ValueError("base_rows must be positive")
    directory.mkdir(parents=True, exist_ok=True)
    sizes = {
        "data-large.root": 2 * base_rows,
        "empty.root": 0,
        "zero-kept.root": base_rows,
        "signal-small.root": max(1, base_rows // 2),
        "signal-large.root": 3 * base_rows,
        "background.root": base_rows,
        "background-tail.root": max(1, base_rows // 4),
    }
    files = []
    for name, rows in sizes.items():
        path = directory / name
        arrays = _arrays(rows, negative=name == "zero-kept.root")
        with uproot.recreate(path) as root:
            root["Events"] = arrays
        files.append({
            "name": name,
            "rows": rows,
            "bytes": path.stat().st_size,
            "data_sha256": _data_hash(arrays),
            "file_sha256": _file_hash(path),
        })
    (directory / "cart.py").write_text(STUDY, encoding="utf-8")
    manifest = {"schema": 1, "base_rows": base_rows, "files": files}
    (directory / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--base-rows", type=int, default=500_000)
    args = parser.parse_args(argv)
    manifest = generate(args.directory, args.base_rows)
    print(f"wrote {args.directory} — {sum(f['rows'] for f in manifest['files']):,} rows")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
