"""The Python examples in docs/examples/python, each run to its end on a file pressed here with the
guide's own recipe (docs/examples/zmumu) from toy Z → μμ ROOT files: three samples, a list column,
a derived weight. Each example is skipped when a package it uses is not installed."""

import importlib.util
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import numpy as np

import rowver
from rowver._rowver import rvr

EXAMPLES = pathlib.Path(__file__).resolve().parents[3] / "docs" / "examples"

SAMPLES = """format = "rvr-samples"
version = 1

[places]

[defaults]
tree = "Events"
metadata = { kind = "simulation", lumi = 11.6 }

[eras]

[[sample]]
name = "data"
label = "Data"
files = "data.root"
metadata = { kind = "data" }

[[sample]]
name = "DY"
label = "Drell-Yan"
files = "DY.root"
metadata = { xsec = 3503.7 }

[[sample]]
name = "TT"
label = "tt̄"
files = "TT.root"
metadata = { xsec = 225.2 }
"""


def has(*modules):
    """Whether these optional packages are installed, without importing them: ROOT takes seconds,
    and a broken install should fail its example, not skip it."""
    return all(importlib.util.find_spec(module) is not None for module in modules)


def toy(path, events, seed):
    """A ROOT file of `events` toy events, NanoAOD's muon branches: two to four muons each, the
    first two of opposite charge, each muon's pT, eta and phi at random."""
    import awkward as ak
    import uproot

    rng = np.random.default_rng(seed)
    count = rng.integers(2, 5, events)
    n = count.sum()
    charge = rng.choice([-1, 1], n).astype(np.int32)
    first = np.cumsum(count) - count
    charge[first + 1] = -charge[first]
    muons = ak.unflatten(ak.zip({
        "pt": rng.exponential(25, n).astype(np.float32) + 15,
        "eta": rng.uniform(-2.3, 2.3, n).astype(np.float32),
        "phi": rng.uniform(-np.pi, np.pi, n).astype(np.float32),
        "mass": np.full(n, 0.1057, np.float32),
        "charge": charge,
    }), count)
    with uproot.recreate(path) as out:
        out.mktree("Events", {"Muon": muons.type.content})  # a TTree: Muon_pt, …, and nMuon
        out["Events"].extend({"Muon": muons})


@unittest.skipUnless(EXAMPLES.is_dir() and has("awkward", "uproot"), "needs docs/examples, awkward and uproot")
class Examples(unittest.TestCase):
    """The guide's study pressed once from toy files; each example run on it in a folder of its own."""

    @classmethod
    def setUpClass(cls):
        cls.dir = pathlib.Path(tempfile.mkdtemp(prefix="rowver-examples-")) / "zmumu"
        cls.addClassCleanup(shutil.rmtree, cls.dir.parent)
        cls.dir.mkdir()
        config = mock.patch.dict(os.environ, {"RVR_CONFIG": str(cls.dir.parent / "config")})
        config.start()
        cls.addClassCleanup(config.stop)
        for seed, name in enumerate(["data", "DY", "TT"]):
            toy(cls.dir / f"{name}.root", 3000, seed)
        (cls.dir / "samples.toml").write_text(SAMPLES)
        shutil.copy(EXAMPLES / "zmumu" / "recipe.toml", cls.dir)
        for args in (["samples", "resolve", str(cls.dir / "samples.toml")], ["press", str(cls.dir)]):
            if rvr(args) != 0:
                raise RuntimeError(f"rvr {' '.join(args)} failed: its words are above")
        cls.path = cls.dir / "zmumu.rvr"

    def run_example(self, script):
        """Run docs/examples/python/SCRIPT on the file, in a folder of its own, plotting to files."""
        out = self.dir.parent / script.removesuffix(".py")
        out.mkdir()
        env = {**os.environ, "MPLBACKEND": "Agg", "MPLCONFIGDIR": str(self.dir.parent / "matplotlib"), "ROOT_RDF_SNAPSHOT_INFO": "0"}
        done = subprocess.run([sys.executable, str(EXAMPLES / "python" / script), str(self.path)], cwd=out, env=env,
                              capture_output=True, text=True, timeout=600)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        return out

    # SPEC 11.3
    def test_each_sample_s_rows_are_the_slice_rows_of_gives(self):
        with rowver.open(self.path) as f:
            sample = f["sample"]
            for at, name in enumerate(f.samples):
                rows = f.rows_of(name)
                self.assertTrue((sample[rows] == at).all(), name)
                self.assertEqual(rows.stop - rows.start, (sample == at).sum(), name)

    # SPEC 11.3, 11.4, 11.5, 11.6, 11.8
    @unittest.skipUnless(has("pandas", "pyarrow"), "needs pandas and pyarrow")
    def test_columns(self):
        out = self.run_example("columns.py")
        self.assertTrue((out / "zmumu.parquet").is_file())

    # SPEC 11.3, 11.4
    @unittest.skipUnless(has("matplotlib"), "needs matplotlib")
    def test_matplotlib_hists(self):
        out = self.run_example("matplotlib_hists.py")
        for png in ["mpl_stack.png", "mpl_shapes.png", "mpl_mass_pt.png"]:
            self.assertTrue((out / png).is_file(), png)

    # SPEC 11.3, 11.4
    @unittest.skipUnless(has("hist", "mplhep", "matplotlib"), "needs hist, mplhep and matplotlib")
    def test_hist_hists(self):
        import uproot

        out = self.run_example("hist_hists.py")
        with uproot.open(out / "hists.root") as written:
            self.assertEqual(written["mass_DY"].classname, "TH1D")
            self.assertEqual(written["mass_pt_DY"].classname, "TH2D")

    # SPEC 11.3, 11.4
    @unittest.skipUnless(has("ROOT"), "needs ROOT")
    def test_root_hists(self):
        out = self.run_example("root_hists.py")
        for written in ["root_stack.png", "root_mass_pt.png", "root_hists.root", "zmumu_tree.root"]:
            self.assertTrue((out / written).is_file(), written)


if __name__ == "__main__":
    unittest.main()
