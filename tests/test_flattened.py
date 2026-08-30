"""Composed-semantics checks for the resolver-rebased `rowver extract flattened`.

An authored v6 cartridge (groups, lumis, cuts, visibility, a region map with a
saved root AND a grid-only map) with hand-computed expected yields. Raw counts
exact; weighted sums exact here too — the numbers are small integers by design.
"""
from __future__ import annotations

import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6
from rowver.extract import ExtractError, flattened


def _golden(tmp: Path) -> Path:
    meta, buf = rvr6.pack(
        {"sig": {"MET": np.array([50.0, 150.0, 250.0]),
                 "w2c": np.array([9.0, 9.0, 9.0])},
         "tt": {"MET": np.array([120.0, 220.0]), "w2c": np.array([4.0, 4.0])},
         "wj": {"MET": np.array([130.0, 230.0]), "w2c": np.array([1.0, 1.0])},
         "obs": {"MET": np.array([110.0, 210.0, 90.0]),
                 "w2c": np.array([1.0, 1.0, 1.0])}},
        weighted={"sig": True, "tt": True, "wj": True, "obs": False},
        weights={"sig": np.array([0.5, 1.0, 2.0]),
                 "tt": np.array([1.0, 1.0]),
                 "wj": np.array([10.0, 10.0])},
        about={"name": "golden"},
        made={"built": "test", "cart": None},
    )
    meta["view"] = {
        "groups": {"of": {"tt": "bkg", "wj": "bkg"}, "sub": {"wj": "wj_sub"},
                   "order": ["bkg"]},
        "lumis": {"group": {"bkg": 2.0}, "sample": {"sig": 3.0}},
    }
    root = {"cut": {"col": "MET", "lt": 200.0}, "yes": None, "no": None}
    meta["tabs"] = [
        {"name": "main",
         "cuts": {"MET": [100.0, None]},
         "shown": ["sig", "tt", "obs"],  # wj hidden — never counted
         "stage": [{"kind": "map", "root": root}]},
        {"name": "grid",  # no cuts, everything shown, tree derived from the grid
         "stage": [{"kind": "map",
                    "grid": {"vars": [{"col": "MET", "bands": [200.0]}],
                             "exhaustive": True}}]},
        {"name": "bare",  # no root, no grid: ONE undivided region
         "stage": [{"kind": "map"}]},
        {"name": "block",  # map inside a block cell, srcs naming a subgroup
         "stage": [{"kind": "block", "layout": [3, 3],
                    "cells": [{"kind": "map", "srcs": ["wj_sub", "sig"],
                               "root": root}, None]}]},
    ]
    path = tmp / "golden.rvr"
    rvr6.write(path, meta, buf)
    return path


def nom(raw, yld, w2sum):
    return {"nominal": [float(raw), float(yld), math.sqrt(w2sum)]}


class FlattenedTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.rvr = _golden(Path(self._tmp.name))

    def tearDown(self):
        self._tmp.cleanup()

    # tab 0, saved root, cuts on by default (MET >= 100), wj hidden.
    # Kept rows (ww = w × lumi): sig 150→3, 250→6; tt 120→2, 220→2; obs 110→1, 210→1.
    def test_from_map_saved_root_cuts_and_visibility(self):
        out = flattened.dump(self.rvr, None)
        self.assertEqual(out, {
            "Bin_MET_lt_200": {"bkg": nom(1, 2, 4), "sig": nom(1, 3, 9),
                               "obs": nom(1, 1, 1)},
            "Bin_MET_ge_200": {"bkg": nom(1, 2, 4), "sig": nom(1, 6, 36),
                               "obs": nom(1, 1, 1)},
        })

    # tab 1: no cuts, no shown list — every row counts, wj included (ww=20 each).
    def test_from_map_grid_derived_tree(self):
        out = flattened.dump(self.rvr, None, tab=1)
        self.assertEqual(out, {
            "Bin_MET_lt_200": {"bkg": nom(2, 22, 404), "sig": nom(2, 4.5, 11.25),
                               "obs": nom(2, 2, 2)},
            "Bin_MET_ge_200": {"bkg": nom(2, 22, 404), "sig": nom(1, 6, 36),
                               "obs": nom(1, 1, 1)},
        })

    def test_no_cuts_keeps_visibility(self):
        out = flattened.dump(self.rvr, None, no_cuts=True)
        # wj still hidden; sig's MET=50 row (ww=1.5) is back
        self.assertEqual(out["Bin_MET_lt_200"]["sig"], nom(2, 4.5, 11.25))
        self.assertEqual(out["Bin_MET_lt_200"]["bkg"], nom(1, 2, 4))

    # tab 2: no cuts, all shown — everything lands in the one undivided "Bin"
    def test_from_map_undivided_region(self):
        out = flattened.dump(self.rvr, None, tab=2)
        self.assertEqual(out, {"Bin": {
            "bkg": nom(4, 44, 808), "sig": nom(3, 10.5, 47.25),
            "obs": nom(3, 3, 3)}})

    # tab 3: block-contained map whose srcs name a SUBGROUP and a sample
    def test_from_map_block_cell_and_subgroup_srcs(self):
        out = flattened.dump(self.rvr, None, tab=3)
        self.assertEqual(out, {
            "Bin_MET_lt_200": {"wj_sub": nom(1, 20, 400),
                               "sig": nom(2, 4.5, 11.25)},
            "Bin_MET_ge_200": {"wj_sub": nom(1, 20, 400),
                               "sig": nom(1, 6, 36)},
        })

    def test_bins_json_names_pass_through_verbatim(self):
        out = flattened.dump(self.rvr, {"": [], "Bin": [], "Bin_2": []})
        self.assertEqual(set(out), {"", "Bin", "Bin_2"})
        self.assertEqual(out[""], out["Bin"])  # same (empty) cuts, no renaming

    def test_out_of_range_tab_refused(self):
        for bad in (-1, 7):
            with self.assertRaises(ExtractError):
                flattened.dump(self.rvr, {"A": []}, tab=bad)

    def test_bins_json_path_same_semantics(self):
        out = flattened.dump(self.rvr, {"BinA": [["MET", ">=", 200.0]]})
        self.assertEqual(out, {"BinA": {"bkg": nom(1, 2, 4), "sig": nom(1, 6, 36),
                                        "obs": nom(1, 1, 1)}})

    def test_w2_column_scales_by_lumi_squared(self):
        out = flattened.dump(self.rvr, {"BinA": [["MET", ">=", 200.0]]}, w2="w2c")
        # sig: w2c 9 × lumi² 9 = 81; bkg tt: 4 × 4 = 16; obs: 1 × 1
        self.assertEqual(out["BinA"]["sig"], nom(1, 6, 81))
        self.assertEqual(out["BinA"]["bkg"], nom(1, 2, 16))
        self.assertEqual(out["BinA"]["obs"], nom(1, 1, 1))

    def test_w2_with_from_map_refused(self):
        with self.assertRaises(ExtractError):
            flattened.dump(self.rvr, None, w2="w2c")

    def test_data_only_cartridge(self):
        meta, buf = rvr6.pack(
            {"obs": {"x": np.array([1.0, 2.0, 3.0])}},
            weighted={"obs": False},
            about={"name": "dataonly"}, made={"built": "test", "cart": None})
        path = Path(self._tmp.name) / "dataonly.rvr"
        rvr6.write(path, meta, buf)
        out = flattened.dump(path, {"All": []})
        # every row counts 1; yield = raw; error = sqrt(n) — regionFlat arithmetic
        self.assertEqual(out, {"All": {"obs": nom(3, 3, 3)}})

    def test_cli_writes_cuts_txt(self):
        out = Path(self._tmp.name) / "flat.json"
        rc = flattened.main([str(self.rvr), "--from-map", "-o", str(out)])
        self.assertEqual(rc, 0)
        self.assertEqual(json.loads(out.read_text()),
                         flattened.dump(self.rvr, None))
        # the cuts file is VALID PYTHON: exec against {np, col, keep} must
        # reproduce the mask actually applied (SCHEMAS.md's contract)
        cuts = out.with_suffix(".cuts.txt").read_text()
        met = np.array([50.0, 150.0, 250.0])
        scope = {"np": np, "col": {"MET": met}, "keep": np.ones(3, bool)}
        exec(compile(cuts, "cuts.txt", "exec"), scope)
        np.testing.assert_array_equal(scope["keep"], met >= 100.0)
        rc = flattened.main([str(self.rvr), "--from-map", "--no-cuts",
                             "--tab", "1", "-o", str(out)])
        self.assertEqual(rc, 0)
        self.assertEqual(out.with_suffix(".cuts.txt").read_text(), "# no cuts\n")

    def test_locked_view_shadows_tab_state(self):
        meta, buf = rvr6.pack(
            {"a": {"x": np.array([0.0, 1.0, 2.0])}, "b": {"x": np.array([1.5])}},
            weighted={"a": False, "b": False},
            about={"name": "locked"}, made={"built": "test", "cart": None})
        meta["view"] = {  # LOCKED: cuts, shown, AND stage
            "cuts": {"x": [1.0, None]}, "shown": ["a"],
            "stage": [{"kind": "map",
                       "root": {"cut": {"col": "x", "lt": 1.0},
                                "yes": None, "no": None}}]}
        meta["tabs"] = [{"name": "t", "cuts": {"x": [None, 0.5]},
                         "shown": ["a", "b"],
                         "stage": [{"kind": "map",
                                    "root": {"cut": {"col": "x", "lt": 0.5},
                                             "yes": None, "no": None}}]}]
        path = Path(self._tmp.name) / "locked.rvr"
        rvr6.write(path, meta, buf)
        out = flattened.dump(path, {"All": []})
        # locked cuts (x >= 1) and locked shown (only a) win over the tab's
        self.assertEqual(out, {"All": {"a": nom(2, 2, 2), "b": nom(0, 0, 0)}})
        # locked stage shadows the tab's map: leaf names come from x < 1.0,
        # not the tab's x < 0.5
        out = flattened.dump(path, None)
        self.assertEqual(out, {"Bin_x_lt_1": {"a": nom(0, 0, 0),
                                              "b": nom(0, 0, 0)},
                               "Bin_x_ge_1": {"a": nom(2, 2, 2),
                                              "b": nom(0, 0, 0)}})

    def test_invalid_w2_column_refused(self):
        # negative sum, and the NaN route: a NON-NEGATIVE w² column of [0]
        # with lumi 1e308 derives 0 × inf = NaN — both must refuse, naming
        # the column, bin, and source
        meta, buf = rvr6.pack(
            {"s": {"x": np.array([1.0]), "bad": np.array([-4.0]),
                   "zero": np.array([0.0])}},
            weighted={"s": True}, weights={"s": np.array([1.0])},
            about={"name": "badw2"}, made={"built": "test", "cart": None})
        meta["view"] = {"lumis": {"sample": {"s": 1e308}}}
        path = Path(self._tmp.name) / "badw2.rvr"
        rvr6.write(path, meta, buf)
        for col in ("bad", "zero"):
            with self.assertRaises(ExtractError) as ctx:
                flattened.dump(path, {"All": []}, w2=col)
            for needle in (col, "All", "'s'"):
                self.assertIn(needle, str(ctx.exception))

    def test_overflow_cancelled_yield_flattens_to_zero_on_both_paths(self):
        # finite weights × finite lumi overflow to ±inf; the sequential sum
        # is inf - inf = NaN, which regionFlat coerces to 0 — the bins.json
        # path must agree, not emit NaN into the JSON
        meta, buf = rvr6.pack(
            {"s": {"x": np.array([1.0, 2.0])}},
            weighted={"s": True}, weights={"s": np.array([1e308, -1e308])},
            about={"name": "ovf"}, made={"built": "test", "cart": None})
        meta["view"] = {"lumis": {"sample": {"s": 2.0}}}
        meta["tabs"] = [{"name": "t", "stage": [{"kind": "map"}]}]
        path = Path(self._tmp.name) / "ovf.rvr"
        rvr6.write(path, meta, buf)
        by_bins = flattened.dump(path, {"Bin": []})
        by_map = flattened.dump(path, None)
        self.assertEqual(by_bins["Bin"], by_map["Bin"])
        self.assertEqual(by_bins["Bin"]["s"]["nominal"][:2], [2.0, 0.0])
        self.assertEqual(by_bins["Bin"]["s"]["nominal"][2], math.inf)

    def test_checked_in_golden_matches_the_generator(self):
        # tests/golden.rvr is the checked-in wire artifact of _golden();
        # header and buffer must stay identical, and its extraction must
        # reproduce the pinned tab-0 numbers — drift in either direction fails
        checked_in = Path(__file__).resolve().parent / "golden.rvr"
        a, abuf = rvr6.read(checked_in)
        b, bbuf = rvr6.read(self.rvr)
        self.assertEqual(abuf, bbuf)
        self.assertEqual({k: a[k] for k in ("data", "view", "tabs")},
                         {k: b[k] for k in ("data", "view", "tabs")})
        self.assertEqual(flattened.dump(checked_in, None),
                         flattened.dump(self.rvr, None))

    def test_exactly_one_bins_source(self):
        self.assertEqual(flattened.main([str(self.rvr)]), 1)


if __name__ == "__main__":
    unittest.main()
