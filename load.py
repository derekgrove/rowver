"""Load a pikl of columns and show what is in it."""
import gzip
import pickle
from pathlib import Path

import numpy as np

STUDIES: Path = Path(__file__).with_name("studies")


def available() -> list[str]:
    """Every pikl on disk as study/name, since a study can hold more than one."""
    return sorted(f"{p.parent.name}/{p.stem}" for p in STUDIES.glob("*/*.pkl"))


def load(name: str, dest: str | Path | None = None) -> dict[str, dict[str, np.ndarray]]:
    """Read <dest>/<name>.pkl into {sample: {column name: ndarray}}."""
    path = (Path(dest) if dest else STUDIES / name) / f"{name}.pkl"
    assert path.exists(), f"no pikl at {path} — have: {available()}"
    with gzip.open(path, "rb") as f:
        return pickle.load(f)["results"]


def n_events(columns: dict[str, np.ndarray]) -> int:
    """How many events one sample holds; raises if its columns disagree in length."""
    n = {len(v) for v in columns.values()}
    assert len(n) == 1, f"ragged columns: {sorted(n)}"
    return n.pop()


if __name__ == "__main__":
    import sys
    name = sys.argv[1] if len(sys.argv) > 1 else (available() or ["<none>"])[0]
    data = load(name.split("/")[-1], STUDIES / name.split("/")[0]) if "/" in name else load(name)
    assert len({tuple(c) for c in data.values()}) == 1, "datasets disagree on keys"

    print(f"{name}.pkl  —  {len(data)} samples, {sum(map(n_events, data.values())):,} events\n")
    for k, v in next(iter(data.values())).items():
        print(f"  {k:22s} {v.dtype}")
    print()
    for ds, columns in data.items():
        print(f"  {ds:24s} {n_events(columns):>9,}")
