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


def tree(path, events, seed):
    """Write one ROOT file with an Events tree of flat and jagged branches."""
    rng = np.random.default_rng(seed)
    counts = rng.integers(0, 4, events).astype(np.int32)
    with uproot.recreate(path) as out:
        out["Events"] = {
            "nMuon": counts,
            "MET": rng.exponential(40, events).astype(np.float32),
            "q": rng.choice(np.array([-1, 1], dtype=np.int32), events),
            "Muon_pt": ak.unflatten(rng.exponential(30, int(counts.sum())).astype(np.float32), counts),
        }


def make(root, name="tiny", files=None, extra=""):
    """A study folder under root; files maps a sample to its paths (default: two files made here)."""
    study = Path(root) / name
    study.mkdir(parents=True)
    if files is None:
        tree(study / "a.root", 200, 1)
        tree(study / "b.root", 120, 2)
        files = {"alpha": [str(study / "a.root")], "beta": [str(study / "b.root")]}
    fileset = {sample: {"files": {path: "Events" for path in paths}, "metadata": {}}
               for sample, paths in files.items()}
    (study / "cart.py").write_text(RECIPE.format(fileset=repr(fileset), extra=extra), encoding="utf-8")
    return study


def point(folder, files):
    """Point a study's recipe at other files: {sample: [path, ...]}."""
    fileset = {sample: {"files": {str(path): "Events" for path in paths}, "metadata": {}}
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
