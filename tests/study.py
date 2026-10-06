"""A small study on disk for the tests: two local ROOT files and a recipe that reads them."""
import os
import subprocess
import sys
from pathlib import Path

import awkward as ak
import numpy as np
import uproot

REPO = Path(__file__).resolve().parent.parent

RECIPE = '''"""A test study of 320 events in two files."""
import numpy as np

from rowver import Cart
{extra}

def keep(events):
    """At least one muon."""
    return events.nMuon >= 1


def lead(events):
    """The leading muon's pT."""
    return events.Muon_pt[:, 0]


FILESET = {fileset}

CART = Cart(
    fileset=FILESET,
    keep=keep,
    columns={{"MET": "MET", "Nmuon": "nMuon", "lead_pt": lead, "Muon_pt": "Muon_pt", "q": "q"}},
    labels={{"q": {{-1: "negative", 1: "positive"}}}},
    title="Tiny study",
)
'''


def tree(path, events, seed, most=3):
    """Write one ROOT file with an Events tree of flat and jagged branches, of at most `most` muons."""
    rng = np.random.default_rng(seed)
    counts = rng.integers(0, most + 1, events).astype(np.int32)
    with uproot.recreate(path) as out:
        out["Events"] = {
            "nMuon": counts,
            "MET": rng.exponential(40, events).astype(np.float32),
            "q": rng.choice(np.array([-1, 1], dtype=np.int32), events),
            "Muon_pt": ak.unflatten(rng.exponential(30, int(counts.sum())).astype(np.float32), counts),
        }


def make(root, name="tiny", files=None, extra="", metadata=None):
    """A study folder under root; files maps a sample to its paths (default: two files made here),
    metadata a sample to its metadata (default: none)."""
    study = Path(root) / name
    study.mkdir(parents=True)
    if files is None:
        tree(study / "a.root", 200, 1)
        tree(study / "b.root", 120, 2)
        files = {"alpha": [str(study / "a.root")], "beta": [str(study / "b.root")]}
    fileset = {sample: {"files": {path: "Events" for path in paths}, "metadata": (metadata or {}).get(sample, {})}
               for sample, paths in files.items()}
    (study / "cart.py").write_text(RECIPE.format(fileset=repr(fileset), extra=extra), encoding="utf-8")
    return study


def point(folder, files, metadata=None):
    """Point a study's recipe at other files: {sample: [path, ...]}, with {sample: metadata}."""
    fileset = {sample: {"files": {str(path): "Events" for path in paths}, "metadata": (metadata or {}).get(sample, {})}
               for sample, paths in files.items()}
    text = (Path(folder) / "cart.py").read_text(encoding="utf-8")
    start = text.index("FILESET = ")
    end = text.index("\n", start)
    (Path(folder) / "cart.py").write_text(f"{text[:start]}FILESET = {fileset!r}{text[end:]}", encoding="utf-8")


def environment(**extra):
    """The environment every spawned presser runs in: this checkout first on the path."""
    env = {key: value for key, value in os.environ.items() if key != "PYTHONPATH"}
    env.update(PYTHONPATH=str(REPO), COLUMNS="100", **extra)
    return env


def rowver(*args, timeout=180, **extra):
    """Run `python -m rowver ...` from this checkout; the completed process."""
    return subprocess.run([sys.executable, "-m", "rowver", *map(str, args)], env=environment(**extra),
                          capture_output=True, text=True, timeout=timeout)


NUMPY = {"bool": np.bool_, "float": np.float32, "double": np.float64, "int8_t": np.int8, "uint8_t": np.uint8,
         "int16_t": np.int16, "uint16_t": np.uint16, "int32_t": np.int32, "uint32_t": np.uint32,
         "int64_t": np.int64, "uint64_t": np.uint64}


def values(rng, name, typename, size):
    """Believable values for one NanoAOD-style branch, so a recipe's selection keeps some events."""
    dtype = NUMPY[typename.removesuffix("[]")]
    field = name.rsplit("_", 1)[-1]
    if dtype is np.bool_:
        return rng.random(size) < 0.8
    if field == "charge":
        return rng.choice(np.array([-1, 1], dtype=dtype), size)
    if field == "eta":
        return rng.normal(0, 1.4, size).astype(dtype)
    if field == "phi":
        return rng.uniform(-np.pi, np.pi, size).astype(dtype)
    if "Iso" in name:
        return rng.exponential(0.05, size).astype(dtype)
    if "btag" in name:
        return rng.random(size).astype(dtype)
    if name == "genWeight":
        return rng.choice(np.array([-1, 1]), size).astype(dtype) * 25000
    if np.issubdtype(dtype, np.integer):
        return rng.integers(0, 5, size).astype(dtype)
    return (15 + rng.exponential(30, size)).astype(dtype)


def nano(path, branches, events=1000, seed=0):
    """A TTree like a NanoAOD file's, holding only the branches named ({name: typename}): each
    collection's jagged branches share one counter, which uproot writes as n<Collection>."""
    rng = np.random.default_rng(seed)
    collections = sorted({name.split("_", 1)[0] for name, typename in branches.items() if typename.endswith("[]")})
    data = {}
    for collection in collections:
        counts = rng.integers(0, 5, events)
        data[collection] = ak.zip({name.split("_", 1)[1]: ak.unflatten(values(rng, name, typename, int(counts.sum())),
                                                                        counts)
                                   for name, typename in branches.items()
                                   if typename.endswith("[]") and name.split("_", 1)[0] == collection})
    for name, typename in branches.items():
        if not typename.endswith("[]") and not (name[:1] == "n" and name[1:] in collections):
            data[name] = values(rng, name, typename, events)
    with uproot.recreate(path) as out:
        out.mktree("Events", {name: array.type.content if isinstance(array, ak.Array) else array.dtype
                              for name, array in data.items()})
        out["Events"].extend(data)
