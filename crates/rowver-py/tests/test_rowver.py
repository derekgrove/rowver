"""The Python package on a file pressed here, from the ROOT fixtures rvr's own tests use: the values
read back, their types, and the Arrow, pandas and Parquet forms. Run from this crate's folder,
after `maturin develop`: `python -m unittest discover tests`."""

import pathlib
import shutil
import tempfile
import unittest

import numpy as np

import rowver
from rowver._rowver import rvr

FIXTURES = pathlib.Path(__file__).resolve().parents[2] / "rvr" / "tests" / "fixtures"

SAMPLES = """format = "rvr-samples"
version = 1

[[sample]]
name = "flat"
label = "Flat ntuple"
files = "uproot_flat.root"
tree = "analysis"
xsec = 1.5
"""

RECIPE = """format = "rvr-recipe"
version = 1

[keep]
jets = "nJet >= 1"

[copy]
branches = ["weight", "nJet", "Jet_pt", "isGood"]

[column]
ht = "sum(Jet_pt)"
"""


def has(module):
    """Whether an optional package is installed."""
    try:
        __import__(module)
        return True
    except ImportError:
        return False


class Pressed(unittest.TestCase):
    """One study pressed once, read by every test."""

    @classmethod
    def setUpClass(cls):
        cls.dir = pathlib.Path(tempfile.mkdtemp(prefix="rowver-py-")) / "flat"
        cls.dir.mkdir()
        shutil.copy(FIXTURES / "uproot_flat.root", cls.dir)
        (cls.dir / "samples.toml").write_text(SAMPLES)
        (cls.dir / "recipe.toml").write_text(RECIPE)
        assert rvr(["samples", "resolve", str(cls.dir / "samples.toml")]) == 0
        assert rvr(["press", str(cls.dir)]) == 0
        cls.path = cls.dir / "flat.rvr"
        cls.file = rowver.open(cls.path)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.dir.parent)

    def test_the_file_says_what_it_holds(self):
        f = self.file
        self.assertEqual((f.rows, len(f), f.samples), (4, 4, ["flat"]))
        self.assertEqual(f.columns, ["weight", "nJet", "Jet_pt", "isGood", "ht"])
        self.assertEqual(f.sample_columns["xsec"], [1.5])
        self.assertEqual(f.sample_columns["label"], ["Flat ntuple"])
        self.assertEqual(f.recipe, RECIPE)
        self.assertEqual(rowver.validate(self.path), (4, 5))

    def test_flat_columns_are_numpy_arrays_of_their_kind(self):
        f = self.file
        np.testing.assert_array_equal(f["weight"], [1.0, 0.5, 2.25, 0.75])
        self.assertEqual((f["weight"].dtype, f["nJet"].dtype, f["isGood"].dtype), (np.float64, np.int32, np.bool_))
        np.testing.assert_array_equal(f["entry"], [0, 1, 2, 4])
        np.testing.assert_array_equal(f["sample"], [0, 0, 0, 0])
        values, offsets = f.read(["Jet_pt"])["Jet_pt"]
        np.testing.assert_array_equal(offsets, [0, 3, 7, 10, 11])
        self.assertEqual(values.dtype, np.float32)

    def test_arrays_handed_out_cannot_change_what_is_read_later(self):
        weight = self.file["weight"]
        with self.assertRaises(ValueError):
            weight *= 2
        np.testing.assert_array_equal(self.file.read("weight")["weight"][0], [1.0, 0.5, 2.25, 0.75])

    @unittest.skipUnless(has("awkward"), "needs awkward")
    def test_list_columns_are_awkward_arrays(self):
        pt = self.file["Jet_pt"]
        self.assertEqual(pt.tolist()[0], [65.3125, 19.96875, 16.421875])
        self.assertEqual(self.file.arrays().fields, self.file.columns)

    @unittest.skipUnless(has("pyarrow") and has("pandas"), "needs pyarrow and pandas")
    def test_arrow_pandas_and_parquet_hold_the_same(self):
        import pyarrow.parquet as pq

        table = self.file.to_arrow()
        self.assertEqual(table.column_names, ["sample", "entry", *self.file.columns])
        self.assertEqual(table["sample"].to_pylist(), ["flat"] * 4)
        frame = self.file.to_pandas(["weight", "ht"])
        self.assertEqual(list(frame.columns), ["sample", "entry", "weight", "ht"])
        np.testing.assert_allclose(frame["ht"], [101.703125, 154.046875, 104.46875, 120.5])
        # A list column asked for too changes nothing about the others' types.
        both = self.file.to_pandas(["sample", "weight", "Jet_pt"])
        self.assertEqual(list(both.columns), ["sample", "entry", "weight", "Jet_pt"])
        self.assertEqual((both["weight"].dtype, both["sample"].dtype.name), (np.float64, "category"))
        self.assertEqual(list(both["Jet_pt"][3]), [120.5])
        self.assertEqual(str(table.schema.field("Jet_pt").type), "large_list<item: float>")
        out = self.dir / "flat.parquet"
        self.file.to_parquet(out)
        self.assertTrue(pq.read_table(out).equals(table))

    def test_mistakes_are_said_plainly(self):
        with self.assertRaisesRegex(KeyError, "not a column of .*; it has weight"):
            self.file["MET"]
        with self.assertRaisesRegex(rowver.Error, "samples.toml: .*not a RowVer v13"):
            rowver.open(self.dir / "samples.toml")
        with self.assertRaises(FileNotFoundError):
            rowver.open(self.dir / "missing.rvr")
        self.assertEqual(rowver.Error.__module__, "rowver")


if __name__ == "__main__":
    unittest.main()
