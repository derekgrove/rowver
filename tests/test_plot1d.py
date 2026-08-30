"""`rowver extract plot1d`: fixture-pinned frame semantics + golden end-to-end.

The `plot1d` and `auto_window` blocks of contract/core/cases.json are
asserted here and by rowver_web's plot1d.test.ts against ui/frame.ts
(frameFor / autoWindow) — window resolution (ints columns, AUTO zoom
quantiles from the packed p80..p99 ladder, MANUAL parseFloat, the invert
guard, the nb clamp), per-source assembly, shape normalization,
bin-spacing coupling.
"""
from __future__ import annotations

import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6
from rowver.extract import ExtractError, core, plot1d

FIXTURE = Path(__file__).resolve().parent.parent / "contract/core/cases.json"
CASES = json.loads(FIXTURE.read_text(encoding="utf-8"))

STR = {"nan": math.nan, "inf": math.inf, "-inf": -math.inf}


def dec(vals):
    return np.array([math.nan if v is None else STR[v]
                     if isinstance(v, str) else v for v in vals], float)


class AutoWindowFixtureTests(unittest.TestCase):
    def test_auto_window_cases(self):
        for c in CASES["auto_window"]:
            got = core.auto_window(c["col"], c["pref"])
            self.assertEqual(got, c["expect"], c["id"])


class FrameFixtureTests(unittest.TestCase):
    def test_plot1d_cases(self):
        for c in CASES["plot1d"]:
            samples = c["samples"]
            col = c["cols"][0]
            values = dec(c["values"][col["name"]])
            shown = {s: (s in c["shown_on"] if c["shown_on"] is not None
                         else True) for s in samples}
            ds = np.array(c["ds"], np.int32)
            view = {"groups": {"of": c["groups"]}}
            ww = dec(c["ww"]) if c["weighted"] else np.ones(ds.size)
            got = plot1d.frame(c["spec"], col, values, view, samples, shown,
                               ds, np.ones(ds.size, bool), ww, c["zoom"])
            x = c["expect"]
            for key in ("lo", "hi", "nb", "ints", "log"):
                self.assertEqual(got[key], x[key], f"{c['id']} {key}")
            self.assertEqual([b["name"] for b in got["bins"]], x["sources"],
                             c["id"])
            for key in ("h", "h2", "n"):
                if key in x:
                    np.testing.assert_array_equal(
                        np.stack([b[key] for b in got["bins"]]),
                        np.array([dec(row) for row in x[key]]),
                        err_msg=f"{c['id']} {key}")
            if "centres" in x:
                np.testing.assert_array_equal(got["centres"], x["centres"],
                                              err_msg=c["id"])


def _cart(tmp: Path) -> Path:
    """The flattened/cutflow golden data, staged with 1d plots."""
    meta, buf = rvr6.pack(
        {"sig": {"MET": np.array([50.0, 150.0, 250.0])},
         "tt": {"MET": np.array([120.0, 220.0])},
         "wj": {"MET": np.array([130.0, 230.0])},
         "obs": {"MET": np.array([110.0, 210.0, 90.0])}},
        weighted={"sig": True, "tt": True, "wj": True, "obs": False},
        weights={"sig": np.array([0.5, 1.0, 2.0]),
                 "tt": np.array([1.0, 1.0]),
                 "wj": np.array([10.0, 10.0])},
        about={"name": "golden"},
        made={"built": "test", "cart": None},
    )
    meta["view"] = {
        "groups": {"of": {"tt": "bkg", "wj": "bkg"}, "order": ["bkg"]},
        "lumis": {"group": {"bkg": 2.0}, "sample": {"sig": 3.0}},
    }
    meta["tabs"] = [
        {"name": "main",
         "cuts": {"MET": [100.0, None]},
         "shown": ["sig", "tt", "obs"],  # wj hidden — never counted
         "stage": [{"kind": "1d", "varx": "MET", "nb": 4, "xauto": False,
                    "xmin": "100", "xmax": "300", "err": "sumw2"},
                   {"kind": "block", "layout": [3, 3],
                    "cells": [{"kind": "1d", "varx": "MET", "nb": 2,
                               "srcs": ["sig"]}, None]}]},
        {"name": "open",  # no cuts, everything shown
         "stage": [{"kind": "1d", "varx": "MET"}]},
    ]
    path = tmp / "golden.rvr"
    rvr6.write(path, meta, buf)
    return path


class GoldenTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.rvr = _cart(self.tmp)

    def tearDown(self):
        self._tmp.cleanup()

    # tab 0 plot 0: MANUAL [100, 300], nb 4, cuts MET >= 100, wj hidden.
    # Kept ww: sig 150→3, 250→6; tt 120→2, 220→2; obs 110→1, 210→1.
    # Frame order: groups first — [bkg, sig, obs].
    def test_tab0_manual_window(self):
        plots = plot1d.dump(self.rvr)
        self.assertEqual([p["name"] for p in plots],
                         ["t0_s0_1d_MET", "t0_s1_c0_1d_MET"])
        f = plots[0]
        self.assertEqual((f["lo"], f["hi"], f["nb"]), (100.0, 300.0, 4))
        self.assertEqual(f["window"], "manual")
        self.assertEqual(f["sources"], ["bkg", "sig", "obs"])
        np.testing.assert_array_equal(
            np.stack([b["h"] for b in f["bins"]]),
            [[2.0, 0.0, 2.0, 0.0], [0.0, 3.0, 0.0, 6.0],
             [1.0, 0.0, 1.0, 0.0]])
        np.testing.assert_array_equal(
            np.stack([b["n"] for b in f["bins"]]),
            [[1.0, 0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 1.0],
             [1.0, 0.0, 1.0, 0.0]])
        # sumw2 bands: down = up = sqrt(h2), bin by bin
        np.testing.assert_allclose(
            f["err"][0], np.sqrt(np.stack([b["h2"] for b in f["bins"]])))
        np.testing.assert_array_equal(f["err"][0], f["err"][1])
        # no tails: the cut removed everything below 100
        self.assertEqual([b["under"] for b in f["bins"]], [0, 0, 0])
        self.assertEqual([b["over"] for b in f["bins"]], [0, 0, 0])

    def test_tab0_cell_srcs(self):
        f = plot1d.dump(self.rvr)[1]
        self.assertEqual(f["sources"], ["sig"])
        self.assertEqual(f["spec"]["sheet"], 0)
        self.assertEqual(f["spec"]["quad"], 0)
        self.assertIsNone(f["err"])  # err defaults off — bands absent

    # tab 1, --zoom p90: the packed whole-cart quantile frames the window.
    # MET sorted: 50,90,110,120,130,150,210,220,230,250 → p90 = 232 (numpy
    # linear interpolation, packed at build time).
    def test_tab1_zoom_p90_packed(self):
        f = plot1d.dump(self.rvr, tab=1, zoom="p90")[0]
        meta, _ = rvr6.read(self.rvr)
        col = meta["data"]["cols"][0]
        self.assertEqual(f["window"], "auto")
        self.assertEqual(f["lo"], col["nice"][0])
        self.assertEqual(f["hi"], core.to_precision(col["p90"], 4))
        self.assertEqual(f["hi"], 232.0)
        self.assertEqual(f["nb"], 40)  # the default

    def test_tab1_zoom_full(self):
        f = plot1d.dump(self.rvr, tab=1, zoom="full")[0]
        meta, _ = rvr6.read(self.rvr)
        col = meta["data"]["cols"][0]
        self.assertEqual(f["window"], "lim")
        self.assertEqual([f["lo"], f["hi"]], [float(v) for v in col["nice"]])

    def test_no_cuts(self):
        f = plot1d.dump(self.rvr, no_cuts=True)[0]
        # sig's MET=50 row (ww 1.5) is back, crammed into bin 0; wj stays hidden
        np.testing.assert_array_equal(
            np.stack([b["h"] for b in f["bins"]])[1], [1.5, 3.0, 0.0, 6.0])
        self.assertEqual(f["active"], [])

    def test_files_written(self):
        out = self.tmp / "extracts"
        rc = plot1d.main([str(self.rvr), "-o", str(out)])
        self.assertEqual(rc, 0)
        with np.load(out / "t0_s0_1d_MET.data.npz") as z:
            for key in ("edges", "centres", "h", "h2", "n", "under", "over",
                        "underW", "overW", "err_down", "err_up"):
                self.assertEqual(z[key].dtype.str, "<f8", key)
            self.assertTrue(z["sources"].dtype.str.startswith("<U"))
            self.assertEqual(list(z["sources"]), ["bkg", "sig", "obs"])
            self.assertEqual(z["h"].shape, (3, 4))
            self.assertEqual(list(z["edges"]),
                             [100.0, 150.0, 200.0, 250.0, 300.0])
        meta = json.loads((out / "t0_s0_1d_MET.meta.json").read_text())
        self.assertEqual(meta["kind"], "plot1d")
        self.assertEqual(meta["version"], 2)
        self.assertEqual(meta["window"],
                         {"lo": 100.0, "hi": 300.0, "nb": 4, "ints": False,
                          "log": False, "log_active": False,
                          "zoom": "p90", "from": "manual"})
        self.assertEqual(meta["cuts"], {"MET": {"range": [100.0, "inf"]}})
        self.assertEqual(meta["spec"]["err"], "sumw2")
        # the contract's "bin edges actually used" live in meta.json too
        self.assertEqual(meta["edges"], [100.0, 150.0, 200.0, 250.0, 300.0])
        self.assertEqual(len(meta["centres"]), 4)
        # err off on the cell plot: the band keys must be ABSENT, not empty
        with np.load(out / "t0_s1_c0_1d_MET.data.npz") as z:
            self.assertNotIn("err_down", z)
        cuts_txt = (out / "t0_s0_1d_MET.cuts.txt").read_text()
        self.assertIn("viewer semantics", cuts_txt)  # mask polarity, NaN passes

    def test_no_1d_plots_refused(self):
        meta, buf = rvr6.read(self.rvr)
        meta["tabs"][0]["stage"] = [{"kind": "map"}]
        bare = self.tmp / "bare.rvr"
        rvr6.write(bare, meta, buf)
        with self.assertRaises(ExtractError):
            plot1d.dump(bare)

    def test_unset_lumi_refused(self):
        meta, buf = rvr6.read(self.rvr)
        del meta["view"]["lumis"]
        bare = self.tmp / "bare.rvr"
        rvr6.write(bare, meta, buf)
        with self.assertRaises(ExtractError):
            plot1d.dump(bare)
        plot1d.dump(bare, allow_unset_lumi=True)  # explicit acceptance


