"""data.sample_cols: the samples' metadata as columns holding one value per sample, read for a row
through its sample index; what makes one, and what run, --onto, derive, validate, diff and dump do."""
import hashlib
import struct
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402
from test_extend import edit  # noqa: E402
from test_press import Folder, lines  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import diff, rvr  # noqa: E402
from rowver.derive import Refused, derive  # noqa: E402

SOURCE = "def weighted(columns):\n    return columns['MET'] * columns['xsec']\n"
# alpha keeps 159 rows, empty keeps none, beta keeps 98; gone has no file that can be read
METADATA = {
    "alpha": {"xsec": 87.3, "kind": "background", "year": 2018, "is_mc": True, "tags": ["a"], "mixed": 1,
              "flag": True, "codes": {1, 2}, "MET": 5, "file": "x", "lumi": 59, "n": 3, "big": 2 ** 70, "nothing": None},
    "empty": {"kind": "data", "year": 2018, "is_mc": False, "tags": ["b"], "mixed": "one", "codes": 3, "MET": 6,
              "lumi": 59.5, "big": 1, "nothing": None, "sample": "s", "entry": 1},
    "beta": {"xsec": 6025.2, "kind": "background", "year": 2018, "is_mc": True, "mixed": 2, "flag": False,
             "codes": 4, "MET": 7, "lumi": 60, "n": 4, "big": 2, "only": "β"},
    "gone": {"xsec": 1.5, "late": 1},
}
VALUES = {"xsec": ("f8", [87.3, None, 6025.2]), "kind": ("text", ["background", "data", "background"]),
          "year": ("i8", [2018, 2018, 2018]), "is_mc": ("b1", [True, False, True]),
          "lumi": ("f8", [59, 59.5, 60]), "n": ("f8", [3, None, 4]), "only": ("text", [None, None, "β"])}


def weighted(columns):
    """The MET times the sample's cross-section."""
    return columns["MET"] * columns["xsec"]


def three(root, metadata=METADATA):
    """A study of three samples with metadata, whose middle one keeps no event, and a fourth
    whose only file is not there."""
    folder = study.make(root)
    study.tree(folder / "none.root", 40, 3, most=0)
    study.point(folder, {"alpha": [folder / "a.root"], "empty": [folder / "none.root"], "beta": [folder / "b.root"],
                         "gone": [folder / "gone.root"]}, metadata)
    return folder


def fails(test, path, *names):
    """Assert validate fails on exactly the checks that start with these names."""
    result = study.rowver("validate", path)
    failed = [line.strip()[5:] for line in result.stdout.splitlines() if line.startswith("  FAIL")]
    test.assertEqual((result.returncode, len(failed)), (1, len(names)), result.stdout)
    for line, name in zip(failed, names):
        test.assertTrue(line.startswith(name), (line, name))


