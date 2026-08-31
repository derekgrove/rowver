from __future__ import annotations

import contextlib
import io
import os
import re
import tempfile
import time
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock

import numpy as np
import uproot

from rowver import Cart, CartError
from rowver import cli, rvr6
from rowver.dashboard import Dashboard, cells, source_name


def keep_all(events):
    return events.x == events.x


class Stream(io.StringIO):
    def __init__(self, tty: bool = False):
        super().__init__()
        self._tty = tty

    def isatty(self):
        return self._tty


class Tree:
    def __init__(self, entries: int):
        self.num_entries = entries

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


def make_cart(*, names=("unweighted", "weighted"), files=1) -> Cart:
    fileset = {}
    for at, name in enumerate(names):
        fileset[name] = {
            "files": {f"/tmp/{name}-{number}.root": "Events"
                      for number in range(files)},
            "metadata": {"kind": "signal" if at else "data",
                         "weights": None if at == 0 else "weight"},
        }
    cart = Cart(fileset=fileset, columns={"z": "z", "a": "a"}, keep=keep_all)
    cart.name = "study"
    return cart


class SummaryAndCliTests(unittest.TestCase):
    def test_summary_is_weight_based_and_lists_columns_in_order(self):
        cart = make_cart()
        summary = cart.summary()
        self.assertEqual(summary.splitlines(), [
            "study: 2 samples, 2 columns",
            "  unweighted: 1 file(s), unweighted",
            "  weighted: 1 file(s), weighted",
            "  columns pressed into cartridge:",
            "    z",
            "    a",
        ])

    def test_three_progress_modes_parse_explicitly(self):
        seen = []
        with mock.patch("rowver.cli._run",
                        side_effect=lambda args: seen.append(args.progress) or 0):
            cli.main(["run", "study"])
            cli.main(["run", "--no-live", "study"])
            cli.main(["run", "--live-robust", "study"])
            with self.assertRaises(SystemExit):
                cli.main(["run", "--live", "study"])
            with self.assertRaises(SystemExit):
                cli.main(["run", "--live-r", "study"])
        self.assertEqual(seen, ["fast", "plain", "robust"])
        with self.assertRaises(SystemExit):
            cli.main(["run", "--no-live", "--live-robust", "study"])

    def test_default_live_wraps_only_the_real_build(self):
        events = []

        class FakeCart:
            def summary(self):
                return "summary"

            def build(self, *args, **kwargs):
                events.append("build")
                self.assert_live()
                kwargs["progress"].source("sample", "/tmp/source.root", "Events")
                return {"file": "out.rvr", "rows": 1, "compressed": 0}

        cart = FakeCart()
        active = {"live": False}
        cart.assert_live = lambda: self.assertTrue(active["live"])

        class FakeBoard:
            def __init__(self, value, robust=False):
                self.assert_cart = value
                self.robust = robust

            def __call__(self, *args):
                events.append("chunk")

            def source(self, *args):
                events.append("source")

            def live(self):
                class Context:
                    def __enter__(inner):
                        active["live"] = True
                        events.append("enter")

                    def __exit__(inner, *args):
                        events.append("exit")
                        active["live"] = False
                return Context()

            def draw(self, final=False):
                events.append("final")

        args = Namespace(list=False, study="study", step_size="1", out=None,
                         progress="fast")
        with mock.patch("rowver.cli.Cart.load", return_value=cart), \
             mock.patch("rowver.dashboard.Dashboard", FakeBoard), \
             mock.patch("builtins.print"):
            cli._run(args)
        self.assertEqual(events, ["enter", "build", "source", "exit", "final"])

    def test_robust_live_indexes_before_the_real_build(self):
        events = []
        cart = mock.Mock()
        cart.summary.return_value = "summary"
        cart.build.return_value = {"file": "out.rvr", "rows": 0, "compressed": 0}

        class FakeBoard:
            def __init__(self, value, robust=False):
                self.robust = robust

            def __call__(self, *args):
                pass

            def live(self):
                return contextlib.nullcontext()

            def index(self, value):
                events.append("index")

            def begin_skim(self):
                events.append("skim")

            def draw(self, final=False):
                pass

        args = Namespace(list=False, study="study", step_size="1", out=None,
                         progress="robust")
        with mock.patch("rowver.cli.Cart.load", return_value=cart), \
             mock.patch("rowver.dashboard.Dashboard", FakeBoard), \
             mock.patch("builtins.print"):
            cli._run(args)
        self.assertEqual(events, ["index", "skim"])
        self.assertTrue(cart.build.call_args.kwargs["progress"].robust)

    def test_no_live_constructs_no_dashboard_or_index(self):
        cart = mock.Mock()
        cart.summary.return_value = "summary"
        cart.build.return_value = {"file": "out.rvr", "rows": 0, "compressed": 0}
        args = Namespace(list=False, study="study", step_size="1", out=None,
                         progress="plain")
        with mock.patch("rowver.cli.Cart.load", return_value=cart), \
             mock.patch("rowver.dashboard.Dashboard") as dashboard, \
             mock.patch("builtins.print"):
            cli._run(args)
        dashboard.assert_not_called()
        self.assertIsNone(cart.build.call_args.kwargs["progress"])

    def test_modes_open_sources_once_once_twice_and_write_equivalent_cartridges(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "events.root"
            with uproot.recreate(source) as output:
                output["Events"] = {"x": np.arange(6.0),
                                    "w": np.arange(6.0) + 0.5}
            cart = Cart(
                fileset={"sample": {"files": {str(source): "Events"},
                                    "metadata": {"weights": "w"}}},
                columns={"x": "x"}, keep=keep_all,
            )
            cart.name = "study"
            cart.dir = root
            results = []
            counts = []
            from rowver import press
            real_open = press._open_sample_tree
            opened = 0

            def tracked_open(*args):
                nonlocal opened
                opened += 1
                return real_open(*args)

            with mock.patch("rowver.cli.Cart.load", return_value=cart), \
                 mock.patch("rowver.press._open_sample_tree", side_effect=tracked_open), \
                 mock.patch("rowver.dashboard._open_sample_tree", side_effect=tracked_open):
                for mode in ("fast", "plain", "robust"):
                    before = opened
                    args = Namespace(list=False, study="study", step_size="2",
                                     out=root / f"{mode}.rvr", progress=mode)
                    with contextlib.redirect_stdout(Stream()):
                        cli._run(args)
                    counts.append(opened - before)
                    results.append(rvr6.read_by_sample(root / f"{mode}.rvr"))

        self.assertEqual(counts, [1, 1, 2])
        for meta, samples in results[1:]:
            self.assertEqual(meta["about"], results[0][0]["about"])
            self.assertEqual(meta["data"], results[0][0]["data"])
            np.testing.assert_array_equal(samples["sample"]["x"],
                                          results[0][1]["sample"]["x"])
            np.testing.assert_array_equal(samples["sample"]["weight"],
                                          results[0][1]["sample"]["weight"])

    def test_default_source_error_still_uses_the_established_cart_error(self):
        cart = Cart(fileset={"sample": {"files": {"/missing/source.root": "Events"},
                                        "metadata": {"weights": None}}},
                    columns={"x": "x"}, keep=keep_all)
        cart.name = "study"
        cart.dir = Path("/tmp")
        args = Namespace(list=False, study="study", step_size="1",
                         out=Path("/tmp/never.rvr"), progress="fast")
        with mock.patch("rowver.cli.Cart.load", return_value=cart), \
             contextlib.redirect_stdout(Stream()), \
             self.assertRaisesRegex(SystemExit,
                                     "cannot open sample 'sample' file '/missing/source.root'"):
            cli._run(args)


class DashboardTests(unittest.TestCase):
    def test_fast_redirected_progress_has_no_exact_totals_percent_or_eta(self):
        stream = Stream()
        board = Dashboard(make_cart(names=("sample",)), output=stream)
        with board.live():
            board.source("sample", "/tmp/sample.root", "Events")
            board("sample", 10, 3)
            board.draw()
            board.sample_done("sample")
            board(None, 0, 0)
        output = stream.getvalue()
        self.assertIn("skimming sample: sample.root", output)
        self.assertIn("skimmed sample: 10 scanned, 3 kept", output)
        self.assertIn("pressing cartridge…", output)
        self.assertNotIn("eta", output.lower())
        self.assertNotIn("%", output)
        self.assertNotIn("/10", output)

    def test_fast_tty_progress_has_no_exact_totals_percent_or_eta(self):
        stream = Stream(True)
        board = Dashboard(make_cart(names=("sample",)), output=stream)
        with board.live():
            board.source("sample", "/tmp/sample.root", "Events")
            board("sample", 10, 3)
            board.draw()
        output = stream.getvalue()
        self.assertIn("Skimming sample", output)
        self.assertIn("10 scanned", output)
        self.assertIn("3 kept", output)
        self.assertNotIn("eta", output.lower())
        self.assertNotIn("%", output)

    def test_redirected_output_has_durable_lines_and_zero_samples_once(self):
        stream = Stream()
        board = Dashboard(make_cart(names=("zero", "full")), output=stream,
                          robust=True)
        with mock.patch("rowver.dashboard._open_sample_tree",
                        side_effect=[Tree(0), Tree(10)]):
            with board.live():
                board.index(make_cart(names=("zero", "full")))
                board.begin_skim()
                board.chunk("full", 10, 3)
                board.chunk(None, 0, 0)
        lines = stream.getvalue().splitlines()
        self.assertEqual(lines[0], "indexing 1/2 zero: zero-0.root")
        self.assertIn("indexed 1/2 zero: zero-0.root (0 entries; 0 total)", lines)
        self.assertIn("indexed 2/2 full: full-0.root (10 entries; 10 total)", lines)
        self.assertEqual(lines.count("skimmed zero: 0 scanned, 0 kept"), 1)
        self.assertEqual(lines.count("skimmed full: 10 scanned, 3 kept"), 1)
        self.assertEqual(lines[-1], "pressing cartridge…")
        self.assertNotIn("┌", stream.getvalue())
        self.assertNotIn("\x1b[", stream.getvalue())

    def test_tty_draws_before_open_and_ticks_during_delayed_open(self):
        stream = Stream(tty=True)
        cart = make_cart(names=("sample",))
        board = Dashboard(cart, output=stream, robust=True)

        def delayed(*args):
            self.assertIn("Indexing files  1/1", stream.getvalue())
            time.sleep(0.55)
            return Tree(12)

        with mock.patch("rowver.dashboard._open_sample_tree", side_effect=delayed):
            with board.live():
                board.index(cart)
        self.assertGreaterEqual(stream.getvalue().count("Indexing files"), 2)
        self.assertIn("12 entries", stream.getvalue())

    def test_robust_tty_retains_exact_totals_percent_and_eta(self):
        stream = Stream(tty=True)
        cart = make_cart(names=("sample",))
        board = Dashboard(cart, output=stream, robust=True)
        with mock.patch("rowver.dashboard._open_sample_tree", return_value=Tree(10)):
            board.index(cart)
        board.begin_skim()
        board("sample", 5, 2)
        board.draw()
        output = stream.getvalue().lower()
        self.assertIn("5/10", output)
        self.assertIn("50.0%", output)
        self.assertIn("eta", output)

    def test_all_phase_transitions_draw_synchronously_in_tty(self):
        board = Dashboard(make_cart(names=("sample",)), output=Stream(tty=True),
                          robust=True)
        phases = []
        original = board._draw

        def record(final):
            phases.append(board.phase)
            return original(final)

        board._draw = record
        with mock.patch("rowver.dashboard._open_sample_tree", return_value=Tree(1)):
            with board.live():
                board.index(make_cart(names=("sample",)))
                board.begin_skim()
                board.chunk(None, 0, 0)
        self.assertIn("indexing", phases)
        self.assertIn("skimming", phases)
        self.assertIn("pressing", phases)

    def test_tty_pressing_transition_emits_no_redirected_completion_lines(self):
        stream = Stream(True)
        cart = make_cart(names=("sample",))
        board = Dashboard(cart, output=stream, robust=True)
        board.total = {"sample": 1}
        board.begin_skim()
        board.chunk("sample", 1, 1)
        stream.seek(0)
        stream.truncate()
        board.chunk(None, 0, 0)
        self.assertNotIn("skimmed ", stream.getvalue())
        self.assertIn("pressing the cartridge", stream.getvalue())

    def test_index_error_quotes_original_sample_and_redacts_url_secrets(self):
        name = "bad\nname"
        url = "root://user:password@host//full/path/file.root?token=secret#credential"
        cart = Cart(fileset={name: {"files": {url: "Events"},
                                   "metadata": {"weights": None}}},
                    columns={"x": "x"}, keep=keep_all)
        board = Dashboard(cart, output=Stream(), robust=True)
        echoed = OSError(f"cannot resolve {url}")
        with mock.patch("rowver.press._open_tree", side_effect=echoed):
            with self.assertRaises(CartError) as caught:
                board.index(cart)
        message = str(caught.exception)
        self.assertIn("sample 'bad\\nname'", message)
        self.assertIn("root://host//full/path/file.root", message)
        self.assertNotIn("password", message)
        self.assertNotIn("secret", message)
        self.assertNotIn("credential", message)

    def test_index_metadata_failure_is_translated_and_redacted(self):
        class LazyFailure:
            @property
            def num_entries(self):
                raise OSError(
                    "failed root://host//file.root?debug=1&authz=SEKRIT after 10 retries"
                )

            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

        url = "root://host//file.root?debug=1&authz=SEKRIT"
        cart = Cart(fileset={"sample": {"files": {url: "Events"},
                                        "metadata": {"weights": None}}},
                    columns={"x": "x"}, keep=keep_all)
        with mock.patch("rowver.dashboard._open_sample_tree", return_value=LazyFailure()):
            with self.assertRaises(CartError) as caught:
                Dashboard(cart, output=Stream(), robust=True).index(cart)
        message = str(caught.exception)
        self.assertNotIn("SEKRIT", message)
        self.assertNotIn("debug=1", message)
        self.assertIn("after 10 retries", message)

    def test_basename_less_url_never_displays_its_query(self):
        shown = source_name("root://host.cern.ch/?authz=SECRETTOKEN")
        self.assertEqual(shown, "host.cern.ch")
        self.assertNotIn("SECRETTOKEN", shown)

    def test_indexing_frame_fits_a_narrow_terminal(self):
        stream = Stream(True)
        board = Dashboard(make_cart(names=("a very long sample name",)), output=stream,
                          robust=True)
        with mock.patch("rowver.dashboard.shutil.get_terminal_size",
                        return_value=os.terminal_size((40, 20))):
            board.draw()
        ansi = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
        for line in stream.getvalue().splitlines():
            self.assertLessEqual(cells(ansi.sub("", line)), 39)

    def test_names_are_sanitized_and_source_is_cell_bounded(self):
        shown = source_name(
            "root://host/a/control%20/very-very-very-long-世界-filename.root?x=1#y"
        )
        self.assertLessEqual(cells(shown), 28)
        self.assertIn("…", shown)
        cart = make_cart(names=("sample\n\x1bname",))
        stream = Stream()
        board = Dashboard(cart, output=stream, robust=True)
        board.total = {"sample\n\x1bname": 1}
        board.begin_skim()
        board.chunk("sample\n\x1bname", 1, 1)
        self.assertIn("skimmed sample··name", stream.getvalue())
        self.assertNotIn("\x1b", stream.getvalue())

    def test_tty_is_derived_from_each_owned_stream(self):
        cart = make_cart(names=("sample",))
        redirected = Dashboard(cart, output=Stream(False))
        terminal = Dashboard(cart, output=Stream(True))
        self.assertFalse(redirected.tty)
        self.assertTrue(terminal.tty)

    def test_sample_rows_have_no_kind_tags_and_weight_controls_active_colour(self):
        stream = Stream(True)
        board = Dashboard(make_cart(), output=stream, robust=True)
        board.total = {"unweighted": 10, "weighted": 10}
        board.begin_skim()
        output = stream.getvalue()
        self.assertNotIn("DATA", output)
        self.assertNotIn("MC  ", output)
        self.assertNotIn("SIG", output)
        self.assertIn("\x1b[36m", output)
        self.assertIn("\x1b[33m", output)

    def test_background_draw_failure_does_not_escape_live_context(self):
        class FailsAfterInitial(Stream):
            def __init__(self):
                super().__init__(True)
                self.writes = 0

            def write(self, value):
                self.writes += 1
                if self.writes > 1:
                    raise BrokenPipeError("closed")
                return super().write(value)

        board = Dashboard(make_cart(names=("sample",)), output=FailsAfterInitial())
        with board.live():
            time.sleep(0.55)

    def test_main_thread_draw_failure_propagates(self):
        class Closed(Stream):
            def write(self, value):
                raise BrokenPipeError("closed")

        board = Dashboard(make_cart(names=("sample",)), output=Closed(True))
        with self.assertRaises(BrokenPipeError):
            with board.live():
                pass

    def test_root_sources_use_same_explicit_handler_for_index_and_skim(self):
        url = "root://host//file.root"
        cart = Cart(fileset={"sample": {"files": {url: "Events"},
                                        "metadata": {"weights": None}}},
                    columns={"x": "x"}, keep=keep_all)
        with mock.patch("rowver.press.uproot.open", return_value=Tree(0)) as opened:
            Dashboard(cart, output=Stream(), robust=True).index(cart)
            cart.skim(step_size=1)
        self.assertEqual(opened.call_count, 2)
        for call in opened.call_args_list:
            self.assertIs(call.kwargs["handler"],
                          __import__("uproot").source.xrootd.XRootDSource)


if __name__ == "__main__":
    unittest.main()
