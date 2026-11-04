"""The Python package on a file pressed here, from the ROOT fixtures rvr's own tests use: the values
read back, their types, and the Arrow, pandas and Parquet forms. Run from this crate's folder,
after `maturin develop`: `python -m unittest discover tests`."""

import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

import numpy as np

import rowver
from rowver._rowver import rvr

FIXTURES = pathlib.Path(__file__).resolve().parents[2] / "rvr" / "tests" / "fixtures"

SAMPLES = """format = "rvr-samples"
version = 1

[places]

[defaults]

[eras]

[[sample]]
name = "flat"
label = "Flat ntuple"
files = "uproot_flat.root"
tree = "analysis"
metadata = { xsec = 1.5 }
"""

RECIPE = """format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["weight", "nJet", "Jet_pt", "isGood"]
ht = "sum(Jet_pt)"

[totals]
sumw = "weight"

[derived]
ht_per_jet = "ht / nJet"
hard_pt = "Jet_pt[Jet_pt > 30]"
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
        cls.config = mock.patch.dict(os.environ, {"RVR_CONFIG": str(cls.dir.parent / "config")})
        cls.config.start()
        shutil.copy(FIXTURES / "uproot_flat.root", cls.dir)
        (cls.dir / "samples.toml").write_text(SAMPLES)
        (cls.dir / "recipe.toml").write_text(RECIPE)
        assert rvr(["samples", "resolve", str(cls.dir / "samples.toml")]) == 0
        assert rvr(["press", str(cls.dir)]) == 0
        cls.path = cls.dir / "flat.rvr"
        cls.file = rowver.open(cls.path)

    @classmethod
    def tearDownClass(cls):
        cls.config.stop()
        shutil.rmtree(cls.dir.parent)

    # SPEC 4.5.1, 7.8.1, 11.3, 11.7; FORMAT 5
    def test_the_file_says_what_it_holds(self):
        f = self.file
        self.assertEqual((f.rows, len(f), f.samples), (4, 4, ["flat"]))
        self.assertEqual(f.columns, ["weight", "nJet", "Jet_pt", "isGood", "ht"])
        self.assertEqual(f.sample_columns["xsec"], [1.5])
        # A total: the sum of weight over every event read, before the cut.
        self.assertEqual(f.sample_columns["sumw"], [5.5])
        self.assertNotIn("label", f.sample_columns)
        self.assertEqual(f.labels, ["Flat ntuple"])
        self.assertEqual(f.recipe, RECIPE)
        self.assertEqual(f.study["samples.toml"], SAMPLES)
        self.assertEqual(f.study["samples.lock"], (self.dir / "samples.lock").read_text())
        self.assertIsNone(f.signed_by)
        self.assertEqual(rowver.validate(self.path), (4, 5))

    # SPEC 11.3, 11.4, 11.5; FORMAT 10; RECIPE choosing-members-of-a-list
    def test_derived_columns_are_worked_out_and_read_as_any_other(self):
        f = self.file
        self.assertEqual(f.derived, ["ht_per_jet", "hard_pt"])
        self.assertIn("ht_per_jet", f)
        self.assertIn("2 derived", repr(f))
        np.testing.assert_array_equal(f["ht_per_jet"], f["ht"] / f["nJet"])
        self.assertEqual(f["ht_per_jet"].dtype, np.float64)
        self.assertFalse(f["ht_per_jet"].flags.writeable)
        if has("awkward"):
            import awkward
            self.assertEqual(awkward.to_list(f["hard_pt"]), [[p for p in row if p > 30] for row in awkward.to_list(f["Jet_pt"])])
        f.forget("ht_per_jet")
        self.assertNotIn("ht_per_jet", f._held)
        np.testing.assert_array_equal(f["ht_per_jet"], f["ht"] / f["nJet"])
        f.forget()
        self.assertEqual(f._held, {})

    # SPEC 11.7; FORMAT 10, 12
    def test_validate_works_out_every_derived_column(self):
        # One hex digit of a derived record changed: every stored checksum still holds.
        data = bytearray(self.path.read_bytes())
        at = data.index(b'"sha256":"', data.index(b'"name":"ht_per_jet"')) + len(b'"sha256":"')
        data[at] = ord("0") if data[at] != ord("0") else ord("1")
        changed = self.dir / "changed.rvr"
        changed.write_bytes(data)
        with self.assertRaisesRegex(rowver.Error, r'derived\.columns\["ht_per_jet"\], worked out here, is not what'):
            rowver.validate(changed)
        changed.unlink()

    # SPEC 4.4.1, 11.4, 11.5; FORMAT 3, 4
    def test_flat_columns_are_numpy_arrays_of_their_kind(self):
        f = self.file
        np.testing.assert_array_equal(f["weight"], [1.0, 0.5, 2.25, 0.75])
        self.assertEqual((f["weight"].dtype, f["nJet"].dtype, f["isGood"].dtype), (np.float64, np.int32, np.bool_))
        np.testing.assert_array_equal(f["entry"], [0, 1, 2, 4])
        np.testing.assert_array_equal(f["sample"], [0, 0, 0, 0])
        values, offsets = f.read(["Jet_pt"])["Jet_pt"]
        np.testing.assert_array_equal(offsets, [0, 3, 7, 10, 11])
        self.assertEqual(values.dtype, np.float32)

    # SPEC 11.4
    def test_arrays_handed_out_cannot_change_what_is_read_later(self):
        weight = self.file["weight"]
        with self.assertRaises(ValueError):
            weight *= 2
        np.testing.assert_array_equal(self.file.read("weight")["weight"][0], [1.0, 0.5, 2.25, 0.75])

    # SPEC 11.4, 11.6
    @unittest.skipUnless(has("awkward"), "needs awkward")
    def test_list_columns_are_awkward_arrays(self):
        pt = self.file["Jet_pt"]
        self.assertEqual(pt.tolist()[0], [65.3125, 19.96875, 16.421875])
        arrays = self.file.arrays()
        self.assertEqual(arrays.fields, ["sample", "entry", *self.file.columns, *self.file.derived])
        self.assertEqual((arrays["sample"].tolist(), arrays["entry"].tolist()), (["flat"] * 4, [0, 1, 2, 4]))
        self.assertEqual(self.file.arrays(["ht", "sample"]).fields, ["sample", "entry", "ht"])

    # SPEC 11.6
    @unittest.skipUnless(has("pyarrow") and has("pandas"), "needs pyarrow and pandas")
    def test_arrow_pandas_and_parquet_hold_the_same(self):
        import pyarrow.parquet as pq

        table = self.file.to_arrow()
        # Every column when none is named: the derived ones too, worked out.
        self.assertEqual(table.column_names, ["sample", "entry", *self.file.columns, "ht_per_jet", "hard_pt"])
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

    # SPEC 1.5, 11.2, 11.4; FORMAT 1
    def test_mistakes_are_said_plainly(self):
        with self.assertRaisesRegex(KeyError, "not a column of .*; it has weight"):
            self.file["MET"]
        with self.assertRaisesRegex(rowver.Error, "samples.toml: .*not an .rvr file"):
            rowver.open(self.dir / "samples.toml")
        with self.assertRaisesRegex(FileNotFoundError, "missing.rvr is not there"):
            rowver.open(self.dir / "missing.rvr")
        with self.assertRaisesRegex(rowver.Error, "is a folder, not a file"):
            rowver.open(self.dir)
        self.assertEqual(rowver.Error.__module__, "rowver")

    # SPEC 11.2, 11.3, 11.5, 11.6, 11.7
    def test_the_names_it_offers_are_the_ones_it_documents(self):
        public = {n for n in dir(rowver) if not n.startswith("_")}
        self.assertEqual(public, {"Error", "File", "open", "validate"})
        documented = {"path", "header", "rows", "samples", "labels", "columns", "derived", "sample_columns", "recipe",
                      "study", "signed_by", "holes", "complete", "empty", "read", "forget", "arrays", "to_arrow",
                      "to_pandas", "to_parquet"}
        self.assertEqual({n for n in dir(self.file) if not n.startswith("_")}, documented)
        self.assertIn("sample", self.file)

    # SPEC 11.4
    def test_a_file_replaced_by_rename_after_it_was_opened_is_read_as_it_was(self):
        copy, other = self.dir / "renamed.rvr", self.dir / "other.rvr"
        shutil.copy(self.path, copy)
        try:
            f = rowver.open(copy)
            other.write_bytes(b"\x00" * 64 + self.path.read_bytes())
            os.replace(other, copy)
            np.testing.assert_array_equal(f["weight"], [1.0, 0.5, 2.25, 0.75])
            self.assertEqual(len(f["ht_per_jet"]), 4)
        finally:
            copy.unlink()

    # SPEC 1.5, 11.4; FORMAT 12
    def test_a_file_emptied_after_it_was_opened_says_so(self):
        copy = self.dir / "cut.rvr"
        shutil.copy(self.path, copy)
        try:
            f = rowver.open(copy)
            whole = copy.read_bytes()
            copy.write_bytes(whole[:12 + int.from_bytes(whole[8:12], "little")])
            with self.assertRaisesRegex(rowver.Error, "cut.rvr changed while it was read \\(emptied, or written again\\): open it again"):
                f["weight"]
        finally:
            copy.unlink()

    # SPEC 11.3
    def test_a_complete_file_has_no_holes(self):
        self.assertEqual((self.file.complete, self.file.holes), (True, []))

    # SPEC 8.9, 8.10, 11.3; FORMAT 1
    def test_an_empty_file_reads_its_header_and_says_how_to_fill_it(self):
        copy = self.dir / "copy.rvr"
        shutil.copy(self.path, copy)
        try:
            whole = copy.read_bytes()
            self.assertEqual(rvr(["empty", str(copy), "--force"]), 0)
            f = rowver.open(copy)
            self.assertEqual((f.empty, self.file.empty, f.rows, f.columns), (True, False, self.file.rows, self.file.columns))
            self.assertIn(", empty>", repr(f))
            with self.assertRaisesRegex(rowver.Error, "the file is empty: .*rvr refill"):
                f["weight"]
            self.assertEqual(rvr(["refill", str(copy)]), 0)
            self.assertEqual(copy.read_bytes(), whole)
        finally:
            copy.unlink()


class Holed(unittest.TestCase):
    """A sample of two files, one deleted after the lock was written: a file with one hole."""

    # SPEC 5.1.6, 7.3.1, 7.3.2, 7.3.3, 7.3.4, 11.3; FORMAT 5, 6.2
    def test_the_hole_says_which_entries_of_which_file_and_why(self):
        dir = pathlib.Path(tempfile.mkdtemp(prefix="rowver-py-")) / "holed"
        dir.mkdir()
        try:
            self.enterContext(mock.patch.dict(os.environ, {"RVR_CONFIG": str(dir.parent / "config")}))
            for name in ("flat_a.root", "flat_b.root"):
                shutil.copy(FIXTURES / "uproot_flat.root", dir / name)
            (dir / "samples.toml").write_text(SAMPLES.replace("uproot_flat.root", "flat_*.root"))
            (dir / "recipe.toml").write_text(RECIPE)
            assert rvr(["samples", "resolve", str(dir / "samples.toml")]) == 0
            (dir / "flat_b.root").unlink()
            self.assertEqual(rvr(["press", str(dir)]), 3)
            f = rowver.open(dir / "holed.rvr")
            self.assertFalse(f.complete)
            [hole] = f.holes
            self.assertEqual((hole["sample"], hole["entries"], hole["kind"]), ("flat", (0, 5), "gone"))
            self.assertTrue(hole["file"].endswith("flat_b.root") and "no longer exists" in hole["why"])
            self.assertEqual(f.sample_columns["events_missing"], [5])
        finally:
            shutil.rmtree(dir.parent)


class Command(unittest.TestCase):
    """The rvr command the package installs."""

    # SPEC 5.1.3, 5.1.6, 11.1
    def test_an_argument_that_is_not_utf8_is_wrong_use(self):
        import sys

        from rowver import _cli

        # rvr writes to the process's standard error itself: caught at its file descriptor.
        with tempfile.TemporaryFile() as said, mock.patch.object(sys, "argv", ["rvr", "validate", "caf\udce9.rvr"]):
            kept = os.dup(2)
            os.dup2(said.fileno(), 2)
            try:
                with self.assertRaises(SystemExit) as stopped:
                    _cli.main()
            finally:
                os.dup2(kept, 2)
                os.close(kept)
            said.seek(0)
            self.assertEqual(stopped.exception.code, 2)
            self.assertIn(b"is not UTF-8 text", said.read())

CORPUS = pathlib.Path(__file__).resolve().parents[3] / "tests" / "conformance" / "rvr"


def zstd():
    """A zstd decompressor: 3.14's compression.zstd, the zstandard package, or the zstd command;
    None when there is none."""
    try:
        from compression import zstd as module
        return module.decompress
    except ImportError:
        pass
    try:
        import zstandard
        return lambda data: zstandard.ZstdDecompressor().decompress(data, max_output_size=1 << 30)
    except ImportError:
        pass
    command = shutil.which("zstd")
    if command:
        return lambda data: subprocess.run([command, "-d", "-c"], input=data, capture_output=True, check=True).stdout
    return None


def decoded(column):
    """A column of the corpus's manifest as the numbers its values are: floats from their bits."""
    kind = column["kind"]
    if kind in ("f4", "f8"):
        return np.array([int(v, 16) for v in column["values"]], dtype="u4" if kind == "f4" else "u8").view(kind)
    return np.array(column["values"], dtype="?" if kind == "b1" else kind)


