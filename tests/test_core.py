"""The shared fixture (contract/core/cases.json) asserted against extract.core.

The same file is asserted by rowver_web's bin/stats/resolver tests; the
sync-checker pins the two copies byte-identical. Raw counts exact; weighted
sums 1e-8 rtol — never bit parity.
"""
from __future__ import annotations

import json
import math
import unittest
from pathlib import Path

import numpy as np

from rowver.extract import core

FIXTURE = Path(__file__).resolve().parent.parent / "contract/core/cases.json"
CASES = json.loads(FIXTURE.read_text(encoding="utf-8"))

RTOL = 1e-8


def dec(vals):
    return np.array([math.nan if v is None
                     else (math.inf if v == "inf" else -math.inf)
                     if isinstance(v, str) else v for v in vals], float)


def scalar(v):
    return {"nan": math.nan, "inf": math.inf, "-inf": -math.inf}.get(v, v)


class MathFixtureTests(unittest.TestCase):
    def assertClose(self, got, want, msg=None):
        np.testing.assert_allclose(np.asarray(got, float), np.asarray(want, float),
                                   rtol=RTOL, atol=0, equal_nan=True,
                                   err_msg=msg or "")

    def test_edges(self):
        for c in CASES["edges"]:
            e = core.edges(c["lo"], c["hi"], c["nb"], c["log"])
            self.assertEqual(e["on"], c["expect"]["on"])
            self.assertClose([e["a"], e["b"], e["s"]],
                             [c["expect"]["a"], c["expect"]["b"], c["expect"]["s"]])
            self.assertClose(core.centres(c["lo"], c["hi"], c["nb"], c["log"]),
                             dec(c["centres"]))
            self.assertClose(core.edge_list(c["lo"], c["hi"], c["nb"], c["log"]),
                             dec(c["edge_list"]))

    def test_binned(self):
        for c in CASES["binned"]:
            r = core.binned(dec(c["values"]), np.array(c["ww"], float),
                            np.array(c["keep"], bool), c["lo"], c["hi"], c["nb"],
                            c["log"], c["oflow"])
            x = c["expect"]
            self.assertEqual(list(r["n"]), x["n"], c["id"])
            self.assertEqual([r["over"], r["under"], r["kept"]],
                             [x["over"], x["under"], x["kept"]], c["id"])
            self.assertClose(r["h"], x["h"], c["id"])
            self.assertClose(r["h2"], x["h2"], c["id"])
            self.assertClose([r["overW"], r["underW"]], [x["overW"], x["underW"]],
                             c["id"])
            for key in ("overW", "underW"):
                if x[key] == 0:
                    # the page's +0 accumulator: a -0.0 tail must not survive
                    self.assertEqual(math.copysign(1.0, r[key]), 1.0,
                                     f'{c["id"]} {key} sign')

    def test_binned2d(self):
        for c in CASES["binned2d"]:
            r = core.binned2d(dec(c["vx"]), np.array(c["vy"], float),
                              np.array(c["ww"]),
                              np.array(c.get("keep", [1] * len(c["vx"])), bool),
                              c["lox"], c["hix"], c["nbx"],
                              c["loy"], c["hiy"], c["nby"],
                              lbx=c["lbx"], lby=c["lby"], oflow=c["oflow"],
                              ds=np.array(c["ds"]) if "ds" in c else None,
                              ok_ds=np.array(c["ok_ds"], bool)
                              if "ok_ds" in c else None)
            self.assertEqual(r["kept"], c["expect"]["kept"], c["id"])
            self.assertClose(r["h"], c["expect"]["h"], c["id"])

    def test_err_modes(self):
        for c in CASES["err_modes"]:
            err, fell = core.err_for(c["mode"], c["sw"], c["sw2"], c["n"])
            self.assertEqual(fell, c["expect"]["fellBack"], c["mode"])
            if c["expect"]["err"] is None:
                self.assertIsNone(err)
            else:
                self.assertClose(err, c["expect"]["err"], c["mode"])

    def test_garwood(self):
        for c in CASES["garwood"]:
            self.assertClose(core.garwood(c["n"]), c["expect"])
        # independent pin: 0.5*chi2.ppf distances at n=0 and n=1 (stats.ts:115)
        self.assertAlmostEqual(core.garwood(0)[1], 1.841, places=3)
        self.assertAlmostEqual(core.garwood(1)[0], 0.827, places=3)
        self.assertAlmostEqual(core.garwood(1)[1], 2.300, places=3)

    def test_foms(self):
        fns = {"sig": core.sig_fom, "zbi": core.zbi, "neff": core.neff,
               "sob": core.sob}
        for c in CASES["foms"]:
            got = fns[c["kind"]](*c["args"])
            want = scalar(c["expect"])
            if isinstance(want, float) and math.isnan(want):
                self.assertTrue(math.isnan(got), c)
            elif isinstance(want, float) and math.isinf(want):
                self.assertEqual(got, want, c)
            else:
                self.assertClose([got], [want], str(c))

    def test_region_read(self):
        for c in CASES["region_read"]:
            for key, got in (("sb", core.sob(c["s"], c["b"])),
                             ("zbi", core.zbi(c["s"], c["b"], c["b2"],
                                              c["syst"], c["weighted"]))):
                want = scalar(c["expect"][key])
                if isinstance(want, float) and math.isnan(want):
                    self.assertTrue(math.isnan(got), c)
                else:
                    self.assertClose([got], [want], str(c))


