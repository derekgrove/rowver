import multiprocessing
import os
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

import numpy as np
import uproot

from rowver import Cart, rvr
from rowver import CartError, cli


def make_parallel_study(root: Path) -> Path:
    sizes = (24, 0, 12, 6, 36, 12, 3)
    names = ("data-large", "empty", "zero-kept", "signal-small",
             "signal-large", "background", "background-tail")
    for name, rows in zip(names, sizes):
        x = np.arange(rows, dtype=np.int32)
        if name == "zero-kept":
            x = -x - 1
        with uproot.recreate(root / f"{name}.root") as output:
            output["Events"] = {
                "x": x,
                "y": x.astype(np.float64) / 7,
                "genWeight": np.where(np.arange(rows) % 2, -0.5, 1.5),
                "valid": np.arange(rows) % 11 != 0,
            }
    (root / "cart.py").write_text(textwrap.dedent("""
        from rowver import Cart

        def prepare(events): return events[events.valid]
        def keep(events): return (events.x >= 0) & (events.x % 3 == 0)
        def scaled(events, metadata): return events.y * metadata["scale"]
        def half_x(events): return events.x // 2
        def weight(events, metadata): return events.genWeight * metadata["norm"]

        CART = Cart(
            fileset={
                "data": {
                    "files": {"data-large.root": "Events", "empty.root": "Events",
                              "zero-kept.root": "Events"},
                    "metadata": {"weights": None, "scale": 1.0},
                },
                "signal": {
                    "files": {"signal-small.root": "Events",
                              "signal-large.root": "Events"},
                    "metadata": {"weights": weight, "norm": 0.25, "scale": 2.0},
                },
                "background": {
                    "files": {"background.root": "Events",
                              "background-tail.root": "Events"},
                    "metadata": {"weights": "genWeight", "scale": 0.5},
                },
            },
            prepare=prepare,
            keep=keep,
            columns={"scaled": scaled, "x": "x", "half_x": half_x},
            title="Parallel press test",
        )
    """), encoding="utf-8")
    return root


class ParallelPressTests(unittest.TestCase):
    def fixture(self, root: Path) -> Path:
        source = root / "events.root"
        with uproot.recreate(source) as output:
            output["Events"] = {
                "x": np.arange(12, dtype=np.int32),
                "weight": np.linspace(1, 2, 12),
            }
        study = root / "study"
        study.mkdir()
        (study / "cart.py").write_text(textwrap.dedent(f"""
            from rowver import Cart

            def prepare(events):
                return events[events.x != 3]

            def keep(events):
                return events.x % 2 == 0

            def doubled(events):
                return events.x * 2

            CART = Cart(
                fileset={{"sample": {{
                    "files": {{{str(source)!r}: "Events"}},
                    "metadata": {{"weights": "weight"}},
                }}}},
                prepare=prepare,
                keep=keep,
                columns={{"x": "x", "twice": doubled}},
            )
        """), encoding="utf-8")
        return study

    def test_production_parallel_builds_are_byte_identical(self):
        fixed = "2026-08-31T12:00:00+00:00"

        class Clock(datetime):
            @classmethod
            def now(cls, tz=None):
                return cls.fromisoformat(fixed).astimezone(tz or timezone.utc)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            make_parallel_study(root)
            with mock.patch("rowver.press.datetime", Clock):
                sequential = Cart.load(root).build(root / "production-default.rvr",
                                                   step_size=5)
            expected = Path(sequential["file"]).read_bytes()
            maximum = max(2, min(os.cpu_count() or 1, 7))
            for workers in range(1, maximum + 1):
                output = root / f"production-{workers}.rvr"
                with mock.patch("rowver.press.datetime", Clock):
                    result = Cart.load(root).build(output, step_size=5, workers=workers)
                actual = Path(result["file"]).read_bytes()
                rvr.read(result["file"])
                self.assertEqual(actual, expected)

    def test_production_merge_ignores_reverse_completion_order(self):
        recipe = """
        import time
        weight = None
        def prepare(events, metadata):
            number = int(Path(metadata["file"]).stem.rsplit("-", 1)[1])
            time.sleep((2 - number) * 0.04)
            return events
        def keep(events): return events.x == events.x
        def column(events): return events.x
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = Cart.load(self.local_study(root, recipe=recipe)).build(
                root / "ordered.rvr", step_size=8, workers=3)
            _, samples = rvr.read_by_sample(result["file"])
        np.testing.assert_array_equal(samples["sample"]["x"], np.arange(24))

    def local_study(self, root: Path, *, recipe: str = "",
                    source_paths: list[str] | None = None) -> Path:
        sources = []
        for number in range(3):
            source = root / f"source-{number}.root"
            with uproot.recreate(source) as output:
                output["Events"] = {
                    "x": np.arange(number * 8, number * 8 + 8, dtype=np.int32),
                    "weight": np.linspace(1, 2, 8),
                }
            sources.append(source)
        study = root / "study"
        study.mkdir()
        paths = source_paths or [str(source) for source in sources]
        files = ", ".join(f"{path!r}: 'Events'" for path in paths)
        source = f"""import os