class Conformance(unittest.TestCase):
    """The conformance corpus (tests/conformance/rvr), read through the Python package."""

    def check(self, lists):
        """Every valid file of the corpus reads as its manifest says: its flat columns, or its list ones."""
        manifest = json.loads((CORPUS / "manifest.json").read_text())
        for entry in manifest["valid"]:
            want = entry["contents"]
            with self.subTest(entry["file"]):
                f = rowver.open(str(CORPUS / entry["file"]))
                self.assertEqual((f.rows, f.empty), (want["rows"], want["empty"]))
                self.assertEqual(f.signed_by is not None, entry["signed"])
                self.assertEqual(rowver.validate(str(CORPUS / entry["file"]))[0], want["rows"])
                if want["empty"]:
                    continue
                self.assertEqual(f["sample"].tolist(), want["sample"])
                self.assertEqual(f["entry"].tolist(), want["entry"])
                for name, column in want["columns"].items():
                    if ("offsets" in column) != lists:
                        continue
                    values = f[name]
                    if lists:
                        import awkward as ak
                        self.assertEqual(ak.num(values).tolist(), np.diff(column["offsets"]).tolist(), name)
                        values = ak.to_numpy(ak.flatten(values))
                    got = np.asarray(values)
                    self.assertEqual(got.dtype, decoded(column).dtype, name)
                    # Bit for bit, NaN payloads and signed zeros included.
                    self.assertEqual(got.tobytes(), decoded(column).tobytes(), name)

    # SPEC 11.2, 11.3, 11.4, 11.7; FORMAT 12
    def test_every_valid_file_of_the_corpus_reads_as_its_manifest_says(self):
        self.check(lists=False)

    # SPEC 11.4; FORMAT 12
    @unittest.skipUnless(has("awkward"), "needs awkward")
    def test_every_list_column_of_the_corpus_reads_as_its_manifest_says(self):
        self.check(lists=True)

    # FORMAT 12
    @unittest.skipUnless(zstd(), "no zstd here: Python 3.14, the zstandard package, or the zstd command")
    def test_every_case_of_the_corpus_has_its_verdict(self):
        manifest = json.loads((CORPUS / "manifest.json").read_text())
        packed = zstd()((CORPUS / "cases.zst").read_bytes())
        with tempfile.TemporaryDirectory() as folder:
            for n, case in enumerate(manifest["cases"]):
                path = os.path.join(folder, f"{n}.rvr")
                with open(path, "wb") as out:
                    out.write(packed[case["at"]:case["at"] + case["size"]])
                with self.subTest(case["rule"]):
                    if case["verdict"] == "opening":
                        self.assertRaises(rowver.Error, rowver.open, path)
                    else:
                        rowver.open(path)
                        if case["verdict"] == "full":
                            self.assertRaises(rowver.Error, rowver.validate, path)
                        else:
                            rowver.validate(path)


if __name__ == "__main__":
    unittest.main()
