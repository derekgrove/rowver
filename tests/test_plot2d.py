"""`rowver extract plot2d` / `plot3d`: fixture-pinned 2D frame semantics +
golden end-to-end.

The `plot2d` block of contract/core/cases.json is asserted here and by
rowver_web's plot2d.test.ts against ui/frame.ts frameFor's grid — two-axis
window resolution (y-ints columns, `nby ?? nb`, `binspacey`/`logy`
coupling, MANUAL parseFloat and the invert guard on y, AUTO zoom on y),
the srcs/mute per-DATASET filter (okDs), and the aggregate h/kept parity
object. The per-source x_* extras have no web twin (binned2D returns no
per-source 2D) and are pinned python-side only.
"""
from __future__ import annotations

import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6
from rowver.extract import ExtractError, plot2d

FIXTURE = Path(__file__).resolve().parent.parent / "contract/core/cases.json"
CASES = json.loads(FIXTURE.read_text(encoding="utf-8"))

STR = {"nan": math.nan, "inf": math.inf, "-inf": -math.inf}


def dec(vals):
    return np.array([math.nan if v is None else STR[v]
                     if isinstance(v, str) else v for v in vals], float)


class FrameFixtureTests(unittest.TestCase):
    def test_plot2d_cases(self):
        for c in CASES["plot2d"]:
            samples = c["samples"]
            by_name = {col["name"]: col for col in c["cols"]}
            colx, coly = by_name[c["spec"]["varx"]], by_name[c["spec"]["vary"]]
            shown = {s: (s in c["shown_on"] if c["shown_on"] is not None
                         else True) for s in samples}
            ds = np.array(c["ds"], np.int32)
            view = {"groups": {"of": c["groups"]}}
            ww = dec(c["ww"]) if c["weighted"] else np.ones(ds.size)
            got = plot2d.frame(c["spec"], colx, coly,
                               dec(c["values"][colx["name"]]),
                               dec(c["values"][coly["name"]]),
                               view, samples, shown, ds,
                               np.ones(ds.size, bool), ww, c["zoom"])
            x = c["expect"]
            for key in ("lo", "hi", "nb", "ints", "log"):
                self.assertEqual(got["x"][key], x[key], f"{c['id']} {key}")
            for key, want in (("lo", "loy"), ("hi", "hiy"), ("nb", "nby"),
                              ("log", "logy")):
                self.assertEqual(got["y"][key], x[want], f"{c['id']} {want}")
            self.assertEqual([e["name"] for e in got["extras"]], x["sources"],
                             c["id"])
            self.assertEqual(got["kept"], x["kept"], c["id"])
            np.testing.assert_allclose(got["h"], dec(x["h"]), rtol=1e-8,
                                       err_msg=f"{c['id']} h")
            if "centres_y" in x:
                np.testing.assert_allclose(got["y"]["centres"], x["centres_y"],
                                           rtol=1e-8, err_msg=c["id"])
            # the extras SUM to the aggregate: same fills, split by source
            if got["extras"]:
                np.testing.assert_allclose(
                    np.sum([e["h"] for e in got["extras"]], axis=0),
                    got["h"], rtol=1e-8, err_msg=f"{c['id']} extras sum")