class FrameSemanticsTests(unittest.TestCase):
    """Python-only pins for behavior frameFor does not expose (the bands are
    computed at draw time by the backends, so no web twin exists for them)."""

    COL = {"name": "x", "lim": [0.0, 2.0], "nice": [0.0, 2.0]}

    def _frame(self, spec, values, ww):
        return plot1d.frame(spec, self.COL, np.array(values, float), {},
                            ["m"], {"m": True}, np.zeros(len(values), np.int32),
                            np.ones(len(values), bool), np.array(ww, float),
                            "full")

    def test_container_err_falls_through_to_symmetric(self):
        # err: [] is truthy in JS and !== 'off'; String([]) is '' and
        # errFor's fallthrough is the sumw2 symmetric band — PRESENT
        f = self._frame({"varx": "x", "nb": 2, "err": []}, [0.5, 1.5], [2, 3])
        np.testing.assert_array_equal(f["err"][0], [[2.0, 3.0]])
        np.testing.assert_array_equal(f["err"][0], f["err"][1])

    def test_container_err_stringifies_to_a_real_mode(self):
        # the backends call errFor(String(err)): ["sqrtn"] IS sqrtn.
        # Unequal weights in one bin so sqrtn (|Σw/n|·√n = 2√2) cannot be
        # confused with the sumw2 fallback (√Σw² = √10)
        f = self._frame({"varx": "x", "nb": 2, "err": ["sqrtn"]},
                        [0.5, 0.5], [1, 3])
        np.testing.assert_allclose(f["err"][0], [[2 * math.sqrt(2), 0.0]])
        # ["off"] stringifies to 'off' AFTER the gate: null bands drawn as 0
        f = self._frame({"varx": "x", "nb": 2, "err": ["off"]}, [0.5], [4])
        np.testing.assert_array_equal(f["err"][0], [[0.0, 0.0]])
        np.testing.assert_array_equal(f["err"][1], [[0.0, 0.0]])

    def test_err_off_absent(self):
        f = self._frame({"varx": "x", "nb": 2, "err": "off"}, [0.5], [1])
        self.assertIsNone(f["err"])

    def test_shape_scales_before_err(self):
        # the band rides the DRAWN (shape-scaled) moments: h [4]→[1],
        # h2 [16]→[1], so sumw2 gives 1, not 4
        f = self._frame({"varx": "x", "nb": 2, "norm": "shape",
                         "err": "sumw2"}, [0.5], [4])
        np.testing.assert_array_equal(f["bins"][0]["h"], [1.0, 0.0])
        np.testing.assert_array_equal(f["err"][0], [[1.0, 0.0]])


class CoercionTests(unittest.TestCase):
    # parseFloat(String(v)) / Number(v) over what a JSON wire can carry
    def test_parse_float(self):
        for v, want in [("10", 10.0), (" 1e2px", 100.0), ("abc", math.nan),
                        ("", math.nan), (".5x", 0.5), ("-Infinity", -math.inf),
                        (None, math.nan), (True, math.nan), (12.5, 12.5),
                        ([15, 2], 15.0), ({}, math.nan),
                        ("١٢px", math.nan), ("१२", math.nan),
                        ("１２", math.nan)]:
            got = core.parse_float(v)
            self.assertTrue(got == want or (got != got and want != want),
                            f"{v!r} -> {got}")

    def test_js_number(self):
        for v, want in [(None, 0.0), (True, 1.0), ("", 0.0), (" 40 ", 40.0),
                        ("1_0", math.nan), ("abc", math.nan), ([5], 5.0),
                        ([], 0.0), ([1, 2], math.nan), ({}, math.nan),
                        ("0x10", 16.0), ("0o7", 7.0), ("0b101", 5.0),
                        ("inf", math.nan), ("infinity", math.nan),
                        ("Infinity", math.inf), ("-Infinity", -math.inf),
                        ("﻿42﻿", 42.0), ("\x8542", math.nan),
                        ("++Infinity", math.nan), ("--Infinity", math.nan),
                        ("+-Infinity", math.nan),
                        ("١٢", math.nan), ("१२", math.nan),
                        ("１２", math.nan),
                        ("0x" + "f" * 1000, math.inf)]:
            got = core._js_number(v)
            self.assertTrue(got == want or (got != got and want != want),
                            f"{v!r} -> {got}")


if __name__ == "__main__":
    unittest.main()