class SampleCols(Folder):
    def setUp(self):
        super().setUp()
        self.folder = three(self.root)
        self.assertEqual(study.rowver("run", self.folder).returncode, 0)
        self.path = self.folder / "tiny.rvr"

    def changed(self, change):
        """A copy of the file with its header changed; its path."""
        header, section = rvr.read(self.path)
        change(header)
        rvr.write(self.folder / "changed.rvr", header, [bytes(section)])
        return self.folder / "changed.rvr"

    def test_kinds_nulls_and_the_keys_that_make_no_sample_column(self):
        header, section = rvr.read(self.path)
        data = header["data"]
        self.assertEqual([(s["name"], s["rows"]) for s in data["samples"]], [("alpha", 159), ("empty", 0), ("beta", 98)])
        # a list, mixed kinds, a truth value one sample lacks, a value not plain, a row column's
        # name, the reserved names, a number past i8, a key with no value, and a sample left out
        self.assertEqual(data["sample_cols"], [{"name": name, "kind": kind, "values": values,
                                                "sha256": rvr.sample_checksum(kind, values)}
                                               for name, (kind, values) in VALUES.items()])
        self.assertEqual(rvr.sample_checksum("f8", [87.3, None, 6025.2]),
                         hashlib.sha256(struct.pack("<d", 87.3) + bytes.fromhex("000000000000f87f")
                                        + struct.pack("<d", 6025.2)).hexdigest())
        self.assertEqual(rvr.sample_checksum("f8", [59, 59.5]), rvr.sample_checksum("f8", [59.0, 59.5]))
        self.assertEqual(rvr.sample_checksum("text", ["β", None]),
                         hashlib.sha256(b"\x01" + "β".encode() + b"\x00\x00").hexdigest())
        self.assertEqual(rvr.sample_checksum("i8", [2018, -1]), hashlib.sha256(struct.pack("<qq", 2018, -1)).hexdigest())
        self.assertEqual(rvr.sample_checksum("b1", [True, False]), hashlib.sha256(b"\x01\x00").hexdigest())
        with self.assertRaisesRegex(ValueError, "names a sample"):
            rvr.broadcast({"kind": "f8", "values": [1.5, 2.5]}, np.array([0, -1, 1]))
        self.assertEqual(header["made"]["files"]["alpha"]["metadata"]["tags"], ["a"])
        self.assertEqual(header["made"]["files"]["alpha"]["metadata_omitted"], ["codes"])
        self.assertEqual(data["sha256"], rvr.data_checksum(data))
        result = study.rowver("validate", self.path)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("ok   sample column xsec: sha256 matches its values", result.stdout)
        self.assertIn("ok   sample_cols are the metadata of made's files", result.stdout)

    def test_a_row_has_its_samples_value(self):
        header, section = rvr.read(self.path)
        rows = np.repeat([0, 2], [159, 98])
        self.assertEqual(rvr.sample_values(header, "xsec"), [87.3, None, 6025.2])
        xsec = rvr.sample_column(header, section, "xsec")
        self.assertEqual((xsec.dtype, xsec.tolist()), (np.dtype("f8"), np.array([87.3, 0, 6025.2])[rows].tolist()))
        year, mc, kind = (rvr.sample_column(header, section, name) for name in ("year", "is_mc", "kind"))
        self.assertEqual((year.dtype, set(year.tolist())), (np.dtype("i8"), {2018}))
        self.assertEqual((mc.dtype, mc.all(), len(mc)), (np.dtype(bool), True, 257))
        self.assertEqual((kind.dtype, set(kind.tolist())), (np.dtype(object), {"background"}))
        only = rvr.sample_column(header, section, "only")
        self.assertEqual(only.tolist(), [None] * 159 + ["β"] * 98)
        record = {"kind": "f8", "values": [1, None]}
        self.assertTrue(np.array_equal(rvr.broadcast(record, np.array([1, 0, 1])), [np.nan, 1, np.nan], equal_nan=True))

    def test_a_parallel_press_gives_the_same_data_section(self):
        two = self.folder / "two.rvr"
        self.assertEqual(study.rowver("run", self.folder, "--out", two, "--parallel", "2").returncode, 0)
        self.assertEqual(rvr.read_header(two)["data"], rvr.read_header(self.path)["data"])

    def test_onto_keeps_the_sample_columns_and_refuses_a_column_of_ones_name(self):
        before = rvr.read_header(self.path)["data"]["sample_cols"]
        edit(self.folder, ('"q": "q"}', '"q": "q", "twice": lambda e: e.MET * 2}'))
        added = self.folder / "added.rvr"
        result = study.rowver("run", self.folder, "--onto", self.path, "--out", added)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(rvr.read_header(added)["data"]["sample_cols"], before)
        self.assertEqual(study.rowver("validate", added).returncode, 0)
        whole = self.folder / "whole.rvr"
        self.assertEqual(study.rowver("run", self.folder, "--out", whole).returncode, 0)
        self.assertEqual(rvr.read_header(whole)["data"], rvr.read_header(added)["data"])
        # a row column wins the name: a press drops the sample column, and an add is refused
        edit(self.folder, ('"q": "q", ', '"q": "q", "xsec": "MET", "year": "nMuon", '))
        for extra in ((), ("--plan",)):
            result = study.rowver("run", self.folder, "--onto", added, "--out", self.folder / "more.rvr",
                                  "--progress", "json", *extra)
            last = lines(result.stdout)[-1]
            self.assertEqual((result.returncode, last["event"], last["kind"]), (1, "error", "differs"))
            self.assertEqual(last["message"], "The cartridge holds xsec and year as a sample column, made from the "
                             "samples' metadata, and the recipe now makes a column of that name. Press again to "
                             "make the cartridge anew.")
        self.assertFalse((self.folder / "more.rvr").exists())
        self.assertEqual(study.rowver("run", self.folder, "--out", whole).returncode, 0)
        self.assertEqual([c["name"] for c in rvr.read_header(whole)["data"]["sample_cols"]],
                         ["kind", "is_mc", "lumi", "n", "only"])
        self.assertEqual(study.rowver("validate", whole).returncode, 0)

    def test_derive_reads_a_sample_column_and_the_file_carries_them(self):
        out = self.folder / "derived.rvr"
        seen = {}

        def tagged(columns):
            """Whether the row is simulated; asking whether a name is held reads nothing."""
            seen.update(held=("kind" in columns, "tags" in columns), names=list(columns)[-2:])
            return columns["is_mc"] & (columns["year"] == 2018)

        header = derive(self.path, out, SOURCE, {"weighted": weighted, "tagged": tagged}, by="test 1.0")
        above, held = rvr.read(self.path)
        self.assertEqual(seen, {"held": (True, False), "names": ["n", "only"]})
        self.assertEqual(header["data"]["sample_cols"], above["data"]["sample_cols"])
        cols = {c["name"]: c for c in above["data"]["cols"]}
        shared = {c["name"]: c["sha256"] for c in above["data"]["sample_cols"]}
        self.assertEqual(header["derived"]["parents"][0]["read"],
                         {"MET": cols["MET"]["sha256"], "xsec": shared["xsec"], "is_mc": shared["is_mc"],
                          "year": shared["year"]})
        met, got = rvr.column(above, held, "MET"), rvr.column(*rvr.read(out), "weighted")
        self.assertEqual(got.dtype, np.dtype("f8"))
        self.assertTrue(np.array_equal(got, met * np.repeat([87.3, 6025.2], [159, 98])))
        self.assertTrue(rvr.column(*rvr.read(out), "tagged").all())
        alone, both = study.rowver("validate", out), study.rowver("validate", out, "--parent", self.path)
        self.assertEqual((alone.returncode, both.returncode), (0, 0), both.stdout)
        self.assertIn("ok   sample_cols are the metadata of made's files", alone.stdout)
        self.assertIn("ok   parent: sample_cols are this file's", both.stdout)
        self.assertIn("ok   parent: every column and sample column read has the sha256 recorded", both.stdout)
        # a derived column may not take a sample column's name, and a derived parent offers them too
        with self.assertRaisesRegex(Refused, "The parent already holds xsec as a sample column"):
            derive(self.path, self.folder / "no.rvr", SOURCE, {"xsec": weighted}, by="test")
        again = derive(out, self.folder / "again.rvr", "", {"w2": lambda c: c["weighted"] * c["lumi"]}, by="test")
        self.assertEqual(again["derived"]["parents"][0]["read"],
                         {"weighted": header["data"]["cols"][0]["sha256"], "lumi": shared["lumi"]})
        self.assertEqual(study.rowver("validate", self.folder / "again.rvr", "--parent", out).returncode, 0)
        # the parent pressed with another cross-section is not the parent, and says so by the sample column
        other = self.folder / "other.rvr"
        edit(self.folder, ("6025.2", "6100.0"))
        self.assertEqual(study.rowver("run", self.folder, "--out", other).returncode, 0)
        said = study.rowver("validate", out, "--parent", other).stdout
        for line in ("parent: sample_cols are this file's", "parent: every column and sample column read"):
            self.assertIn("FAIL " + line, said)
        # values that are not the ones the parent's checksum names are not read
        broken = self.changed(lambda h: h["data"]["sample_cols"][0]["values"].__setitem__(0, 1.0))
        with self.assertRaisesRegex(Refused, "The parent's sample columns are not what its metadata gives"):
            derive(broken, self.folder / "no.rvr", SOURCE, {"weighted": weighted}, by="test")
        self.assertFalse((self.folder / "no.rvr").exists())

    def test_validate_catches_changed_values_a_wrong_sha256_and_drift_from_made(self):
        cols, agree = "sample_cols: each has a name", "sample_cols are the metadata"

        def resealed(header, place, value):
            """Change one value and give the column the sha256 of its new values."""
            column = header["data"]["sample_cols"][0]
            column["values"][place] = value
            column["sha256"] = rvr.sample_checksum(column["kind"], column["values"])

        def remade(header):
            """Another cross-section in made, under a checksum that is right."""
            header["made"]["files"]["beta"]["metadata"]["xsec"] = 1.0
            header["made"]["sha256"] = rvr.made_checksum(header["made"])

        fails(self, self.changed(lambda h: h["data"]["sample_cols"][0]["values"].__setitem__(0, 1.0)),
              "sample column xsec: sha256", agree)
        fails(self, self.changed(lambda h: h["data"]["sample_cols"][0].update(sha256="0" * 64)),
              "sample column xsec: sha256", agree)
        fails(self, self.changed(lambda h: resealed(h, 0, 1.0)), agree)
        fails(self, self.changed(lambda h: resealed(h, 0, 87)), agree)  # 87 is not the 87.3 of made
        fails(self, self.changed(remade), agree)
        fails(self, self.changed(lambda h: h["data"]["sample_cols"].pop()), agree)
        self.assertEqual(study.rowver("validate", self.changed(lambda h: h["data"]["sample_cols"].reverse())).returncode, 0)
        fails(self, self.changed(lambda h: h["data"]["sample_cols"][3].update(values=[1, 0, 1])), cols)
        fails(self, self.changed(lambda h: h["data"]["sample_cols"][2].update(values=[2018, None, 2018])), cols)
        fails(self, self.changed(lambda h: h["data"]["sample_cols"][0]["values"].pop()), cols)
        fails(self, self.changed(lambda h: h["data"].pop("sample_cols")), cols)
        fails(self, self.changed(lambda h: h["data"]["sample_cols"][0].update(name="MET")),
              "sample_cols: names are distinct", agree)

    def test_diff_names_the_sample_whose_value_changed(self):
        first, second = self.path, self.folder / "second.rvr"
        edit(self.folder, ("6025.2", "6100.0"), ("'year': 2018, 'is_mc': True, 'tags'", "'year': 2018.5, 'is_mc': True, 'tags'"),
             (", 'only': 'β'", ", 'more': 'x'"))
        self.assertEqual(study.rowver("run", self.folder, "--out", second).returncode, 0)
        for path in (first, second):  # no array is left to decompress
            raw = path.read_bytes()
            path.write_bytes(raw[:12 + int.from_bytes(raw[8:12], "little")])
        found = diff.compare(first, second)
        self.assertEqual(found["changed"], [])
        self.assertEqual(found["sample_cols"], {
            "only_a": ["only"], "only_b": ["more"], "kind": {"year": ["i8", "f8"]},
            "changed": [{"column": "xsec", "sample": "beta", "values": [6025.2, 6100.0]}]})
        result = study.rowver("diff", first, second)
        self.assertEqual((result.returncode, result.stdout.splitlines()), (1, [
            f"sample column only: only in {first}", f"sample column more: only in {second}",
            f"sample column year: i8 in {first}, f8 in {second}",
            f"sample column xsec: differs in sample beta: 6025.2 in {first}, 6100.0 in {second}"]))

    def test_dump_shows_each_sample_column_and_a_parent_named_for_a_pressed_file_is_said(self):
        dumped = study.rowver("dump", self.path, "--columns").stdout
        self.assertIn("xsec           f8 by sample[3]  alpha: 87.3, empty: null, beta: 6025.2\n", dumped)
        self.assertIn('only           text by sample[3]  alpha: null, empty: null, beta: "β"\n', dumped)
        self.assertIn('"sample_cols": [', study.rowver("dump", self.path).stdout)
        result = study.rowver("validate", self.path, "--parent", self.path)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(result.stdout.count("  --   the parent was not compared: this file is not derived\n"), 1)
        self.assertNotIn("not compared", study.rowver("validate", self.path).stdout)


if __name__ == "__main__":
    unittest.main()