from pathlib import Path
import numpy as np
from rowver import Cart

{textwrap.dedent(recipe)}

CART = Cart(
    fileset={{"sample": {{
        "files": {{{files}}},
        "metadata": {{"weights": weight}},
    }}}},
    prepare=prepare,
    keep=keep,
    columns={{"x": column}},
)
"""
        (study / "cart.py").write_text(source, encoding="utf-8")
        return study

    def test_parallel_global_size_limit_cancels_without_publication(self):
        recipe = """
        weight = None
        prepare = None
        def keep(events): return events.x == events.x
        def column(events): return events.x
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            study = self.local_study(root, recipe=recipe)
            target = root / "existing.rvr"
            target.write_bytes(b"previous cartridge")
            original = rvr.MAX_DATA_BYTES
            rvr.MAX_DATA_BYTES = 150  # each source fits; their combined 24 rows do not
            try:
                with self.assertRaisesRegex(CartError, "no longer fit"):
                    Cart.load(study).build(target, step_size=4, workers=3)
            finally:
                rvr.MAX_DATA_BYTES = original
            self.assertEqual(target.read_bytes(), b"previous cartridge")
            self.assertFalse(any(child.name.startswith("rowver-source-")
                                 for child in multiprocessing.active_children()))

    def test_parallel_malformed_recipe_outputs_are_rejected(self):
        cases = {
            "prepare": ("prepare = lambda events: 3", "event array"),
            "keep": ("prepare = None\nkeep = lambda events: events.x", "boolean"),
            "keep_length": ("prepare = None\n"
                            "keep = lambda events: events.x[:-1] == events.x[:-1]",
                            "KEEP returned"),
            "column": ("prepare = None\nkeep = lambda events: events.x == events.x\n"
                       "column = lambda events: events.x[:-1]", "returned"),
            "column_nan": ("prepare = None\nkeep = lambda events: events.x == events.x\n"
                           "column = lambda events: np.full(len(events), np.nan)", "NaN"),
            "weight": ("weight = lambda events: events.weight[:-1]", "weight length"),
            "weight_inf": ("weight = lambda events: np.full(len(events), np.inf)",
                           "infinity"),
        }
        defaults = textwrap.dedent("""
        weight = None
        prepare = None
        def keep(events): return events.x == events.x
        def column(events): return events.x
        """)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, (override, message) in cases.items():
                case = root / name
                case.mkdir()
                study = self.local_study(case, recipe=defaults + "\n" + override)
                with self.subTest(name=name), self.assertRaisesRegex(CartError, message):
                    Cart.load(study).build(case / "never.rvr", step_size=4, workers=2)
                self.assertFalse((case / "never.rvr").exists())

    def test_parallel_source_errors_are_redacted_at_every_task_position(self):
        from rowver import press
        original_open = press._open_tree
        secret = "root://user:password@host//bad.root?token=secret#credential"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for failed_at in range(3):
                case = root / str(failed_at)
                case.mkdir()
                local_files = [str(case / f"source-{n}.root") for n in range(3)]
                ordered = local_files[:]
                ordered[failed_at] = secret
                study = self.local_study(case, source_paths=ordered, recipe="""
                    from rowver import press
                    _real_open = press._open_tree
                    def fail_remote(path, tree):
                        if "://" in path:
                            raise OSError(f"backend echoed {path}")
                        return _real_open(path, tree)
                    press._open_tree = fail_remote
                    weight = None
                    prepare = None
                    def keep(events): return events.x == events.x
                    def column(events): return events.x
                """)
                try:
                    with self.subTest(position=failed_at), self.assertRaises(CartError) as caught:
                        Cart.load(study).build(case / "never.rvr", step_size=4, workers=3)
                finally:
                    press._open_tree = original_open
                message = str(caught.exception)
                self.assertIn("root://host//bad.root", message)
                self.assertNotIn("password", message)
                self.assertNotIn("secret", message)
                self.assertNotIn("credential", message)

    def test_parallel_callback_failure_cancels_workers_and_preserves_target(self):
        recipe = """
        import time
        weight = None
        prepare = None
        def keep(events):
            time.sleep(0.05)
            return events.x == events.x
        def column(events): return events.x
        """

        class FailingProgress:
            def source(self, *args): pass
            def __call__(self, *args): raise BrokenPipeError("display closed")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "existing.rvr"
            target.write_bytes(b"old")
            with self.assertRaisesRegex(BrokenPipeError, "display closed"):
                Cart.load(self.local_study(root, recipe=recipe)).build(
                    target, step_size=2, workers=3, progress=FailingProgress())
            self.assertEqual(target.read_bytes(), b"old")
            self.assertFalse(any(child.name.startswith("rowver-source-")
                                 for child in multiprocessing.active_children()))

    def test_parallel_worker_crash_is_detected_and_cleaned_up(self):
        recipe = """
        weight = None
        prepare = None
        def keep(events, metadata):
            if Path(metadata["file"]).name == "source-1.root":
                os._exit(23)
            return events.x == events.x
        def column(events): return events.x
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(CartError, "exit code 23"):
                Cart.load(self.local_study(root, recipe=recipe)).build(
                    root / "never.rvr", step_size=4, workers=3)
            self.assertFalse((root / "never.rvr").exists())
            self.assertFalse(any(child.name.startswith("rowver-source-")
                                 for child in multiprocessing.active_children()))

    def test_parallel_backend_exception_is_never_unpickled(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            recipe = f"""
            import sys
            sys.path.insert(0, {str(root / 'study')!r})
            from baderrors import BadError
            weight = None
            prepare = None
            def keep(events): raise BadError("backend failure", "detail")
            def column(events): return events.x
            """
            study = self.local_study(root, recipe=recipe)
            (study / "baderrors.py").write_text(textwrap.dedent("""
                class BadError(OSError):
                    def __init__(self, message, detail):
                        super().__init__(message)
                        self.detail = detail
            """), encoding="utf-8")
            with self.assertRaisesRegex(CartError, "BadError: backend failure") as caught:
                Cart.load(study).build(root / "never.rvr", step_size=4, workers=2)
            self.assertNotIn("required positional argument", str(caught.exception))
            self.assertFalse((root / "never.rvr").exists())

    def test_parallel_detects_study_helper_drift(self):
        recipe = """
        weight = None
        prepare = None
        def keep(events): return events.x == events.x
        def column(events): return events.x
        """

        class ChangeHelper:
            def __init__(self, helper: Path):
                self.helper = helper
                self.changed = False

            def source(self, *args):
                if not self.changed:
                    self.helper.write_text("VALUE = 2\n", encoding="utf-8")
                    self.changed = True

            def __call__(self, *args): pass

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            study = self.local_study(root, recipe=recipe)
            helper = study / "helper.py"
            helper.write_text("VALUE = 1\n", encoding="utf-8")
            with self.assertRaisesRegex(CartError, "study recipe changed"):
                Cart.load(study).build(root / "never.rvr", step_size=4, workers=2,
                                       progress=ChangeHelper(helper))
            self.assertFalse((root / "never.rvr").exists())

            helper.write_text("VALUE = 1\n", encoding="utf-8")
            cart = Cart.load(study)
            helper.write_text("VALUE = 2\n", encoding="utf-8")
            with self.assertRaisesRegex(CartError, "study recipe changed"):
                cart.build(root / "also-never.rvr", step_size=4, workers=2)
            self.assertFalse((root / "also-never.rvr").exists())

    def test_unreadable_study_recipe_is_reported_as_cart_error(self):
        with tempfile.TemporaryDirectory() as directory:
            study = self.fixture(Path(directory))
            with mock.patch("pathlib.Path.read_bytes", side_effect=PermissionError("denied")):
                with self.assertRaisesRegex(CartError, "cannot read Study recipe.*denied"):
                    Cart.load(study)

    def test_parallel_large_fileset_does_not_deadlock_dispatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            study = root / "study"
            study.mkdir()
            original = root / "one.root"
            with uproot.recreate(original) as output:
                output["Events"] = {"x": np.array([1], dtype=np.int32)}
            paths = []
            for number in range(1000):
                path = root / f"source-{number:04d}-{'x' * 100}.root"
                os.link(original, path)
                paths.append(path)
            # one hard-linked file under a thousand samples: one UUID per sample is allowed
            samples = ",\n".join(
                f"        'sample-{number:04d}': {{'files': {{{str(path)!r}: 'Events'}}, "
                "'metadata': {'weights': None}}" for number, path in enumerate(paths))
            (study / "cart.py").write_text(f"""from rowver import Cart