class ResolverFixtureTests(unittest.TestCase):
    def test_resolver_cases(self):
        for c in CASES["resolver"]:
            got = core.resolve_tab(c["meta"], 0)
            x = c["expect"]
            self.assertEqual(got["active"], x["active_load"], c["id"])
            self.assertEqual(got["weighted"], x["weighted"], c["id"])
            self.assertEqual(list(got["lumi"]), x["lumi"], c["id"])
            if "active_session" in x:
                self.assertEqual(
                    core.active_cuts(c["meta"]["data"]["cols"], got["cuts"],
                                     c["session_cutorder"]),
                    x["active_session"], c["id"])
            shown_on = [s for s, on in got["shown"].items() if on]
            self.assertEqual(shown_on, x["shown_on"], c["id"])
            if "effective_cuts" in x:
                self.assertEqual(got["cuts"], x["effective_cuts"], c["id"])
            plots = [e for e in got["entries"] if e["kind"] in ("1d", "2d", "3d")]
            self.assertEqual(len(plots), len(x["plots"]), c["id"])
            for want, entry in zip(x["plots"], plots):
                self.assertEqual(entry["path"], want["path"], c["id"])
                self.assertEqual(entry["name"], want["name"], c["id"])
                # the key SET is part of the contract: an invented or
                # resurrected key must fail, not slide past a subset check
                self.assertEqual(sorted(entry["spec"]), sorted(want["spec"]),
                                 c["id"])
                for k, v in want["spec"].items():
                    self.assertEqual(entry["spec"].get(k), v, f"{c['id']}: {k}")
            for kind in ("board", "map"):
                want = x[kind]
                entry = next((e for e in got["entries"] if e["kind"] == kind), None)
                if want is None:
                    self.assertIsNone(entry, c["id"])
                else:
                    self.assertEqual(entry["path"], want["path"], c["id"])
                    self.assertEqual(entry["name"], want["name"], c["id"])
                    self.assertEqual(sorted(entry["spec"]), sorted(want["spec"]),
                                     f"{c['id']} {kind} keys")
                    for k, v in want["spec"].items():
                        self.assertEqual(entry["spec"].get(k), v,
                                         f"{c['id']} {kind} {k}")

    def test_identity_hand_pinned(self):
        # names/paths typed HERE by hand, immune to fixture regeneration —
        # the identity contract is extractor-only (the page has no names)
        meta = {"view": {}, "tabs": [{"name": "t", "stage": [
            {"kind": "1d", "varx": "MET"},
            {"kind": "block", "layout": [3, 3], "cells": [
                {"kind": "2d", "varx": "MET", "vary": "nJet"},
                {"kind": "map"}]},
            {"kind": "board"},
            {"kind": "1d", "varx": "../A  B/"},
        ]}], "data": {"samples": [{"name": "s", "weighted": False}],
                      "cols": [{"name": "MET", "lim": [0, 1], "nice": [0, 1]},
                               {"name": "nJet", "lim": [0, 5], "nice": [0, 5]},
                               {"name": "../A  B/", "lim": [0, 1],
                                "nice": [0, 1]}]}}
        entries = core.stage_entries(meta, 0)
        self.assertEqual(
            [(e["path"], e["name"]) for e in entries],
            [("t0.s0", "t0_s0_1d_MET"),
             ("t0.s1.c0", "t0_s1_c0_2d_MET_nJet"),
             ("t0.s1.c1", "t0_s1_c1_map"),
             ("t0.s2", "t0_s2_board"),
             # sanitization pinned: replace, collapse runs, strip edges
             ("t0.s3", "t0_s3_1d_A_B")])

    def test_mask_cases(self):
        cols = CASES["resolver"][0]["meta"]["data"]["cols"]
        samples = [s["name"] for s in CASES["resolver"][0]["meta"]["data"]["samples"]]
        for c in CASES["mask"]:
            columns = {"MET": dec(c["MET"]), "nJet": np.array(c["nJet"])}
            shown = {s: s in c["shown_on"] for s in samples}
            keep = core.sel_mask(columns, cols, c["cuts"], shown,
                                 np.array(c["ds"]), samples)
            self.assertEqual([int(k) for k in keep], c["expect_keep"], c["id"])


