"""`rowver extract regions`: the leaf partition + Asimov/neff columns."""
from __future__ import annotations

import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6
from rowver.extract import ExtractError, core, regions

from test_board import CASES, _cart


class RegionSigFixtureTests(unittest.TestCase):
    """The shared region_sig block — twin of rowver_web's regionSig pin."""

    def test_region_sig_cases(self):
        for c in CASES["region_sig"]:
            r = core.region_sig(c["flat"], c["sig"], c["bkg"])
            e = c["expect"]
            if e["z"] == "nan":
                self.assertTrue(math.isnan(r["z"]), c["id"])
            else:
                self.assertAlmostEqual(r["z"], e["z"], places=10, msg=c["id"])
            self.assertEqual((r["bins"], r["dropped"], r["thin"]),
                             (e["bins"], e["dropped"], e["thin"]), c["id"])


class RegionsTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    # one cut MET < 200 over the kept rows (wj hidden, MET >= 100):
    # yes leaf — bkg 120→2, sig 150→3, obs 110→1
    # no leaf  — bkg 220→2, sig 250→6, obs 210→1
    def test_partition_and_grid(self):
        num = regions.dump(_cart(self.tmp))
        self.assertEqual(num["name"], "t0_s1_map")
        self.assertEqual(num["bins"], ["Bin_MET_lt_200", "Bin_MET_ge_200"])
        self.assertEqual(num["srcs"], ["bkg", "sig", "obs"])  # absent srcs
        np.testing.assert_array_equal(num["w"], [[2, 3, 1], [2, 6, 1]])
        np.testing.assert_array_equal(num["w2"], [[4, 9, 1], [4, 36, 1]])
        np.testing.assert_array_equal(num["n"], [[1, 1, 1], [1, 1, 1]])
        self.assertEqual(num["total"], 15.0)  # each kept row counted ONCE
        np.testing.assert_array_equal(num["neff"], np.ones((2, 3)))

    def test_asimov_z_counts_thin_bins(self):
        a = regions.dump(_cart(self.tmp))["asimov"]
        q0 = 2 * ((3 + 2) * math.log(1 + 3 / 2) - 3) \
            + 2 * ((6 + 2) * math.log(1 + 6 / 2) - 6)
        self.assertAlmostEqual(a["z"], math.sqrt(q0), places=12)
        # both bins contribute, and both pooled raw backgrounds are 1 < 5
        self.assertEqual((a["bins"], a["dropped"], a["thin"]), (2, 0, 2))

    def test_significance_not_ready_is_the_pages_placeholder(self):
        rvr = _cart(self.tmp, map_spec={
            "kind": "map", "root": {"cut": {"col": "MET", "lt": 200.0},
                                    "yes": None, "no": None}})  # no sig/bkg
        a = regions.dump(rvr)["asimov"]
        self.assertTrue(math.isnan(a["z"]))
        self.assertEqual((a["bins"], a["dropped"], a["thin"]), (0, 0, 0))

    def test_no_root_is_one_undivided_region(self):
        rvr = _cart(self.tmp, map_spec={
            "kind": "map", "grid": {"vars": [{"col": "MET", "bands": [200.0]}]},
            "sig": "sig", "bkg": ["bkg"]})
        num = regions.dump(rvr)
        # a grid with no root is a recipe not yet applied — ONE region
        self.assertEqual(num["bins"], ["Bin"])
        np.testing.assert_array_equal(num["w"], [[4, 9, 2]])

    def test_neff_guards(self):
        # an all-zero cell (obs feeds nothing under MET < 100 is impossible
        # here, so poison via srcs naming a source with no kept rows)
        rvr = _cart(self.tmp, map_spec={
            "kind": "map", "root": None, "srcs": ["wj"]})  # wj is hidden
        num = regions.dump(rvr)
        self.assertTrue(np.isnan(num["neff"]).all())  # 0 yield: no neff

    def test_cli_writes_the_schema(self):
        rvr = _cart(self.tmp)
        out = self.tmp / "extracts"
        rc = regions.main([str(rvr), "-o", str(out)])
        self.assertEqual(rc, 0)
        with np.load(out / "t0_s1_map.data.npz") as z:
            self.assertEqual([str(b) for b in z["bins"]],
                             ["Bin_MET_lt_200", "Bin_MET_ge_200"])
            np.testing.assert_array_equal(z["w"], [[2, 3, 1], [2, 6, 1]])
            self.assertEqual(float(z["total"]), 15.0)
            self.assertEqual(int(z["asimov_thin"]), 2)
            self.assertEqual(z["neff"].dtype.str, "<f8")
        meta = json.loads((out / "t0_s1_map.meta.json").read_text())
        self.assertEqual(meta["kind"], "regions")
        self.assertEqual(meta["asimov"]["bins"], 2)
        self.assertEqual(meta["syst"], 0.05)

    def test_no_map_refused(self):
        rvr = _cart(self.tmp)
        meta, buf = rvr6.read(rvr)
        meta["tabs"][0]["stage"] = [meta["tabs"][0]["stage"][0]]  # board only
        rvr6.write(rvr, meta, buf)
        with self.assertRaises(ExtractError):
            regions.dump(rvr)


if __name__ == "__main__":
    unittest.main()