def _cart(tmp: Path) -> Path:
    """The plot1d golden data, staged with 2d/3d plots."""
    meta, buf = rvr6.pack(
        {"sig": {"MET": np.array([50.0, 150.0, 250.0]),
                 "HT": np.array([10.0, 20.0, 30.0])},
         "tt": {"MET": np.array([120.0, 220.0]),
                "HT": np.array([10.0, 30.0])},
         "wj": {"MET": np.array([130.0, 230.0]),
                "HT": np.array([20.0, 20.0])},
         "obs": {"MET": np.array([110.0, 210.0, 90.0]),
                 "HT": np.array([10.0, 30.0, 20.0])}},
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
         "stage": [{"kind": "2d", "varx": "MET", "vary": "HT",
                    "nb": 4, "nby": 2, "xauto": False,
                    "xmin": "100", "xmax": "300",
                    "yauto": False, "ymin": "0", "ymax": "40"},
                   {"kind": "block", "layout": [3, 3],
                    "cells": [{"kind": "2d", "varx": "MET", "vary": "HT",
                               "nb": 2, "srcs": ["sig"]}, None]},
                   # spec-identical to the s0 2d entry: only kind differs
                   {"kind": "3d", "varx": "MET", "vary": "HT",
                    "nb": 4, "nby": 2, "xauto": False,
                    "xmin": "100", "xmax": "300",
                    "yauto": False, "ymin": "0", "ymax": "40"}]},
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

    # tab 0 plot 0: MANUAL x [100, 300]×4, y [0, 40]×2, cuts MET >= 100,
    # wj hidden. Kept rows (ww, MET, HT): sig 150,20→3; sig 250,30→6;
    # tt 120,10→2; tt 220,30→2; obs 110,10→1; obs 210,30→1.
    # x bins of width 50 from 100; y bins [0,20),[20,40).
    def test_tab0_manual_grid(self):
        plots = plot2d.dump(self.rvr)
        self.assertEqual([p["name"] for p in plots],
                         ["t0_s0_2d_MET_HT", "t0_s1_c0_2d_MET_HT"])
        f = plots[0]
        self.assertEqual((f["x"]["lo"], f["x"]["hi"], f["x"]["nb"]),
                         (100.0, 300.0, 4))
        self.assertEqual((f["y"]["lo"], f["y"]["hi"], f["y"]["nb"]),
                         (0.0, 40.0, 2))
        self.assertEqual(f["kept"], 6)
        # y0 row: tt 120→x0 (2), obs 110→x0 (1); y1 row: sig 150→x1 (3),
        # sig 250→x3 (6), tt 220→x2 (2), obs 210→x2 (1)
        np.testing.assert_allclose(
            np.asarray(f["h"]).reshape(2, 4),
            [[3.0, 0.0, 0.0, 0.0], [0.0, 3.0, 3.0, 6.0]])
        self.assertEqual(f["sources"], ["bkg", "sig", "obs"])

    def test_tab0_cell_srcs_okds(self):
        f = plot2d.dump(self.rvr)[1]
        self.assertEqual(f["sources"], ["sig"])
        # srcs narrows the GRID too, not just the extras: only sig rows count
        self.assertEqual(f["kept"], 2)
        self.assertEqual(f["spec"]["sheet"], 0)
        self.assertEqual(f["spec"]["quad"], 0)

    def test_3d_same_grid(self):
        # the s2 3d entry is spec-identical to the s0 2d entry: the whole
        # record must agree — axes, grid, extras — with only kind differing
        f2 = plot2d.dump(self.rvr)[0]
        f3 = plot2d.dump(self.rvr, kind="3d")[0]
        self.assertEqual(f3["name"], "t0_s2_3d_MET_HT")
        for ax in ("x", "y"):
            for k in ("lo", "hi", "nb", "ints", "log", "log_active"):
                self.assertEqual(f3[ax][k], f2[ax][k], f"{ax} {k}")
        self.assertEqual(f3["kept"], f2["kept"])
        np.testing.assert_array_equal(f3["h"], f2["h"])
        self.assertEqual([e["name"] for e in f3["extras"]],
                         [e["name"] for e in f2["extras"]])
        for e3, e2 in zip(f3["extras"], f2["extras"]):
            for k in ("h", "h2", "n"):
                np.testing.assert_array_equal(e3[k], e2[k], err_msg=k)

    def test_label_stamped_with_exp_preference(self):
        # no view.label in the cart: exp follows --exp (the machine-local
        # preference), tag the page default; "none" is the NONE toggle
        f = plot2d.dump(self.rvr)[0]
        self.assertEqual(f["label"], {"exp": "CMS",
                                      "tag": "Work in progress",
                                      "right": ""})
        f = plot2d.dump(self.rvr, exp="ATLAS")[0]
        self.assertEqual(f["label"]["exp"], "ATLAS")
        f = plot2d.dump(self.rvr, exp="none")[0]
        self.assertEqual(f["label"]["exp"], "")

    def test_files_written(self):
        out = self.tmp / "extracts"
        rc = plot2d.main([str(self.rvr), "-o", str(out)])
        self.assertEqual(rc, 0)
        with np.load(out / "t0_s0_2d_MET_HT.data.npz") as z:
            # the EXACT key set — nothing extra, nothing missing
            self.assertEqual(set(z.files), {
                "h", "kept", "edges_x", "edges_y", "centres_x", "centres_y",
                "sources"} | {f"x_{m}_{s}" for m in ("h", "h2", "n")
                              for s in ("bkg", "sig", "obs")})
            for key in z.files:
                if key == "kept":
                    self.assertEqual(z[key].dtype.str, "<i8")
                elif key == "sources":
                    self.assertTrue(z[key].dtype.str.startswith("<U"))
                else:
                    self.assertEqual(z[key].dtype.str, "<f8", key)
            self.assertEqual(int(z["kept"]), 6)
            self.assertEqual(list(z["sources"]), ["bkg", "sig", "obs"])
            self.assertEqual(z["h"].shape, (2, 4))
            self.assertEqual(list(z["edges_x"]),
                             [100.0, 150.0, 200.0, 250.0, 300.0])
            self.assertEqual(list(z["edges_y"]), [0.0, 20.0, 40.0])
            # hand-computed per-source moment grids, cell by cell.
            # Kept rows (ww, MET, HT): tt 120,10→2; tt 220,30→2;
            # sig 150,20→3; sig 250,30→6; obs 110,10→1; obs 210,30→1
            np.testing.assert_allclose(
                z["x_h_bkg"], [[2.0, 0, 0, 0], [0, 0, 2.0, 0]])
            np.testing.assert_allclose(
                z["x_h2_bkg"], [[4.0, 0, 0, 0], [0, 0, 4.0, 0]])
            np.testing.assert_allclose(
                z["x_n_bkg"], [[1.0, 0, 0, 0], [0, 0, 1.0, 0]])
            np.testing.assert_allclose(
                z["x_h_sig"], [[0, 0, 0, 0], [0, 3.0, 0, 6.0]])
            np.testing.assert_allclose(
                z["x_h2_sig"], [[0, 0, 0, 0], [0, 9.0, 0, 36.0]])
            np.testing.assert_allclose(
                z["x_n_sig"], [[0, 0, 0, 0], [0, 1.0, 0, 1.0]])
            for key in ("x_h_obs", "x_h2_obs", "x_n_obs"):  # unweighted 1s
                np.testing.assert_allclose(
                    z[key], [[1.0, 0, 0, 0], [0, 0, 1.0, 0]], err_msg=key)
            # extras sum to the aggregate
            np.testing.assert_allclose(
                z["x_h_bkg"] + z["x_h_sig"] + z["x_h_obs"], z["h"])
        meta = json.loads((out / "t0_s0_2d_MET_HT.meta.json").read_text())
        self.assertEqual(meta["kind"], "plot2d")
        self.assertEqual(meta["version"], 1)
        self.assertEqual(meta["window"]["x"],
                         {"lo": 100.0, "hi": 300.0, "nb": 4, "ints": False,
                          "log": False, "log_active": False,
                          "zoom": "p90", "from": "manual"})
        self.assertEqual(meta["window"]["y"]["nb"], 2)
        self.assertEqual(meta["window"]["y"]["from"], "manual")
        self.assertEqual(meta["kept"], 6)
        self.assertEqual(meta["extras"],
                         {"bkg": "bkg", "sig": "sig", "obs": "obs"})
        self.assertEqual(meta["edges"]["y"], [0.0, 20.0, 40.0])
        self.assertEqual(len(meta["centres"]["x"]), 4)
        self.assertIn("colours", meta)
        cuts_txt = (out / "t0_s0_2d_MET_HT.cuts.txt").read_text()
        self.assertIn("viewer semantics", cuts_txt)

    def test_3d_files_kind(self):
        out = self.tmp / "extracts3"
        rc = plot2d.main3d([str(self.rvr), "-o", str(out)])
        self.assertEqual(rc, 0)
        meta = json.loads((out / "t0_s2_3d_MET_HT.meta.json").read_text())
        self.assertEqual(meta["kind"], "plot3d")

    def test_no_2d_plots_refused(self):
        meta, buf = rvr6.read(self.rvr)
        meta["tabs"][0]["stage"] = [{"kind": "1d", "varx": "MET"}]
        bare = self.tmp / "bare.rvr"
        rvr6.write(bare, meta, buf)
        with self.assertRaises(ExtractError):
            plot2d.dump(bare)

    def test_unset_lumi_refused(self):
        meta, buf = rvr6.read(self.rvr)
        del meta["view"]["lumis"]
        bare = self.tmp / "bare.rvr"
        rvr6.write(bare, meta, buf)
        with self.assertRaises(ExtractError):
            plot2d.dump(bare)
        plot2d.dump(bare, allow_unset_lumi=True)  # explicit acceptance


class ExtraKeyTests(unittest.TestCase):
    def test_sanitized_collision_suffix(self):
        # "a b" and "a_b" both sanitize to a_b — suffixed in stack order
        self.assertEqual(plot2d._extra_keys(["a b", "a_b", "c"]),
                         ["a_b", "a_b_2", "c"])

    def test_natural_name_collides_with_generated_suffix(self):
        # a source NATURALLY named a_b_2 lands on the generated suffix —
        # keys must stay globally unique, never silently overwrite (codex
        # step-6 r1)
        self.assertEqual(plot2d._extra_keys(["a b", "a_b", "a_b_2"]),
                         ["a_b", "a_b_2", "a_b_2_2"])
        self.assertEqual(plot2d._extra_keys(["a_b_2", "a b", "a_b"]),
                         ["a_b_2", "a_b", "a_b_3"])


if __name__ == "__main__":
    unittest.main()
