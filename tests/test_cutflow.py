"""`rowver extract cutflow`: fixture-pinned ladder semantics + golden end-to-end.

The `cutflow` block of contract/core/cases.json is asserted here and by
rowver_web's cutflow.test.ts — including the NaN split (a NaN FAILS a ladder
step but PASSES the selection mask's range cut, the page's own behavior) and
the sankey's step-label formatting. Raw counts exact; weighted sums exact
here too — the numbers are small by design (one case pins row-order
accumulation with values exact at every partial sum).
"""
from __future__ import annotations

import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6
from rowver.extract import ExtractError, core, cutflow

from test_flattened import _golden

FIXTURE = Path(__file__).resolve().parent.parent / "contract/core/cases.json"
CASES = json.loads(FIXTURE.read_text(encoding="utf-8"))


STR = {"nan": math.nan, "inf": math.inf, "-inf": -math.inf}


def dec(vals):
    return np.array([math.nan if v is None else STR[v]
                     if isinstance(v, str) else v for v in vals], float)


def _pieces(c):
    samples = c["samples"]
    columns = {name: dec(vals) for name, vals in c["values"].items()}
    shown = {s: (s in c["shown_on"] if c["shown_on"] is not None else True)
             for s in samples}
    ds = np.array(c["ds"], np.int32)
    view = {"groups": {"of": c["groups"]}}
    if "w" in c:
        # raw weights + lumis: ww is COMPOSED here, pinning row_weights too
        view["lumis"] = c["lumis"]
        unw = set(c.get("unweighted_samples") or [])
        meta = {"data": {"weight_col": "w",
                         "samples": [{"name": s, "weighted": s not in unw}
                                     for s in samples]},
                "view": view}
        ww = core.row_weights(meta, {"w": dec(c["w"])}, ds)
    elif c["weighted"]:
        ww = dec(c["ww"])
    else:
        ww = np.ones(ds.size)
    return columns, c["cols"], c["cuts"], shown, ds, samples, ww, view


class CutflowFixtureTests(unittest.TestCase):
    def test_cutflow_cases(self):
        for c in CASES["cutflow"]:
            got = core.cutflow(*_pieces(c))
            x = c["expect"]
            self.assertEqual(got["steps"], x["steps"], c["id"])
            self.assertEqual(list(got["raw"]), [float(v) for v in x["raw"]], c["id"])
            np.testing.assert_array_equal(got["w"], dec(x["w"]), c["id"])
            self.assertEqual(got["sources"], x["sources"], c["id"])
            np.testing.assert_array_equal(
                got["by"], np.array([dec(row) for row in x["by"]]), c["id"])

    def test_mask_split_pinned(self):
        # the ladder and the mask disagree exactly where the fixture says:
        # every case carries the mask's kept total alongside the ladder's rows
        for c in CASES["cutflow"]:
            columns, cols, cuts, shown, ds, samples, _, _ = _pieces(c)
            kept = core.sel_mask(columns, cols, cuts, shown, ds, samples)
            self.assertEqual(int(kept.sum()), c["expect_mask_kept"], c["id"])


class SourceLookTests(unittest.TestCase):
    # frame.ts sourceLook(): the same group -> subgroup -> sample namespace
    # order as source membership — a name styled in the WRONG tier must miss
    def test_namespace_routing(self):
        samples = ["s1", "s2", "s3"]
        view = {"groups": {"of": {"s1": "G"}, "sub": {"s2": "SG"}},
                "look": {"group": {"G": {"colour": "#111"},
                                   "s3": {"colour": "#bad"}},
                         "sub": {"SG": {"colour": "#222"}},
                         "sample": {"s3": {"colour": "#333"}}}}
        self.assertEqual(core.source_look(view, samples, "G"), {"colour": "#111"})
        self.assertEqual(core.source_look(view, samples, "SG"), {"colour": "#222"})
        # s3 is neither a group nor a subgroup: only the sample tier reaches it
        self.assertEqual(core.source_look(view, samples, "s3"), {"colour": "#333"})
        self.assertIsNone(core.source_look(view, samples, "s2"))


