"""`rowver render`: images from the record alone, viewer-style resolution.

Rendering is a faithful plot of the record's numbers, never a pixel-clone
claim — so these tests pin the SEMANTICS on the drawn figure (overlay vs
stack, scales, frames, points/errstyle rules, colours), not pixels.
"""
from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver.extract import ExtractError, plot1d, render

from test_plot1d import _cart

HAVE_MPL = importlib.util.find_spec("matplotlib") is not None
HAVE_MPLHEP = importlib.util.find_spec("mplhep") is not None

A = np.array


class YRangeTests(unittest.TestCase):
    """autoYRange's arithmetic, no matplotlib needed — twin numbers from
    rowver_web's frame.test.ts where they exist."""

    def test_mixed_sign_stacked_envelope(self):
        lo, hi = render._y_range([A([10.0]), A([-20.0])], [A([0.]), A([0.])],
                                 [A([0.]), A([0.])], None, [None, None],
                                 [False, False], True, False, None)
        self.assertAlmostEqual(hi, 11.5)
        self.assertAlmostEqual(lo, -23.0)

    def test_band_extends_the_stacked_reach(self):
        lo, hi = render._y_range([A([10.0])], [A([25.0])], [A([1.0])], None,
                                 [None], [False], True, False, "sumw2")
        self.assertAlmostEqual(hi, 15 * 1.15)

    def test_log_floor_walks_the_drawn_cumulatives(self):
        # plotly draws cumulatives 10, 4, -2: the floor sits under the 4
        lo, _ = render._y_range([A([10.0]), A([-6.0]), A([-6.0])],
                                [A([0.])] * 3, [A([0.])] * 3, None,
                                [None] * 3, [False] * 3, True, True, None)
        self.assertAlmostEqual(lo, 2.0)  # 4 * 0.5

    def test_sequential_accumulation_not_compensated(self):
        # 1e16 absorbs each +1 in left-to-right order (the viewer's +=);
        # python's compensated sum() would total 1e16+8 instead
        hs = [A([1e16])] + [A([1.0])] * 8
        z = [A([0.0])] * 9
        _, hi = render._y_range(hs, z, z, None, [None] * 9, [False] * 9,
                                True, False, None)
        self.assertEqual(hi, 1e16 * 1.15)

    def test_overlay_reach_uses_the_recorded_bands(self):
        # recorded up-band 3 (NOT what sumw2 over the moments would give):
        # the frame must ride the drawn numbers
        errs = ([A([1.0])], [A([3.0])])
        _, hi = render._y_range([A([4.0])], [A([100.0])], [A([1.0])], errs,
                                [None], [False], False, False, "sumw2")
        self.assertAlmostEqual(hi, 7 * 1.15)

    def test_points_errstyle_none_excluded_from_reach(self):
        errs = ([A([2.0])], [A([2.0])])
        _, hi = render._y_range([A([4.0])], [A([4.0])], [A([4.0])], errs,
                                ["none"], [True], False, False,
                                "poisson_raw")
        self.assertAlmostEqual(hi, 4 * 1.15)

    def test_band_totals_accumulate_in_draw_order(self):
        band, tot = render._band("sumw2", A([[3.0], [1.0]]),
                                 A([[9.0], [16.0]]), A([[1.0], [1.0]]))
        self.assertEqual(tot[0], 4.0)
        self.assertAlmostEqual(band[0][0], 5.0)  # sqrt(9+16)


class RefusalTests(unittest.TestCase):
    """Argument validation happens before matplotlib is touched."""

    def test_bad_dpi_size_style(self):
        for kw in ({"dpi": 0}, {"dpi": float("nan")}, {"size": (0, 5)},
                   {"size": (float("inf"), 5)}, {"style": "wat"}):
            with self.assertRaises(ExtractError, msg=kw):
                render.render("nowhere.meta.json", **kw)

    def test_parse_size(self):
        self.assertEqual(render._parse_size("10x5"), (10.0, 5.0))
        with self.assertRaises(ExtractError):
            render._parse_size("wide")