CART = Cart(
    fileset={{
{samples}
    }},
    keep=lambda events: events.x == events.x,
    columns={{"x": "x"}},
)
""", encoding="utf-8")
            target = root / "many.rvr"
            completed = subprocess.run(
                [sys.executable, "-m", "rowver.cli", "run", "--no-live",
                 "--parallel", "3", str(study), "--step-size", "8",
                 "--out", str(target)],
                cwd=Path(__file__).parents[1], capture_output=True, text=True,
                timeout=30,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            meta, _ = rvr.read(target)
            self.assertEqual(meta["data"]["rows"], 1000)

    def test_parallel_workers_exit_if_coordinator_dies(self):
        def alive(pid: int) -> bool:
            try:
                os.kill(pid, 0)
                return True
            except ProcessLookupError:
                return False

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pid_file = root / "workers.txt"
            recipe = f"""
            import time
            weight = None
            def prepare(events):
                with open({str(pid_file)!r}, "a", encoding="utf-8") as stream:
                    stream.write(f"{{os.getpid()}}\\n")
                time.sleep(30)
                return events
            def keep(events): return events.x == events.x
            def column(events): return events.x
            """
            study = self.local_study(root, recipe=recipe)
            for method in ("terminate", "kill"):
                pid_file.unlink(missing_ok=True)
                target = root / f"{method}.rvr"
                process = subprocess.Popen(
                    [sys.executable, "-m", "rowver.cli", "run", "--no-live",
                     "--parallel", "2", str(study), "--step-size", "8",
                     "--out", str(target)],
                    cwd=Path(__file__).parents[1], stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                worker_pids: set[int] = set()
                try:
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline and len(worker_pids) < 2:
                        if pid_file.exists():
                            worker_pids = {int(value) for value in
                                           pid_file.read_text().splitlines()}
                        time.sleep(0.05)
                    self.assertEqual(len(worker_pids), 2)
                    getattr(process, method)()
                    process.wait(timeout=5)
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline and any(map(alive, worker_pids)):
                        time.sleep(0.05)
                    self.assertFalse(any(map(alive, worker_pids)),
                                     f"{method} left workers {worker_pids}")
                    self.assertFalse(target.exists())
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    for pid in worker_pids:
                        if alive(pid):
                            os.kill(pid, signal.SIGTERM)

    def test_parallel_progress_and_atomic_publication_stay_in_coordinator(self):
        recipe = """
        weight = "weight"
        prepare = None
        def keep(events): return events.x % 2 == 0
        def column(events): return events.x
        """

        class Progress:
            def __init__(self): self.pids = []
            def source(self, *args): self.pids.append(os.getpid())
            def __call__(self, *args): self.pids.append(os.getpid())
            def sample_done(self, *args): self.pids.append(os.getpid())

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            study = self.local_study(root, recipe=recipe)
            progress = Progress()
            target = root / "existing.rvr"
            target.write_bytes(b"old")
            failures = (("rowver.rvr.gzip.compress", OSError("compress failed")),
                        ("rowver.rvr.os.replace", OSError("publish failed")))
            for patched, failure in failures:
                with self.subTest(patched=patched), \
                     mock.patch(patched, side_effect=failure), \
                     self.assertRaisesRegex(OSError, str(failure)):
                    Cart.load(study).build(target, step_size=4, workers=3,
                                           progress=progress)
                self.assertEqual(target.read_bytes(), b"old")
                self.assertEqual(list(root.glob(f".{target.name}.*")), [])
            self.assertTrue(progress.pids)
            self.assertEqual(set(progress.pids), {os.getpid()})

    def test_parallel_cli_validation_auto_count_and_live_contract(self):
        parsed = []
        with mock.patch("rowver.cli._run", side_effect=lambda args: parsed.append(args) or 0):
            cli.main(["run", "study", "--parallel"])
            cli.main(["run", "study", "--parallel", "1"])
            cli.main(["run", "study", "--parallel", "max"])
            for bad in ("0", "-1", "nope"):
                with self.assertRaises(SystemExit):
                    cli.main(["run", "study", "--parallel", bad])
        self.assertEqual([args.parallel for args in parsed], ["auto", 1, "max"])

        cart = mock.Mock()
        cart.fileset = {"sample": {"files": {str(n): "Events" for n in range(3)}}}
        cart.summary.return_value = "summary"
        cart.build.return_value = {"file": "out.rvr", "rows": 0, "compressed": 0}
        board = mock.Mock()
        board.live.return_value = mock.MagicMock(__enter__=mock.Mock(return_value=None),
                                                 __exit__=mock.Mock(return_value=False))
        args = mock.Mock(list=False, study="study", step_size="1", out=None,
                         live=True, parallel="auto")
        with mock.patch("rowver.cli.Cart.load", return_value=cart), \
             mock.patch("rowver.dashboard.Dashboard", return_value=board), \
             mock.patch("rowver.cli.os.cpu_count", return_value=10), \
             mock.patch("builtins.print"):
            cli._run(args)
        cart.build.assert_called_once()
        self.assertIs(cart.build.call_args.kwargs["progress"], board)
        self.assertEqual(cart.build.call_args.kwargs["workers"], 3)

        args.parallel = "max"
        cart.build.reset_mock()
        with mock.patch("rowver.cli.Cart.load", return_value=cart), \
             mock.patch("rowver.dashboard.Dashboard", return_value=board), \
             mock.patch("rowver.cli.os.cpu_count", return_value=10), \
             mock.patch("builtins.print"):
            cli._run(args)
        self.assertEqual(cart.build.call_args.kwargs["workers"], 3)

    def test_run_reports_effective_workers_and_available_cores(self):
        cases = [
            (None, 16, 20,
             "using 1 worker (16 cores available); try --parallel for faster processing",
             None),
            (6, 16, 20, "using 6 workers (16 cores available)", 6),
            ("auto", 16, 20, "using 14 workers (16 cores available)", 14),
            ("max", 16, 20, "using 16 workers (16 cores available)", 16),
            ("max", 16, 10, "using 10 workers (16 cores available)", 10),
            (6, 16, 1,
             "using 1 worker (16 cores available); running sequentially", 1),
            (1, 16, 20,
             "using 1 worker (16 cores available); running sequentially", 1),
            ("max", None, 20,
             "using 1 worker (1 core available); running sequentially", 1),
        ]
        for parallel, cores, sources, expected, passed_workers in cases:
            with self.subTest(parallel=parallel, cores=cores, sources=sources):
                cart = mock.Mock()
                cart.fileset = {
                    "sample": {"files": {str(n): "Events" for n in range(sources)}}
                }
                cart.summary.return_value = "summary"
                cart.build.return_value = {"file": "out.rvr", "rows": 0, "compressed": 0}
                args = mock.Mock(list=False, study="study", step_size="1", out=None,
                                 live=False, parallel=parallel)
                with mock.patch("rowver.cli.Cart.load", return_value=cart), \
                     mock.patch("rowver.cli.os.cpu_count", return_value=cores), \
                     mock.patch("builtins.print") as output:
                    cli._run(args)
                output.assert_any_call(expected)
                self.assertEqual(cart.build.call_args.kwargs["workers"], passed_workers)


if __name__ == "__main__":
    unittest.main()
