"""`rowver extract board`: the yields board, one summed background shelf.

Twin numbers are authored on the flattened/cutflow golden data — hand
yields small enough to check on paper, exercised through the same
cuts/visibility/lumi path the page resolves.
"""
from __future__ import annotations

import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6
from rowver.extract import ExtractError, board, core

FIXTURE = Path(__file__).resolve().parent.parent / "contract/core/cases.json"
CASES = json.loads(FIXTURE.read_text(encoding="utf-8"))


def _dec(vals):
    return [math.nan if v is None else v for v in vals]


def _fixture_numbers(c: dict) -> dict:
    samples = c["samples"]
    ds = np.array(c["ds"], np.int32)
    columns = {"w": np.array(c["w"], float)}
    unw = set(c.get("unweighted_samples") or [])
    view = {"groups": {"of": c["groups"]}, "lumis": c["lumis"]}
    if c["syst"] is not None:
        view["syst"] = c["syst"]
    meta = {"data": {"weight_col": "w",
                     "samples": [{"name": s, "weighted": s not in unw}
                                 for s in samples]},
            "view": view}
    shown = {s: (s in c["shown_on"]) for s in samples}
    keep = core.sel_mask(columns, [], {}, shown, ds, samples)
    return board.board_numbers(meta, columns, ds, keep, c["spec"])


def _cart(tmp: Path, board_spec: dict | None = None,
          map_spec: dict | None = None) -> Path:
    """The golden data with a board and a region map staged.

    Kept rows under MET >= 100 (wj hidden): sig 150→ww 3, 250→6;
    tt 120→2, 220→2; obs 110→1, 210→1.
    """
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
    stage = [board_spec or {"kind": "board", "rows": 2, "cols": 2,
                            "cells": ["sig", None, "bkg", None],
                            "bkgs": ["bkg"], "err": "sumw2"},
             map_spec or {"kind": "map",
                          "root": {"cut": {"col": "MET", "lt": 200.0},
                                   "yes": None, "no": None},
                          "sig": "sig", "bkg": ["bkg"]}]
    meta["tabs"] = [{"name": "main",
                     "cuts": {"MET": [100.0, None]},
                     "shown": ["sig", "tt", "obs"],  # wj hidden
                     "stage": stage}]
    path = tmp / "golden.rvr"
    rvr6.write(path, meta, buf)
    return path


class BoardFixtureTests(unittest.TestCase):
    """The shared board block (contract/core/cases.json) — twin of
    rowver_web's board.test.ts. Raw exact; weighted 1e-8 rtol."""

    def assertClose(self, got, want, msg=""):
        np.testing.assert_allclose(np.asarray(got, float),
                                   np.asarray(want, float), rtol=1e-8, atol=0,
                                   equal_nan=True, err_msg=msg)

    def test_board_cases(self):
        for c in CASES["board"]:
            num = _fixture_numbers(c)
            e = c["expect"]
            sq = num["squares"]
            self.assertEqual(num["sources"], e["sources"], c["id"])
            self.assertEqual(num["mets"], e["mets"], c["id"])
            self.assertEqual(num["syst"], e["syst_used"], c["id"])
            self.assertEqual([q["src"] for q in sq], e["cell_sources"], c["id"])
            for key in ("raw", "yield", "w2"):
                attr = {"raw": "n", "yield": "w", "w2": "w2"}[key]
                self.assertClose([q[attr] for q in sq], _dec(e[key]),
                                 f"{c['id']} {key}")
            if "err_down" in e:
                self.assertClose([q["err"][0] if q["err"] else math.nan
                                  for q in sq],
                                 _dec(e["err_down"]), f"{c['id']} err_down")
                self.assertClose([q["err"][1] if q["err"] else math.nan
                                  for q in sq],
                                 _dec(e["err_up"]), f"{c['id']} err_up")
            else:
                self.assertEqual(num["mode"], "off", c["id"])
            for met in e["mets"]:
                self.assertClose([q[met] for q in sq], _dec(e[met]),
                                 f"{c['id']} {met}")
            self.assertClose([q["w"] for q in num["shelf"]],
                             _dec(e["bkg_w"]), f"{c['id']} shelf")
            self.assertClose([num["bkg"]["w"], num["bkg"]["w2"]],
                             [e["bkg_total"]["w"], e["bkg_total"]["w2"]],
                             f"{c['id']} pooled")
            self.assertEqual(num["bkg"]["n"], e["bkg_total"]["raw"], c["id"])
            self.assertEqual(num["fell_back"], e["fell_back"], c["id"])
            self.assertEqual([list(p) for p in num["overlap"]], e["overlap"],
                             c["id"])


class BoardTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def test_squares_read_against_the_summed_shelf(self):
        num = board.dump(_cart(self.tmp))
        self.assertEqual(num["name"], "t0_s0_board")
        self.assertEqual(num["sources"], ["sig", "bkg"])
        sq = num["squares"]
        self.assertEqual([q["src"] for q in sq], ["sig", "", "bkg", ""])
        # sig square: ww 3 + 6 (lumi 3 × weights 1, 2); bkg: 2 + 2
        self.assertEqual((sq[0]["w"], sq[0]["w2"], sq[0]["n"]), (9.0, 45.0, 2))
        self.assertEqual((sq[2]["w"], sq[2]["w2"], sq[2]["n"]), (4.0, 8.0, 2))
        # empty squares are the viewer's zero(): 0 in memory, NaN metrics
        self.assertTrue(all(sq[k]["w"] == 0 and math.isnan(sq[k]["sob"])
                            for k in (1, 3)))
        # every square's B is the shelf sum
        self.assertEqual(num["bkg"], {"w": 4.0, "w2": 8.0, "n": 2})
        self.assertEqual(sq[0]["sob"], 9.0 / 4.0)
        self.assertAlmostEqual(sq[0]["sig"], 9.0 / math.sqrt(4.0 + 8.0))
        self.assertAlmostEqual(sq[0]["err"][0], math.sqrt(45.0))
        self.assertIsNone(sq[1]["err"])
        self.assertEqual(sq[0]["zbi"],
                         core.zbi(9.0, 4.0, 8.0, 0.05, True))  # default syst
        self.assertEqual(num["mets"], ["sob", "sig", "zbi"])  # absent = all
        self.assertEqual(num["fell_back"], 0)
        self.assertEqual(num["overlap"], [])

    def test_overlapping_shelf_reported_not_silenced(self):
        rvr = _cart(self.tmp, board_spec={
            "kind": "board", "rows": 1, "cols": 1, "cells": ["sig"],
            "bkgs": ["bkg", "tt"], "err": "sumw2"})
        num = board.dump(rvr)
        # tt is inside bkg: the pooled B double-counts it — reported
        self.assertEqual(num["overlap"], [("bkg", "tt")])
        self.assertEqual(num["bkg"], {"w": 8.0, "w2": 16.0, "n": 4})

    def test_met_subset_and_off_mode(self):
        rvr = _cart(self.tmp, board_spec={
            "kind": "board", "rows": 1, "cols": 1, "cells": ["sig"],
            "bkgs": ["bkg"], "err": "off", "met": ["zbi"]})
        num = board.dump(rvr)
        self.assertEqual(num["mets"], ["zbi"])
        self.assertEqual(num["mode"], "off")

    def test_garbled_syst_normalizes_to_five_percent(self):
        rvr = _cart(self.tmp)
        meta, buf = rvr6.read(rvr)
        meta["view"]["syst"] = "sure"  # Number('sure') is NaN — format 5%
        rvr6.write(rvr, meta, buf)
        self.assertEqual(board.dump(rvr)["syst"], 0.05)
        meta["view"]["syst"] = "0.10"
        rvr6.write(rvr, meta, buf)
        self.assertEqual(board.dump(rvr)["syst"], 0.10)

    def test_cli_writes_the_schema(self):
        rvr = _cart(self.tmp)
        out = self.tmp / "extracts"
        rc = board.main([str(rvr), "-o", str(out)])
        self.assertEqual(rc, 0)
        with np.load(out / "t0_s0_board.data.npz") as z:
            self.assertEqual(int(z["rows"]), 2)
            self.assertEqual([str(s) for s in z["cell_sources"]],
                             ["sig", "", "bkg", ""])
            np.testing.assert_array_equal(z["yield"][[0, 2]], [9.0, 4.0])
            self.assertTrue(np.isnan(z["yield"][[1, 3]]).all())
            self.assertEqual(float(z["bkg_total_w"]), 4.0)
            self.assertEqual(z["overlap"].shape, (0, 2))
            for met in ("sob", "sig", "zbi"):  # all three: met absent
                self.assertIn(met, z)
            self.assertIn("err_down", z)  # sumw2 mode writes bands
        meta = json.loads((out / "t0_s0_board.meta.json").read_text())
        self.assertEqual(meta["kind"], "board")
        self.assertEqual(meta["err"], "sumw2")
        self.assertEqual(meta["syst"], 0.05)

    def test_no_board_refused(self):
        rvr = _cart(self.tmp)
        meta, buf = rvr6.read(rvr)
        meta["tabs"][0]["stage"] = [meta["tabs"][0]["stage"][1]]  # map only
        rvr6.write(rvr, meta, buf)
        with self.assertRaises(ExtractError):
            board.dump(rvr)


if __name__ == "__main__":
    unittest.main()
