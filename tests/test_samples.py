"""by_sample: one sha256 per sample on every per-row array, the same however the file was pressed,
and `rowver diff`, which names the column and the sample that changed from two headers alone."""
import hashlib
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402
from test_extend import edit  # noqa: E402
from test_press import Folder  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import diff, rvr  # noqa: E402

SCALED = ('"q": "q"}', '"q": "q", "scaled": lambda e, metadata: e.Muon_pt * metadata.get("k", 1)}')


def sha(array):
    """The sha256 of an array's little-endian bytes."""
    return hashlib.sha256(np.ascontiguousarray(array).astype(array.dtype.newbyteorder("<")).tobytes()).hexdigest()


def three(root):
    """A study of three samples whose middle one keeps no event: its file has no muon."""
    folder = study.make(root)
    study.tree(folder / "none.root", 40, 3, most=0)
    study.point(folder, {"alpha": [folder / "a.root"], "empty": [folder / "none.root"], "beta": [folder / "b.root"]})
    return folder


class BySample(Folder):
    def test_flat_and_jagged_columns_and_a_sample_with_no_rows(self):
        folder = three(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        header, section = rvr.read(folder / "tiny.rvr")
        data = header["data"]
        self.assertEqual([s["rows"] for s in data["samples"]], [159, 0, 98])
        self.assertNotIn("by_sample", data["sample"])
        spans = [(0, 159), (159, 159), (159, 257)]
        entry = rvr.array(section, data["entry"], "i8", 257)
        self.assertEqual(data["entry"]["by_sample"], [sha(entry[lo:hi]) for lo, hi in spans])
        met = next(c for c in data["cols"] if c["name"] == "MET")
        values = rvr.array(section, met, "f4", 257)
        self.assertEqual(met["by_sample"], [sha(values[lo:hi]) for lo, hi in spans])
        self.assertEqual(met["by_sample"][1], hashlib.sha256(b"").hexdigest())
        muons = next(c for c in data["cols"] if c["name"] == "Muon_pt")
        offsets = rvr.array(section, muons["offsets"], "i8", 258)
        content = rvr.array(section, muons["content"], "f4", muons["content"]["count"])
        self.assertEqual(muons["offsets"]["by_sample"], [sha(offsets[lo:hi + 1] - offsets[lo]) for lo, hi in spans])
        self.assertEqual(muons["offsets"]["by_sample"][1], sha(np.zeros(1, "<i8")))
        self.assertEqual(muons["content"]["by_sample"], [sha(content[offsets[lo]:offsets[hi]]) for lo, hi in spans])
        self.assertEqual(muons["content"]["by_sample"][1], hashlib.sha256(b"").hexdigest())
        # beta's offsets are counted from its own first row, so they are those of beta pressed alone
        study.point(folder, {"beta": [folder / "b.root"]})
        self.assertEqual(study.rowver("run", folder, "--out", folder / "beta.rvr").returncode, 0)
        alone = next(c for c in rvr.read_header(folder / "beta.rvr")["data"]["cols"] if c["name"] == "Muon_pt")
        self.assertEqual(alone["offsets"]["by_sample"], muons["offsets"]["by_sample"][2:])
        self.assertEqual(alone["content"]["by_sample"], muons["content"]["by_sample"][2:])
        result = study.rowver("validate", folder / "tiny.rvr")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("ok   Muon_pt offsets: by_sample has one sha256 per sample", result.stdout)

    def test_a_parallel_press_gives_the_same_data_section(self):
        folder = three(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        self.assertEqual(study.rowver("run", folder, "--out", folder / "two.rvr", "--parallel", "2").returncode, 0)
        self.assertEqual(rvr.read_header(folder / "two.rvr")["data"], rvr.read_header(folder / "tiny.rvr")["data"])

    def test_validate_catches_a_wrong_or_missing_by_sample(self):
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        path = folder / "tiny.rvr"
        header, section = rvr.read(path)
        met = next(c for c in header["data"]["cols"] if c["name"] == "MET")
        met["by_sample"] = met["by_sample"][::-1]
        del header["data"]["entry"]["by_sample"]
        rvr.write(path, header, [bytes(section)])
        result = study.rowver("validate", path)
        self.assertEqual(result.returncode, 1)
        failed = [line.strip() for line in result.stdout.splitlines() if "FAIL" in line]
        self.assertEqual(failed, ["FAIL entry: by_sample has one sha256 per sample, each matching its rows",
                                  "FAIL MET: by_sample has one sha256 per sample, each matching its rows"])


class Diff(Folder):
    def test_it_names_the_column_and_the_sample_that_changed_from_the_headers_alone(self):
        folder = study.make(self.root)
        edit(folder, SCALED)
        first, second = folder / "first.rvr", folder / "second.rvr"
        self.assertEqual(study.rowver("run", folder, "--out", first).returncode, 0)
        text = (folder / "cart.py").read_text(encoding="utf-8")
        cut = text.rindex("'metadata': {}")
        (folder / "cart.py").write_text(text[:cut] + "'metadata': {'k': 2}" + text[cut + 14:], encoding="utf-8")
        self.assertEqual(study.rowver("run", folder, "--out", second).returncode, 0)
        for path in (first, second):  # no array is left to decompress
            raw = path.read_bytes()
            path.write_bytes(raw[:12 + int.from_bytes(raw[8:12], "little")])
        self.assertEqual(diff.compare(first, second), {
            "rows": [257, 257],
            "samples": {"only_a": [], "only_b": [], "rows": {}},
            "columns": {"only_a": [], "only_b": [], "kind": {}},
            "entry": [],
            "changed": [{"column": "scaled", "sample": "beta", "parts": ["content"]}]})
        result = study.rowver("diff", first, second)
        self.assertEqual((result.returncode, result.stdout),
                         (1, "column scaled: differs in sample beta (content)\n"))
        same = study.rowver("diff", first, first)
        self.assertEqual((same.returncode, same.stdout.count("\n")), (0, 1))

    def test_samples_columns_rows_and_kinds_that_differ(self):
        folder = study.make(self.root)
        first, second = folder / "first.rvr", folder / "second.rvr"
        self.assertEqual(study.rowver("run", folder, "--out", first).returncode, 0)
        study.tree(folder / "c.root", 50, 3)
        study.point(folder, {"beta": [folder / "b.root"], "alpha": [folder / "a.root"], "gamma": [folder / "c.root"]})
        edit(folder, ("events.nMuon >= 1", "events.nMuon >= (2 if metadata['file'].endswith('a.root') else 1)"),
             ("def keep(events):", "def keep(events, metadata):"),
             ('"MET": "MET", ', '"MET": lambda e: np.asarray(e.MET).astype(np.float64), '), ('"q": "q"}', '"n2": "nMuon"}'))
        result = study.rowver("run", folder, "--out", second)
        self.assertEqual(result.returncode, 0, result.stderr)
        found = diff.compare(first, second)
        rows = rvr.read_header(second)["data"]["samples"]
        self.assertEqual(found["rows"], [257, sum(s["rows"] for s in rows)])
        self.assertEqual(found["samples"], {"only_a": [], "only_b": ["gamma"], "rows": {"alpha": [159, rows[1]["rows"]]}})
        self.assertEqual(found["columns"], {"only_a": ["q"], "only_b": ["n2"], "kind": {"MET": ["f4", "f8"]}})
        self.assertEqual(found["entry"], ["alpha"])
        # samples are matched by name, whatever their order: beta is unchanged, alpha is not
        self.assertEqual({(c["column"], c["sample"]) for c in found["changed"]},
                         {("Nmuon", "alpha"), ("lead_pt", "alpha"), ("Muon_pt", "alpha")})
        self.assertEqual(next(c for c in found["changed"] if c["column"] == "Muon_pt")["parts"], ["offsets", "content"])
        said = study.rowver("diff", first, second).stdout
        for line in (f"rows: 257 in {first}, {found['rows'][1]} in {second}", f"sample gamma: only in {second}",
                     f"column q: only in {first}", f"column MET: f4 in {first}, f8 in {second}",
                     "entry: differs in sample alpha", "column Muon_pt: differs in sample alpha (offsets, content)"):
            self.assertIn(line + "\n", said)



class Metadata(Folder):
    def test_plain_sample_metadata_is_carried_and_other_values_are_named(self):
        """A cross-section rides into made and on into a derived file; a set is left out by name."""
        folder = study.make(self.root)
        cart = folder / "cart.py"
        text = cart.read_text(encoding="utf-8")
        given = "{'xsec': 87.3, 'lumi': 59, 'tags': ['mc', True], 'more': {'k': None}, 'codes': {1, 2}}"
        cart.write_text(text.replace("'metadata': {}", "'metadata': " + given, 1), encoding="utf-8")
        study.rowver("run", folder)
        out = folder / "tiny.rvr"
        files = rvr.read_header(out)["made"]["files"]
        self.assertEqual(files["alpha"]["metadata"], {"xsec": 87.3, "lumi": 59, "tags": ["mc", True], "more": {"k": None}})
        self.assertEqual(files["alpha"]["metadata_omitted"], ["codes"])
        self.assertEqual(files["beta"], {"files": files["beta"]["files"], "metadata": {}})
        self.assertIn("\n  valid\n", study.rowver("validate", out).stdout)
        header, section = rvr.read(out)
        header["made"]["files"]["alpha"]["metadata"]["xsec"] = 1.0
        rvr.write(out, header, [bytes(section)])
        self.assertRegex(study.rowver("validate", out).stdout, r"FAIL\s+made sha256 matches")


if __name__ == "__main__":
    unittest.main()
