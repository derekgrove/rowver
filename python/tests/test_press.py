"""rowver run end to end on a small local study: the JSON lines, the human table, failed files,
recipe errors, cancelling, and the file the viewer reads.

Runs under pytest or `python -m unittest discover tests`; the presser is always this checkout.
"""
import hashlib
import importlib.util
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import __version__, rvr  # noqa: E402
from rowver.run import GRACE  # noqa: E402
from rowver.terminal import PIPE_BUF, emit  # noqa: E402

GOLDEN = Path(__file__).resolve().parent / "table.txt"


def lines(stdout):
    """Every stdout line as JSON; fails the test if any line is not."""
    return [json.loads(line) for line in stdout.splitlines()]


def files(events, state):
    """The file lines in one state."""
    return [event for event in events if event["event"] == "file" and event["state"] == state]


def normal(stdout, folder):
    """The human output with what changes between runs replaced: the study's path and the speed."""
    text = stdout.replace(str(folder), "<study>")
    return "\n".join(re.sub(r"\s+[\d,]+$", "  <rate>", line) if re.match(r"\s*\d+/\d+\s", line) else line
                     for line in text.splitlines()) + "\n"


def validate_mjs():
    """The viewer's validate.mjs, from ROWVER_WEB or the rowver_web checkout this one sits in; None without one."""
    for web in (os.environ.get("ROWVER_WEB"), study.REPO.parent.parent):
        if web and (Path(web) / "dist" / "validate.mjs").is_file() and shutil.which("node"):
            return Path(web) / "dist" / "validate.mjs"
    return None


def viewer(test, *args):
    """Run the viewer's validate.mjs; skip the test, saying why, when that viewer reads another rowver."""
    result = subprocess.run(["node", str(validate_mjs()), *map(str, args)], capture_output=True, text=True, timeout=120)
    reads = re.search(r"reads rowver (\d+) only", result.stdout + result.stderr)
    if result.returncode and reads and int(reads.group(1)) != rvr.VERSION:
        test.skipTest(f"the viewer at {validate_mjs().parent.parent} reads rowver {reads.group(1)} only; "
                      f"this presser writes rowver {rvr.VERSION}")
    return result