class CutflowGoldenTests(unittest.TestCase):
    """The authored cartridge from test_flattened, walked as a cutflow."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.rvr = _golden(self.tmp)

    def tearDown(self):
        self._tmp.cleanup()

    # tab 0: cuts MET >= 100, wj hidden, lumis sig->3 (sample), bkg->2 (group).
    # ww: sig [1.5, 3, 6]; tt [2, 2]; obs (data) [1, 1, 1].
    def test_tab0(self):
        flow = cutflow.dump(self.rvr)
        self.assertEqual(flow["steps"], ["no cuts", "MET ≥ 100"])
        self.assertEqual(list(flow["raw"]), [8.0, 6.0])
        self.assertEqual(list(flow["w"]), [17.5, 15.0])
        self.assertEqual(flow["sources"], ["sig", "bkg", "obs"])
        np.testing.assert_array_equal(
            flow["by"], np.array([[10.5, 4.0, 3.0], [9.0, 4.0, 2.0]]))
        self.assertTrue(flow["weighted"])
        self.assertEqual(flow["lumi"],
                         {"sig": 3.0, "tt": 2.0, "wj": 2.0, "obs": 1.0})
        self.assertEqual(flow["cuts"], {"MET": {"range": [100.0, "inf"]}})

    # tab 1: no cuts, everything shown — one totals row, wj back (ww 20 each).
    def test_tab1_empty_ladder(self):
        flow = cutflow.dump(self.rvr, tab=1)
        self.assertEqual(flow["steps"], ["no cuts"])
        self.assertEqual(list(flow["raw"]), [10.0])
        self.assertEqual(list(flow["w"]), [57.5])
        np.testing.assert_array_equal(flow["by"], np.array([[10.5, 44.0, 3.0]]))

    def test_files_written(self):
        out = self.tmp / "extracts"
        rc = cutflow.main([str(self.rvr), "-o", str(out)])
        self.assertEqual(rc, 0)
        with np.load(out / "t0_cutflow.data.npz") as z:
            self.assertEqual(list(z["steps"]), ["no cuts", "MET ≥ 100"])
            # little-endian on ANY host — SCHEMAS.md common policy
            self.assertEqual(z["raw"].dtype.str, "<f8")
            self.assertEqual(z["by"].dtype.str, "<f8")
            self.assertTrue(z["steps"].dtype.str.startswith("<U"))
            self.assertTrue(z["sources"].dtype.str.startswith("<U"))
            self.assertEqual(list(z["raw"]), [8.0, 6.0])
            self.assertEqual(z["by"].shape, (2, 3))
            self.assertEqual(list(z["sources"]), ["sig", "bkg", "obs"])
        meta = json.loads((out / "t0_cutflow.meta.json").read_text())
        self.assertEqual(meta["kind"], "cutflow")
        self.assertEqual(meta["version"], 1)
        self.assertEqual(meta["active"], ["MET"])
        # the golden cart styles nothing: every source's look resolves to null
        self.assertEqual(meta["look"], {"sig": None, "bkg": None, "obs": None})
        # ladder polarity in cuts.txt: NaN fails a range step
        cuts_txt = (out / "t0_cutflow.cuts.txt").read_text()
        self.assertIn("ladder semantics", cuts_txt)
        self.assertIn("(col['MET'] >= 100.0) & (col['MET'] <= float('inf'))",
                      cuts_txt)

    def test_default_out_dir_name(self):
        # ./extracts/<cart-stem>/ under the cwd — run from tmp to keep it there
        import os
        cwd = os.getcwd()
        os.chdir(self.tmp)
        try:
            cutflow.main([str(self.rvr), "--tab", "1"])
            self.assertTrue(
                (self.tmp / "extracts/golden/t1_cutflow.data.npz").is_file())
        finally:
            os.chdir(cwd)

    def test_unset_lumi_refused(self):
        meta, buf = rvr6.read(self.rvr)
        del meta["view"]["lumis"]
        bare = self.tmp / "bare.rvr"
        rvr6.write(bare, meta, buf)
        with self.assertRaises(ExtractError):
            cutflow.dump(bare)
        # explicit acceptance: every weighted sample falls back to lumi 1
        flow = cutflow.dump(bare, allow_unset_lumi=True)
        self.assertEqual(list(flow["w"]), [8.5, 7.0])

    def test_bad_tab_refused(self):
        with self.assertRaises(ExtractError):
            cutflow.dump(self.rvr, tab=9)


if __name__ == "__main__":
    unittest.main()