class RegionFixtureTests(unittest.TestCase):
    def test_region_cases(self):
        for c in CASES["region"]:
            meta = {"view": c["view"], "tabs": [],
                    "data": {"samples": [{"name": s, "weighted": not u}
                                         for s, u in zip(c["samples"], c["unweighted"])],
                             "cols": CASES["resolver"][0]["meta"]["data"]["cols"],
                             "rows": len(c["ds"]), "weight_col": "w"}}
            columns = {"MET": dec(c["rows"]["MET"]), "nJet": np.array(c["rows"]["nJet"]),
                       "w": np.array(c["rows"]["w"])}
            # the page counts only a saved root; both suites derive it from the
            # grid the same way, pinning tree_from_grid itself
            spec = {"kind": "map", "root": core.tree_from_grid(c["grid"])}
            if c["spec_srcs"] is not None:
                spec["srcs"] = c["spec_srcs"]
            num = core.region_count(meta, columns, np.array(c["ds"]),
                                    np.array(c.get("keep", [1] * len(c["ds"])),
                                             bool), spec)
            x = c["expect"]
            self.assertEqual(num["srcs"], x["srcs"], c["id"])
            self.assertEqual([l["name"] for l in num["leaves"]], x["names"], c["id"])
            np.testing.assert_array_equal(num["n"], np.array(x["n"]), c["id"])
            np.testing.assert_allclose(num["w"], np.array(x["w"]), rtol=RTOL, atol=0)
            np.testing.assert_allclose(num["w2"], np.array(x["w2"]), rtol=RTOL, atol=0)
            if x["total"] == 0:
                self.assertEqual(num["total"], 0.0, c["id"])
                self.assertEqual(math.copysign(1.0, num["total"]), 1.0, c["id"])
            else:
                self.assertAlmostEqual(num["total"], x["total"],
                                       delta=abs(x["total"]) * RTOL)
            self.assertFlat(core.region_flat(num), x["flat"], c["id"])

    def assertFlat(self, flat, want, cid):
        """Full content: bin and process keys exact, raw exact, moments rtol."""
        self.assertEqual(list(flat), list(want), cid)
        for bin_, row in want.items():
            self.assertEqual(list(flat[bin_]), list(row), f"{cid} {bin_}")
            for src, cell in row.items():
                got = flat[bin_][src]["nominal"]
                self.assertEqual(got[0], cell["nominal"][0], f"{cid} {bin_} {src} raw")
                np.testing.assert_allclose(got[1:], cell["nominal"][1:],
                                           rtol=RTOL, atol=0,
                                           err_msg=f"{cid} {bin_} {src}")

    def test_region_rootless_grid_is_one_region(self):
        c = CASES["region_rootless"]
        meta = {"view": c["view"], "tabs": [],
                "data": {"samples": [{"name": s, "weighted": not u}
                                     for s, u in zip(c["samples"], c["unweighted"])],
                         "cols": CASES["resolver"][0]["meta"]["data"]["cols"],
                         "rows": len(c["ds"]), "weight_col": "w"}}
        columns = {"MET": dec(c["rows"]["MET"]), "nJet": np.array(c["rows"]["nJet"]),
                   "w": np.array(c["rows"]["w"])}
        num = core.region_count(meta, columns, np.array(c["ds"]),
                                np.ones(len(c["ds"]), bool),
                                {"kind": "map", "grid": c["grid"]})
        self.assertEqual([l["name"] for l in num["leaves"]], c["expect"]["names"])
        self.assertEqual(num["srcs"], c["expect"]["srcs"])
        np.testing.assert_array_equal(num["n"], np.array(c["expect"]["n"]))
        np.testing.assert_allclose(num["w"], np.array(c["expect"]["w"]),
                                   rtol=RTOL, atol=0)

    def test_region_flat_dups(self):
        self._check_flat_case(CASES["region_flat_dups"])

    def test_region_flat_suffix_collision(self):
        # the KNOWN SHARED LIMIT: page and extractor overwrite identically
        self._check_flat_case(CASES["region_flat_suffix_collision"])

    def _check_flat_case(self, c):
        num = {"leaves": [{"name": n} for n in c["leaves"]], "srcs": c["srcs"],
               "n": np.array([dec(r) for r in c["n"]]),
               "w": np.array([dec(r) for r in c["w"]]),
               "w2": np.array([dec(r) for r in c["w2"]])}
        flat = core.region_flat(num)
        self.assertEqual(list(flat), list(c["expect"]))
        for b, row in c["expect"].items():
            for s, cell in row.items():
                got = flat[b][s]["nominal"]
                want = [scalar(v) for v in cell["nominal"]]
                self.assertEqual(got[0], want[0], f"{b} {s} raw")
                for g, w in zip(got[1:], want[1:]):
                    if w == 0:
                        self.assertEqual(g, 0.0, f"{b} {s}")
                        # JS `|| 0` yields POSITIVE zero; -0.0 must not survive
                        self.assertEqual(math.copysign(1.0, g), 1.0, f"{b} {s}")
                    elif math.isinf(w):
                        self.assertEqual(g, w, f"{b} {s}")
                    else:
                        self.assertAlmostEqual(g, w, delta=abs(w) * RTOL)

    def test_region_windows(self):
        for c in CASES["region_windows"]:
            leaves = core.region_leaves(c["root"])
            got = [{"path": l["path"], "name": l["name"], "windows": l["windows"],
                    "vals": l["vals"]} for l in leaves]
            self.assertEqual(got, c["expect"], c["id"])

    def test_grid_leaf_counts(self):
        # 2 thresholds -> 3 MET windows; exhaustive splits every window by the
        # 3-way categorical (incl. remainder): 9 leaves. Non-exhaustive leaves
        # the open tail unsplit: 2*3 + 1 = 7.
        self.assertEqual(len(core.region_leaves(core.tree_from_grid(
            CASES["region"][0]["grid"]))), 9)
        self.assertEqual(len(core.region_leaves(core.tree_from_grid(
            CASES["region"][1]["grid"]))), 7)


if __name__ == "__main__":
    unittest.main()