@unittest.skipUnless(HAVE_MPL, "matplotlib not installed (rowver[render])")
class RenderTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        rvr = _cart(self.tmp)
        self.out = self.tmp / "extracts"
        plot1d.main([str(rvr), "-o", str(self.out)])
        rvr.unlink()  # from here on, the RECORD alone must suffice
        self.meta0 = self.out / "t0_s0_1d_MET.meta.json"

    def tearDown(self):
        import matplotlib.pyplot as plt
        plt.close("all")
        self._tmp.cleanup()

    def _fig(self, edit=None, **kw):
        meta_path = self.meta0
        if edit:
            meta = json.loads(meta_path.read_text())
            edit(meta)
            meta_path = self.out / "edited.meta.json"
            meta_path.write_text(json.dumps(meta))
            data = (self.out / "t0_s0_1d_MET.data.npz").read_bytes()
            (self.out / "edited.data.npz").write_bytes(data)
        # plain by default: the figure tests must pass with matplotlib
        # alone — the advertised --style plain environment
        fig, _ = render._draw(meta_path, kw.pop("dpi", 150),
                              kw.pop("size", (10, 5)),
                              kw.pop("style", "plain"))
        return fig.axes[0]

    # --- semantics on the drawn figure -------------------------------------

    def test_default_is_overlay_with_per_source_errors(self):
        from matplotlib.patches import StepPatch
        ax = self._fig()
        steps = [p for p in ax.patches if isinstance(p, StepPatch)]
        self.assertEqual(len(steps), 3)          # bkg, sig, obs outlines
        self.assertEqual(len(ax.containers), 3)  # one errorbar set each
        # the stamped record colours are what is drawn, in frame order
        from matplotlib.colors import to_hex
        meta = json.loads(self.meta0.read_text())
        self.assertEqual([to_hex(s.get_edgecolor()).upper() for s in steps],
                         [meta["colours"][s].upper()
                          for s in meta["sources"]])

    def test_stack_truthy_stacks_with_one_total_band(self):
        from matplotlib.container import BarContainer
        ax = self._fig(edit=lambda m: m["spec"].__setitem__("stack", True))
        bars = [c for c in ax.containers if isinstance(c, BarContainer)]
        self.assertEqual(len(bars), 4)  # 3 stacked sources + the band
        # the frame clears the stacked top with the viewer's headroom
        with np.load(self.out / "t0_s0_1d_MET.data.npz") as z:
            top = z["h"].sum(0).max()
        self.assertGreaterEqual(ax.get_ylim()[1], top)

    def test_points_source_and_errstyle_none(self):
        def mark(m):
            m["look"]["sig"] = {"draw": "points", "errstyle": "none"}
        ax = self._fig(edit=mark)
        from matplotlib.patches import StepPatch
        steps = [p for p in ax.patches if isinstance(p, StepPatch)]
        self.assertEqual(len(steps), 2)  # sig left the overlay
        # sig draws as markers but its error bars are SUPPRESSED: only the
        # two remaining bars sources carry errorbar collections
        self.assertEqual(len(ax.collections), 2)

    def test_log_scales_and_manual_y(self):
        ax = self._fig(edit=lambda m: m["spec"].update(logy=True))
        self.assertEqual(ax.get_yscale(), "log")
        ax = self._fig(edit=lambda m: m["spec"].update(
            yauto=False, ymin="1", ymax="50"))
        self.assertEqual(ax.get_ylim(), (1.0, 50.0))
        # manual applies only when BOTH bounds parse finite and ordered —
        # a junk bound keeps the FULL auto range (the backends' rule)
        auto = self._fig().get_ylim()
        ax = self._fig(edit=lambda m: m["spec"].update(
            yauto=False, ymin="abc", ymax="50px"))
        self.assertEqual(ax.get_ylim(), auto)
        # a nonpositive manual low bound on log y CLAMPS to 1e-12 (plotly's
        # rule), never falls back to auto or an unrelated prior limit
        ax = self._fig(edit=lambda m: m["spec"].update(
            logy=True, yauto=False, ymin="-5", ymax="50"))
        self.assertEqual(ax.get_ylim(), (1e-12, 50.0))

    def test_contradictory_records_refused(self):
        # bands in the npz while the spec's error mode is off
        with self.assertRaises(ExtractError):
            self._fig(edit=lambda m: m["spec"].__setitem__("err", "off"))
        # meta.json mirrors that disagree with the npz
        with self.assertRaises(ExtractError):
            self._fig(edit=lambda m: m["edges"].__setitem__(0, -999.0))
        with self.assertRaises(ExtractError):
            self._fig(edit=lambda m: m["sources"].reverse())
        # window.lo controls logx engagement: it must match the edges
        with self.assertRaises(ExtractError):
            self._fig(edit=lambda m: m["window"].__setitem__("lo", -1.0))
        # colours are required as of v2, never guessed at
        with self.assertRaises(ExtractError):
            self._fig(edit=lambda m: m.pop("colours"))

    def test_one_bar_source_never_stacks(self):
        # JSROOT stacks only PLURAL bars: one bar + stack:true draws as the
        # plain outline with its recorded error bars, no fill, no band
        from matplotlib.patches import StepPatch
        def solo(m):
            m["spec"]["stack"] = True
            m["look"]["bkg"] = {"draw": "points"}
            m["look"]["obs"] = {"draw": "points"}
        ax = self._fig(edit=solo)
        from matplotlib.container import BarContainer
        steps = [p for p in ax.patches if isinstance(p, StepPatch)]
        self.assertEqual(len(steps), 1)  # outline, not a filled bar
        self.assertEqual([c for c in ax.containers
                          if isinstance(c, BarContainer)], [])  # no band

    def test_drawn_errorbars_are_the_recorded_bands(self):
        # skew the recorded up-band in the npz: the drawn artists must
        # follow it — proof the renderer reads bands, not moments
        src = self.out / "t0_s0_1d_MET.data.npz"
        with np.load(src) as z:
            arrays = dict(z)
        arrays["err_up"] = arrays["err_up"] + 7.0
        edited = json.loads(self.meta0.read_text())
        (self.out / "edited.meta.json").write_text(json.dumps(edited))
        np.savez(self.out / "edited.data.npz", **arrays)
        fig, _ = render._draw(self.out / "edited.meta.json", 150, (10, 5),
                              "plain")
        ax = fig.axes[0]
        segs = ax.containers[0][2][0].get_segments()  # first source's bars
        h0 = arrays["h"][0]
        for i, seg in enumerate(segs):
            self.assertAlmostEqual(seg[0][1], h0[i] - arrays["err_down"][0][i])
            self.assertAlmostEqual(seg[1][1], h0[i] + arrays["err_up"][0][i])

    def test_style_selection(self):
        with self.assertRaises(ExtractError):
            render.render(self.meta0, style="wat")
        if HAVE_MPLHEP:  # the default sheet actually draws
            self.assertTrue(render.render(self.meta0, self.tmp / "s",
                                          style="root").is_file())
        # without mplhep, every non-plain style refuses with the friendly
        # message instead of an ImportError mid-draw
        import sys
        from unittest import mock
        with mock.patch.dict(sys.modules, {"mplhep": None}):
            with self.assertRaises(ExtractError):
                render._style_context("root")

    # --- files, CLI, refusals ----------------------------------------------

    def test_renders_every_record_from_the_files_alone(self):
        from rowver import cli
        rc = cli.main(["render", str(self.out), "--size", "4x3",
                       "--dpi", "72", "--style", "plain"])
        self.assertEqual(rc, 0)
        pngs = sorted(p.name for p in self.out.glob("*.png"))
        self.assertEqual(pngs, ["t0_s0_1d_MET.png", "t0_s1_c0_1d_MET.png"])

    def test_single_record_and_out_dir(self):
        target = render.render(self.meta0, self.tmp / "imgs", fmt="pdf",
                               style="plain")
        self.assertEqual(target, self.tmp / "imgs" / "t0_s0_1d_MET.pdf")
        self.assertTrue(target.is_file())

    def test_refusals(self):
        bad = self.out / "x.meta.json"
        bad.write_text('{"kind": "cutflow"}')
        with self.assertRaises(ExtractError):
            render.render(bad)  # a NAMED record's failure is the command's
        corrupt = self.out / "y.meta.json"
        corrupt.write_text("{not json")
        # the directory walk skips both and still renders the real records
        self.assertEqual(render.main([str(self.out), "--style", "plain"]), 0)
        # a future schema version is refused, never silently drawn
        v3 = json.loads(self.meta0.read_text())
        v3["version"] = 3
        (self.out / "v3.meta.json").write_text(json.dumps(v3))
        with self.assertRaises(ExtractError):
            render.render(self.out / "v3.meta.json")


if __name__ == "__main__":
    unittest.main()