class Folder(unittest.TestCase):
    """A fresh temporary folder per test."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="rowver-test-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)


class JsonPress(unittest.TestCase):
    """One sequential press with --progress json and --out, shared by the checks below."""

    @classmethod
    def setUpClass(cls):
        cls.root = Path(tempfile.mkdtemp(prefix="rowver-test-"))
        cls.study = study.make(cls.root)
        cls.out = cls.study / ".rowver" / "pressing.rvr"
        cls.result = study.rowver("run", cls.study, "--out", cls.out, "--progress", "json")
        cls.events = lines(cls.result.stdout)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.root, ignore_errors=True)

    def test_lines_tell_the_press(self):
        self.assertEqual(self.result.returncode, 0, self.result.stderr)
        start, wrote = self.events[0], self.events[-1]
        self.assertEqual(start["event"], "start")
        self.assertEqual((start["study"], start["samples"], start["files"], start["workers"]), ("tiny", 2, 2, 1))
        self.assertEqual(start["presser"], __version__)
        self.assertEqual([sample for sample, _ in start["inputs"]], ["alpha", "beta"])
        self.assertEqual([event["index"] for event in files(self.events, "opening")], [0, 1])
        done = files(self.events, "done")
        self.assertEqual([(event["entries"], event["kept"]) for event in done], [(200, 159), (120, 98)])
        self.assertTrue(files(self.events, "reading"))
        self.assertEqual(self.events[-2], {"event": "pack", "rows": 257})
        self.assertEqual(wrote["event"], "wrote")
        self.assertEqual((wrote["rows"], wrote["columns"], wrote["failed"], wrote["left_out"]), (257, 5, [], []))
        self.assertEqual((Path(wrote["path"]), wrote["bytes"]), (self.out, self.out.stat().st_size))

    def test_only_the_out_path_is_written(self):
        self.assertTrue(self.out.is_file())
        self.assertFalse((self.study / "tiny.rvr").exists())

    def test_no_bytecode_in_the_study(self):
        self.assertEqual(list(self.study.rglob("__pycache__")), [])

    def test_rowver_validate_accepts_it(self):
        result = study.rowver("validate", self.out)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertTrue(result.stdout.rstrip().endswith("valid"))

    def test_recipe_snapshot_matches_cart_py(self):
        source = (self.study / "cart.py").read_text(encoding="utf-8")
        header, _ = rvr.read(self.out)
        self.assertEqual(header["made"]["cart"]["sha256"], hashlib.sha256(source.encode("utf-8")).hexdigest())
        dumped = study.rowver("dump", self.out, "--cart").stdout
        printed = dumped.split("##### begin cart.py #####\n", 1)[1].rsplit("##### end cart.py #####", 1)[0]
        self.assertEqual(hashlib.sha256(printed.encode("utf-8")).hexdigest(), header["made"]["cart"]["sha256"])

    @unittest.skipUnless(validate_mjs(), "no rowver_web dist/validate.mjs (set ROWVER_WEB) or no node")
    def test_the_viewer_reads_it_and_its_save_validates(self):
        saved = self.root / "saved.rvr"
        result = viewer(self, self.out, "--initialize", saved)
        self.assertEqual(result.returncode, 0, result.stderr)
        header, _ = rvr.read(saved)
        self.assertIn("rowver_web", header)
        result = study.rowver("validate", saved)
        self.assertEqual(result.returncode, 0, result.stdout)
        original, _ = rvr.read(self.out)
        self.assertEqual((header["data"], header["made"]), (original["data"], original["made"]))


class ParallelPress(Folder):
    def test_workers_write_whole_lines(self):
        folder = study.make(self.root, files={})
        paths = {sample: [folder / f"{sample}{n}.root" for n in range(6)] for sample in ("alpha", "beta")}
        for seed, path in enumerate(paths["alpha"] + paths["beta"]):
            study.tree(path, 150, seed)
        study.point(folder, paths)
        result = study.rowver("run", folder, "--parallel", "3", "--progress", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        events = lines(result.stdout)
        self.assertEqual(events[0]["files"], 12)
        self.assertEqual(sorted(event["index"] for event in files(events, "done")), list(range(12)))
        self.assertTrue(all(len(line) + 1 < PIPE_BUF for line in result.stdout.splitlines()[1:-1]))
        self.assertEqual(events[-1]["event"], "wrote")
        self.assertEqual(study.rowver("validate", folder / "tiny.rvr").returncode, 0)
        self.assertEqual(list(folder.rglob("__pycache__")), [])
        sequential = study.rowver("run", folder, "--out", self.root / "one.rvr", "--progress", "json")
        self.assertEqual(lines(sequential.stdout)[-1]["rows"], events[-1]["rows"])

    def test_rows_come_in_fileset_order(self):
        """The first file is the largest, so it finishes last; its rows still come first."""
        folder = study.make(self.root, files={})
        paths = [folder / f"a{n}.root" for n in range(4)]
        for n, path in enumerate(paths):
            study.tree(path, 200_000 if n == 0 else 100, n)
        study.point(folder, {"alpha": paths})
        result = study.rowver("run", folder, "--parallel", "4", "--progress", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        done = [event["index"] for event in files(lines(result.stdout), "done")]
        self.assertEqual(done[-1], 0, "the large file should finish last for this test to mean anything")
        self.assertEqual(study.rowver("run", folder, "--out", self.root / "one.rvr").returncode, 0)
        parallel, _ = rvr.read(folder / "tiny.rvr")
        sequential, _ = rvr.read(self.root / "one.rvr")
        self.assertEqual(parallel["data"], sequential["data"])
        self.assertEqual(list(parallel["made"]["files"]["alpha"]["files"]), [str(path) for path in paths])


class Container(Folder):
    """rowver 13: a plain header, each array compressed on its own, and each row's entry number."""

    def test_entries_are_each_rows_place_in_its_file(self):
        import numpy as np
        import uproot
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder, "--parallel", "2").returncode, 0)
        header, section = rvr.read(folder / "tiny.rvr")
        data = header["data"]
        entry = rvr.array(section, data["entry"], "i8", data["rows"])
        self.assertEqual([(f["sample"], Path(f["path"]).name) for f in data["files"]],
                         [("alpha", "a.root"), ("beta", "b.root")])
        start = 0
        for f in data["files"]:
            muons = uproot.open(f["path"])["Events"]["nMuon"].array(library="np")
            self.assertEqual(entry[start:start + f["rows"]].tolist(), np.flatnonzero(muons >= 1).tolist())
            self.assertTrue((rvr.column(header, section, "Nmuon")[start:start + f["rows"]]
                             == muons[muons >= 1]).all())
            start += f["rows"]
        self.assertEqual(start, data["rows"])

    def test_the_header_is_plain_and_the_arrays_follow_it(self):
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        raw = (folder / "tiny.rvr").read_bytes()
        self.assertEqual(raw[:8], b"\x89RVR\r\n\x1a\n")
        length = int.from_bytes(raw[8:12], "little")
        header = json.loads(raw[12:12 + length])
        self.assertEqual(header["rowver"], 13)
        records = [record for record, *_ in rvr.blocks(header["data"])]
        self.assertEqual(sum(r["size"] for r in records), len(raw) - 12 - length)
        self.assertEqual(header["data"]["sha256"], rvr.data_checksum(header["data"]))

    def test_validate_names_a_damaged_array_and_bytes_after_the_last(self):
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        path = folder / "tiny.rvr"
        good = path.read_bytes()
        header, _ = rvr.read(path)
        met = next(c for c in header["data"]["cols"] if c["name"] == "MET")
        where = len(good) - sum(r["size"] for r, *_ in rvr.blocks(header["data"])) + met["at"] + met["size"] // 2
        path.write_bytes(good[:where] + bytes([good[where] ^ 0xFF]) + good[where + 1:])
        result = study.rowver("validate", path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL MET: decompresses to", result.stdout)
        path.write_bytes(good + b"\0")
        result = study.rowver("validate", path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL arrays fill the file in order, end to end", result.stdout)

    def test_entries_follow_events_that_prepare_selects_and_reorders(self):
        import numpy as np
        import uproot
        extra = ("import awkward as ak\n\n\ndef prep(events):\n"
                 "    events = events[events.MET > 20]\n"
                 "    return events[ak.argsort(events.MET)]\n")
        folder = study.make(self.root, extra=extra)
        cart = folder / "cart.py"
        cart.write_text(cart.read_text(encoding="utf-8").replace("keep=keep,", "keep=keep, prepare=prep,"),
                        encoding="utf-8")
        result = study.rowver("run", folder)
        self.assertEqual(result.returncode, 0, result.stderr)
        header, section = rvr.read(folder / "tiny.rvr")
        data = header["data"]
        entry, met = rvr.array(section, data["entry"], "i8", data["rows"]), rvr.column(header, section, "MET")
        start = 0
        for f in data["files"]:
            root = uproot.open(f["path"])["Events"]["MET"].array(library="np")
            rows = slice(start, start + f["rows"])
            self.assertTrue((root[entry[rows]] == met[rows]).all())
            start += f["rows"]
        self.assertTrue((np.diff(met[:data["files"][0]["rows"]]) >= 0).all(), "prepare's order is kept")
        self.assertEqual(study.rowver("validate", folder / "tiny.rvr").returncode, 0)

    def test_a_prepare_that_returns_new_events_is_a_recipe_error(self):
        extra = ("import awkward as ak\n\n\ndef prep(events):\n"
                 "    return ak.zip({'nMuon': events.nMuon, 'MET': events.MET, 'Muon_pt': events.Muon_pt, 'q': events.q})\n")
        folder = study.make(self.root, extra=extra)
        cart = folder / "cart.py"
        cart.write_text(cart.read_text(encoding="utf-8").replace("keep=keep,", "keep=keep, prepare=prep,"),
                        encoding="utf-8")
        result = study.rowver("run", folder, "--progress", "json")
        self.assertNotEqual(result.returncode, 0)
        last = lines(result.stdout)[-1]
        self.assertEqual((last["event"], last["kind"]), ("error", "recipe"), last)
        self.assertIn("without the fields they were given", last["message"])

    def test_validate_refuses_a_file_that_is_not_rowver_13(self):
        path = self.root / "old.rvr"
        path.write_bytes(b"\x1f\x8b" + bytes(64))
        result = study.rowver("validate", path, path)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout.count("FAIL a rowver 13 file"), 2, result.stdout)

    def test_a_rowver_12_header_is_refused(self):
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        path = folder / "tiny.rvr"
        header, section = rvr.read(path)
        header["rowver"] = 12
        rvr.write(path, header, [bytes(section)])
        for reader in (rvr.read, rvr.read_header):
            with self.assertRaisesRegex(ValueError, "says rowver 12; this reads rowver 13 only"):
                reader(path)
        result = study.rowver("validate", path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("FAIL a rowver 13 file", result.stdout)
        self.assertIn("says rowver 12", result.stdout)
        for command in (("dump", path), ("diff", path, path)):
            self.assertNotEqual(study.rowver(*command).returncode, 0)


class RecipeModules(Folder):
    """A recipe that imports modules from its own folder carries them in made.cart.modules."""

    def test_modules_are_snapshotted(self):
        extra = ("import sys\nfrom pathlib import Path\n"
                 "sys.path.insert(0, str(Path(__file__).resolve().parent))\n"
                 "import helper\n")
        folder = study.make(self.root, extra=extra)
        (folder / "helper.py").write_text("from util import more\n\nSCALE = more.SCALE\n", encoding="utf-8")
        (folder / "util").mkdir()
        (folder / "util" / "more.py").write_text("SCALE = 2  # é\n", encoding="utf-8")
        (folder / "unused.py").write_text("NOT_IMPORTED = 1\n", encoding="utf-8")
        result = study.rowver("run", folder)
        self.assertEqual(result.returncode, 0, result.stderr)
        header, _ = rvr.read(folder / "tiny.rvr")
        modules = header["made"]["cart"]["modules"]
        self.assertEqual(list(modules), ["helper.py", "util/more.py"])
        for path, module in modules.items():
            text = (folder / path).read_text(encoding="utf-8")
            self.assertEqual(module, {"sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(), "source": text})
        self.assertEqual(header["made"]["sha256"], rvr.made_checksum(header["made"]))
        checked = study.rowver("validate", folder / "tiny.rvr")
        self.assertEqual(checked.returncode, 0, checked.stdout)
        self.assertIn("module util/more.py: sha256 matches its source", checked.stdout)
        self.assertEqual(list(folder.rglob("__pycache__")), [])

    def test_installed_code_is_left_out_and_source_decodes_as_python(self):
        extra = ("import sys\nfrom pathlib import Path\nhere = Path(__file__).resolve().parent\n"
                 "sys.path[:0] = [str(here), str(here / 'env' / 'lib' / 'site-packages')]\n"
                 "import latin, installed\n")
        folder = study.make(self.root, extra=extra)
        (folder / "latin.py").write_bytes("# -*- coding: latin-1 -*-\nNAME = 'café'\r\n".encode("latin-1"))
        (folder / "env" / "lib" / "site-packages").mkdir(parents=True)
        (folder / "env" / "lib" / "site-packages" / "installed.py").write_text("X = 1\n", encoding="utf-8")
        result = study.rowver("run", folder)
        self.assertEqual(result.returncode, 0, result.stderr)
        header, _ = rvr.read(folder / "tiny.rvr")
        modules = header["made"]["cart"]["modules"]
        self.assertEqual(list(modules), ["latin.py"])
        self.assertEqual(modules["latin.py"]["source"], "# -*- coding: latin-1 -*-\nNAME = 'café'\n")
        self.assertEqual(study.rowver("validate", folder / "tiny.rvr").returncode, 0)

    def test_sequential_and_parallel_record_the_same_snapshot(self):
        """A module a hook imports only while events are read is in neither."""
        extra = ("import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parent))\n"
                 "import helper\n\n\ndef prep(events):\n    import lazy\n    return events\n")
        folder = study.make(self.root, extra=extra)
        cart = folder / "cart.py"
        cart.write_text(cart.read_text(encoding="utf-8").replace("keep=keep,", "keep=keep, prepare=prep,"),
                        encoding="utf-8")
        (folder / "helper.py").write_text("X = 1\n", encoding="utf-8")
        (folder / "lazy.py").write_text("Y = 2\n", encoding="utf-8")
        self.assertEqual(study.rowver("run", folder, "--out", self.root / "one.rvr").returncode, 0)
        result = study.rowver("run", folder, "--out", self.root / "two.rvr", "--parallel", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        one, _ = rvr.read(self.root / "one.rvr")
        two, _ = rvr.read(self.root / "two.rvr")
        self.assertEqual(list(one["made"]["cart"]["modules"]), ["helper.py"])
        self.assertEqual(one["made"]["cart"], two["made"]["cart"])

    def test_a_recipe_without_modules_has_none(self):
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        header, _ = rvr.read(folder / "tiny.rvr")
        self.assertNotIn("modules", header["made"]["cart"])

    @unittest.skipUnless(validate_mjs(), "no rowver_web dist/validate.mjs (set ROWVER_WEB) or no node")
    def test_the_viewer_reads_a_file_with_modules(self):
        extra = "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parent))\nimport helper\n"
        folder = study.make(self.root, extra=extra)
        (folder / "helper.py").write_text("X = 1\n", encoding="utf-8")
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        result = viewer(self, folder / "tiny.rvr")
        self.assertEqual(result.returncode, 0, result.stderr)


class HumanTable(Folder):
    def test_table_is_unchanged(self):
        """The table rowver 11.0.0 printed for the same study, speeds aside (tests/table.txt)."""
        folder = study.make(self.root)
        result = study.rowver("run", folder)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(normal(result.stdout, folder), GOLDEN.read_text())

    def test_a_failed_file_is_named(self):
        folder = study.make(self.root, files={})
        study.tree(folder / "a.root", 200, 1)
        study.point(folder, {"alpha": [folder / "a.root", folder / "missing.root"]})
        result = study.rowver("run", folder)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("missing.root could not be read, left out:", result.stdout)
        self.assertIn("1 of 2 files could not be read and are not in the file.", result.stdout)


class FailedFiles(Folder):
    def press(self, files, *extra):
        folder = study.make(self.root, files={})
        study.tree(folder / "a.root", 200, 1)
        study.tree(folder / "b.root", 120, 2)
        study.point(folder, {sample: [folder / name for name in names] for sample, names in files.items()})
        result = study.rowver("run", folder, "--progress", "json", *extra)
        return folder, result, lines(result.stdout)

    def test_a_missing_file_is_retried_once_then_left_out(self):
        for extra in ((), ("--parallel", "2")):
            with self.subTest(extra=extra):
                shutil.rmtree(self.root / "tiny", ignore_errors=True)
                folder, result, events = self.press({"alpha": ["a.root", "gone.root"], "beta": ["b.root"]}, *extra)
                self.assertEqual(result.returncode, 0, result.stderr)
                gone = str(folder / "gone.root")
                failed = files(events, "failed")
                self.assertEqual([event["path"] for event in failed], [gone])
                self.assertIn("gone.root", failed[0]["message"])
                self.assertEqual(len([e for e in files(events, "opening") if e["path"] == gone]), 2)
                wrote = events[-1]
                self.assertEqual([entry["path"] for entry in wrote["failed"]], [gone])
                self.assertEqual((wrote["rows"], wrote["left_out"]), (257, []))
                header, _ = rvr.read(folder / "tiny.rvr")
                self.assertNotIn(gone, header["made"]["files"]["alpha"]["files"])
                self.assertEqual(study.rowver("validate", folder / "tiny.rvr").returncode, 0)

    def test_the_first_file_missing_under_workers(self):
        folder, result, events = self.press({"alpha": ["gone.root", "a.root"], "beta": ["b.root"]}, "--parallel", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([event["index"] for event in files(events, "failed")], [0])
        self.assertEqual(sorted(event["index"] for event in files(events, "done")), [1, 2])

    def test_a_sample_with_no_file_read_is_left_out(self):
        folder, result, events = self.press({"alpha": ["a.root"], "beta": ["gone.root"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(events[-1]["left_out"], ["beta"])
        header, _ = rvr.read(folder / "tiny.rvr")
        self.assertEqual([sample["name"] for sample in header["data"]["samples"]], ["alpha"])
        self.assertEqual(study.rowver("validate", folder / "tiny.rvr").returncode, 0)

    def test_no_file_read_is_an_input_error(self):
        folder, result, events = self.press({"alpha": ["gone.root"], "beta": ["lost.root"]})
        self.assertEqual(result.returncode, 1)
        self.assertEqual((events[-1]["event"], events[-1]["kind"]), ("error", "input"))
        self.assertEqual(events[-1]["message"], "None of the 2 input files could be read.")
        self.assertFalse((folder / "tiny.rvr").exists())


HALF_WRITE = """
import sys
from pathlib import Path
from rowver import rvr, run

def half(path, header, arrays):
    Path(path).write_bytes(rvr.MAGIC + bytes(4096))
    print("on disk", Path(path).stat().st_size, file=sys.stderr)
    raise OSError(28, "No space left on device")

rvr.write = half
sys.exit(run.main(sys.argv[1], 1, None, True))
"""


class WriteFailures(Folder):
    def ends_in_a_write_error(self, result):
        self.assertEqual(result.returncode, 1, result.stderr)
        events = lines(result.stdout)
        self.assertEqual((events[-1]["event"], events[-1]["kind"]), ("error", "write"), result.stderr)
        return events[-1]

    def test_out_is_an_existing_folder(self):
        folder = study.make(self.root)
        (self.root / "taken").mkdir()
        self.ends_in_a_write_error(study.rowver("run", folder, "--progress", "json", "--out", self.root / "taken"))
        self.assertTrue((self.root / "taken").is_dir())

    def test_a_parent_of_out_is_a_file(self):
        folder = study.make(self.root)
        (self.root / "afile").write_text("not a folder")
        out = self.root / "afile" / "x.rvr"
        self.ends_in_a_write_error(study.rowver("run", folder, "--progress", "json", "--out", out))
        self.assertEqual((self.root / "afile").read_text(), "not a folder")

    def test_a_half_written_file_is_removed(self):
        folder = study.make(self.root)
        result = subprocess.run([sys.executable, "-c", HALF_WRITE, str(folder)], env=study.environment(),
                                capture_output=True, text=True, timeout=180)
        self.assertIn("on disk 4104", result.stderr)
        error = self.ends_in_a_write_error(result)
        self.assertIn("No space left on device", error["message"])
        self.assertFalse((folder / "tiny.rvr").exists())


class RecipeErrors(Folder):
    def broken(self, old, new, *extra):
        folder = study.make(self.root)
        text = (folder / "cart.py").read_text()
        self.assertIn(old, text)
        (folder / "cart.py").write_text(text.replace(old, new))
        result = study.rowver("run", folder, "--progress", "json", *extra)
        return folder, result, lines(result.stdout)

    def test_a_failing_hook_stops_before_the_workers_start(self):
        for extra in ((), ("--parallel", "2")):
            with self.subTest(extra=extra):
                shutil.rmtree(self.root / "tiny", ignore_errors=True)
                folder, result, events = self.broken("return events.nMuon >= 1", "return events.nMuon / 0 >= undefined",
                                                     *extra)
                self.assertEqual(result.returncode, 1)
                self.assertEqual((events[-1]["event"], events[-1]["kind"]), ("error", "recipe"))
                self.assertEqual(events[-1]["message"], "keep: NameError: name 'undefined' is not defined")
                self.assertIn("cart.py", events[-1]["traceback"])
                self.assertEqual(files(events, "done"), [])
                if extra:
                    self.assertEqual([event for event in events if event["event"] == "file"], [])
                self.assertFalse((folder / "tiny.rvr").exists())

    def test_a_column_naming_no_branch(self):
        _, result, events = self.broken('"MET": "MET"', '"MET": "MET_pt"')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(events[-1]["kind"], "recipe")
        self.assertTrue(events[-1]["message"].startswith("column MET: "), events[-1]["message"])

    def test_a_syntax_error(self):
        _, result, events = self.broken("def lead(events):", "def lead(events)")
        self.assertEqual(result.returncode, 1)
        self.assertEqual([event["event"] for event in events], ["error"])
        self.assertEqual(events[0]["kind"], "recipe")
        self.assertTrue(events[0]["message"].startswith("cart.py: SyntaxError: "), events[0]["message"])

    def test_a_print_in_a_hook_stays_off_the_json(self):
        for extra in ((), ("--parallel", "2")):
            with self.subTest(extra=extra):
                shutil.rmtree(self.root / "tiny", ignore_errors=True)
                _, result, events = self.broken('    """At least one muon."""',
                                                '    """At least one muon."""\n    print("hello from keep")', *extra)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(events[-1]["event"], "wrote")
                self.assertIn("hello from keep", result.stderr)


SLOW = '''
import multiprocessing, os, time
if multiprocessing.parent_process() is not None or os.environ.get("ROWVER_TEST_SLOW") == "all":
    _keep = keep

    def keep(events):
        """Take a minute, so the press can be cancelled while it reads."""
        time.sleep(60)
        return _keep(events)
'''


class Cancel(Folder):
    def start(self, *extra, **env):
        folder = study.make(self.root)
        text = (folder / "cart.py").read_text()
        (folder / "cart.py").write_text(text.replace("\n\nFILESET", SLOW + "\n\nFILESET", 1))
        process = subprocess.Popen([sys.executable, "-m", "rowver", "run", str(folder), "--progress", "json", *extra],
                                   env=study.environment(**env), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, start_new_session=True)
        self.addCleanup(lambda: process.poll() is None and os.killpg(process.pid, signal.SIGKILL))
        return folder, process

    def wait_for(self, process, count):
        """Read lines until `count` files are opening; return what was read."""
        events = []
        while len(files(events, "opening")) < count:
            line = process.stdout.readline()
            self.assertTrue(line, "the press ended before the files opened")
            events.append(json.loads(line))
        time.sleep(0.5)
        return events

    def children(self, pid):
        """The living processes whose parent is pid."""
        return [child for child, parent in self.living().items() if parent == pid]

    @staticmethod
    def living(group=None):
        """{pid: parent} of every process that has not ended, or of one process group's; a zombie has
        ended (in a container nothing may reap an orphan, so it stays listed)."""
        out = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,pgid=,stat="], capture_output=True, text=True).stdout
        rows = [line.split() for line in out.splitlines()]
        return {int(pid): int(ppid) for pid, ppid, pgid, stat in rows
                if not stat.startswith("Z") and (group is None or int(pgid) == group)}

    def finish(self, process, folder, children):
        """The press ends at once, says it was cancelled, and leaves no worker and no file behind."""
        stdout, _ = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 1)
        last = json.loads(stdout.splitlines()[-1])
        self.assertEqual((last["event"], last["kind"]), ("error", "cancelled"))
        deadline = time.monotonic() + 10
        while (set(children) & set(self.living()) or self.living(process.pid)) and time.monotonic() < deadline:
            time.sleep(0.1)
        self.assertEqual(set(children) & set(self.living()), set())
        self.assertEqual(self.living(process.pid), {})
        self.assertFalse((folder / "tiny.rvr").exists())
        self.assertEqual(list(folder.rglob("__pycache__")), [])

    def test_sigterm_to_the_presser_ends_its_workers(self):
        folder, process = self.start("--parallel", "2")
        self.wait_for(process, 2)
        children = self.children(process.pid)
        self.assertGreaterEqual(len(children), 2)
        process.send_signal(signal.SIGTERM)
        self.finish(process, folder, children)

    def test_sigterm_to_the_process_group(self):
        folder, process = self.start("--parallel", "2")
        self.wait_for(process, 2)
        children = self.children(process.pid)
        os.killpg(process.pid, signal.SIGTERM)
        self.finish(process, folder, children)

    def test_sigint_to_a_sequential_press(self):
        folder, process = self.start(ROWVER_TEST_SLOW="all")
        self.wait_for(process, 1)
        process.send_signal(signal.SIGINT)
        self.finish(process, folder, [])

    @unittest.skipUnless(importlib.util.find_spec("XRootD"), "no XRootD client")
    def test_sigterm_while_an_open_waits_on_the_network(self):
        """192.0.2.1 is TEST-NET: a connection there waits until XRootD gives up, inside C code that
        Python cannot interrupt, so the watchdog ends the press within its grace period."""
        folder = study.make(self.root, files={})
        study.tree(folder / "a.root", 200, 1)
        study.point(folder, {"remote": ["root://192.0.2.1//store/test.root"], "alpha": [folder / "a.root"]})
        process = subprocess.Popen([sys.executable, "-m", "rowver", "run", str(folder), "--progress", "json"],
                                   env=study.environment(XRD_CONNECTIONWINDOW="30", XRD_CONNECTIONRETRY="2"),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.addCleanup(lambda: process.poll() is None and os.killpg(process.pid, signal.SIGKILL))
        self.wait_for(process, 1)
        if process.poll() is not None:
            self.skipTest("this network refuses 192.0.2.1 at once, so no open waits")
        sent = time.monotonic()
        process.send_signal(signal.SIGTERM)
        self.finish(process, folder, [])
        self.assertLess(time.monotonic() - sent, GRACE + 5)


class Details(Folder):
    def test_version(self):
        result = study.rowver("--version")
        self.assertEqual(result.stdout.strip(), f"rowver {__version__}")
        project = tomllib.loads((study.REPO / "pyproject.toml").read_text())
        self.assertEqual(project["tool"]["setuptools"]["dynamic"]["version"], {"attr": "rowver.__version__"})

    def test_recipe_hash_is_of_utf8_text(self):
        """A non-ASCII recipe with Windows line ends hashes as UTF-8 with \\n, as the app will hash it."""
        folder = study.make(self.root)
        text = (folder / "cart.py").read_text(encoding="utf-8").replace('title="Tiny study"', 'title="Tiny μμ study"')
        (folder / "cart.py").write_bytes(text.replace("\n", "\r\n").encode("utf-8"))
        result = study.rowver("run", folder, "--progress", "json", LC_ALL="C", LANG="C", PYTHONUTF8="0")
        self.assertEqual(result.returncode, 0, result.stderr)
        header, _ = rvr.read(folder / "tiny.rvr")
        self.assertEqual(header["made"]["cart"]["sha256"], hashlib.sha256(text.encode("utf-8")).hexdigest())
        self.assertEqual(header["made"]["title"], "Tiny μμ study")

    def test_a_long_message_stays_under_pipe_buf(self):
        read, write = os.pipe()
        saved = os.dup(1)
        try:
            os.dup2(write, 1)
            emit({"event": "file", "path": "root://x//" + "p" * 200, "state": "failed",
                  "message": 'a "quoted" μ message ' * 100}, whole=True)
        finally:
            os.dup2(saved, 1)
            os.close(saved)
            os.close(write)
        line = os.read(read, 4 * PIPE_BUF)
        os.close(read)
        self.assertLess(len(line), PIPE_BUF)
        self.assertTrue(json.loads(line)["message"].endswith("..."))


if __name__ == "__main__":
    unittest.main()
