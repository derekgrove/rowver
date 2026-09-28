"""rowver probe and rowver check on local files: a 200-event tree, the small test study, and the
four shipped recipes run on local stand-ins for their remote files (tests/canned.json).

tests/canned.json was recorded from the real files on 2026-09-25: for each shipped sample, the
branches `rowver check` read, with the types `rowver probe` gave, and the kind of each column.
Runs under pytest or `python -m unittest discover tests`; the presser is always this checkout.
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import awkward as ak
import numpy as np
import uproot

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import recipe  # noqa: E402

CANNED = json.loads((Path(__file__).resolve().parent / "canned.json").read_text(encoding="utf-8"))


def lines(stdout):
    """Every stdout line as JSON; fails the test if any line is not."""
    return [json.loads(line) for line in stdout.splitlines()]


class Folder(unittest.TestCase):
    """A fresh temporary folder per test."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="rowver-test-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)


class Probe(Folder):
    def setUp(self):
        super().setUp()
        self.file = self.root / "two.root"
        counts = np.tile(np.array([2, 0, 3, 1], dtype=np.int32), 50)
        muon = ak.zip({"pt": ak.unflatten(np.arange(counts.sum(), dtype=np.float32) + 0.5, counts),
                       "tightId": ak.unflatten(np.arange(counts.sum()) % 2 == 0, counts)})
        with uproot.recreate(self.file) as out:
            out.mktree("Events", {"Muon": muon.type.content, "MET_pt": np.float32, "run": np.int64,
                                  "HLT_IsoMu24": np.bool_, "fixed": np.dtype((np.float64, (3,)))})
            out["Events"].extend({"Muon": muon, "MET_pt": np.linspace(0.1, 20, 200).astype(np.float32),
                                  "run": np.arange(200) + 316000, "HLT_IsoMu24": np.arange(200) % 3 == 0,
                                  "fixed": np.ones((200, 3))})
            out["Text"] = {"name": ak.Array(["a", "bb", "c"]), "odd": ak.Array([np.nan, np.inf, 1.0])}

    def test_trees_branches_kinds_and_first_values(self):
        result = study.rowver("probe", self.file, "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        found = json.loads(result.stdout)
        self.assertEqual((found["event"], found["path"]), ("probe", str(self.file)))
        self.assertEqual(found["uuid"], str(uproot.open(self.file).file.uuid))
        trees = {tree["name"]: tree for tree in found["trees"]}
        self.assertEqual(list(trees), ["Events", "Text"])
        self.assertEqual(trees["Events"]["entries"], 200)
        branches = {b["name"]: (b["type"], b["kind"], b["first"]) for b in trees["Events"]["branches"]}
        self.assertEqual(branches, {
            "nMuon": ("int32_t", "flat", [2, 0, 3]),
            "Muon_pt": ("float[]", "jagged", [[0.5, 1.5], [], [2.5, 3.5, 4.5]]),
            "Muon_tightId": ("bool[]", "jagged", [[True, False], [], [True, False, True]]),
            "MET_pt": ("float", "flat", [0.1, 0.2, 0.3]),
            "run": ("int64_t", "flat", [316000, 316001, 316002]),
            "HLT_IsoMu24": ("bool", "flat", [True, False, False]),
            "fixed": ("double[3]", "jagged", [[1.0, 1.0, 1.0]] * 3),
        })
        text = {b["name"]: (b["kind"], b["first"]) for b in trees["Text"]["branches"]}
        self.assertEqual(text, {"name": ("unsupported", []), "odd": ("flat", ["nan", "inf", 1.0])})

    def test_one_tree(self):
        found = json.loads(study.rowver("probe", self.file, "--tree", "Text", "--json").stdout)
        self.assertEqual([tree["name"] for tree in found["trees"]], ["Text"])

    def test_a_tree_that_is_not_there(self):
        result = study.rowver("probe", self.file, "--tree", "Nope", "--json")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(lines(result.stdout), [{"event": "error", "kind": "input", "traceback": "",
                                                 "message": "This file has no tree named Nope. Its trees: Events, Text."}])

    def test_a_file_that_is_not_there(self):
        result = study.rowver("probe", self.root / "gone.root", "--json")
        self.assertEqual(result.returncode, 1)
        error = lines(result.stdout)[0]
        self.assertEqual((error["event"], error["kind"]), ("error", "input"))
        self.assertIn("gone.root", error["message"])

    def test_the_table(self):
        result = study.rowver("probe", self.file)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Events: 200 entries, 7 branches", result.stdout)
        self.assertIn("Text: 3 entries, 2 branches", result.stdout)
        self.assertRegex(result.stdout, r"Muon_pt +float\[\] +jagged +\[0\.5, 1\.5\], \[\], \[2\.5, 3\.5, 4\.5\]")
        self.assertTrue(all(len(line) < 100 for line in result.stdout.splitlines()[2:]))


class Check(Folder):
    def test_the_first_sample(self):
        folder = study.make(self.root)
        result = study.rowver("check", folder, "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        start, checked = lines(result.stdout)
        self.assertEqual(start, {"event": "start", "study": "tiny", "samples": ["alpha"], "workers": 1})
        self.assertEqual((checked["event"], checked["index"], checked["sample"]), ("checked", 0, "alpha"))
        self.assertEqual((checked["path"], checked["tree"]), (str(folder / "a.root"), "Events"))
        self.assertEqual((checked["entries"], checked["read"], checked["kept"]), (200, 200, 159))
        self.assertEqual(checked["touched"], ["MET", "Muon_pt", "nMuon", "q"])
        self.assertEqual([(c["name"], c["kind"], c["type"]) for c in checked["columns"]],
                         [("MET", "flat", "float32"), ("Nmuon", "flat", "int32"), ("lead_pt", "flat", "float32"),
                          ("Muon_pt", "jagged", "var * float32"), ("q", "flat", "int32")])
        self.assertEqual(len(checked["columns"][0]["first"]), 3)
        self.assertEqual(list(folder.rglob("__pycache__")), [])

    def test_one_named_sample_and_every_sample(self):
        folder = study.make(self.root)
        named = lines(study.rowver("check", folder, "--sample", "beta", "--json").stdout)
        self.assertEqual(named[0]["samples"], ["beta"])
        self.assertEqual((named[1]["index"], named[1]["sample"], named[1]["kept"]), (1, "beta", 98))
        for extra in ((), ("--parallel", "2")):
            with self.subTest(extra=extra):
                result = study.rowver("check", folder, "--all", "--json", *extra)
                self.assertEqual(result.returncode, 0, result.stderr)
                events = lines(result.stdout)
                self.assertEqual(events[0]["samples"], ["alpha", "beta"])
                self.assertEqual(sorted((e["index"], e["kept"]) for e in events[1:]), [(0, 159), (1, 98)])

    def test_a_stop_below_1000_limits_the_read(self):
        folder = study.make(self.root)
        text = (folder / "cart.py").read_text()
        (folder / "cart.py").write_text(text.replace(f"'{folder / 'a.root'}': 'Events'",
                                                     f"'{folder / 'a.root'}': {{'tree': 'Events', 'stop': 50}}"))
        checked = lines(study.rowver("check", folder, "--json").stdout)[1]
        self.assertEqual(checked["read"], 50)

    def test_a_failing_hook_is_a_recipe_error(self):
        folder = study.make(self.root)
        text = (folder / "cart.py").read_text()
        (folder / "cart.py").write_text(text.replace("return events.Muon_pt[:, 0]", "return events.nothing"))
        result = study.rowver("check", folder, "--all", "--json")
        self.assertEqual(result.returncode, 1)
        errors = lines(result.stdout)[1:]
        self.assertEqual([(e["event"], e["kind"], e["sample"]) for e in errors],
                         [("error", "recipe", "alpha"), ("error", "recipe", "beta")])
        self.assertTrue(errors[0]["message"].startswith("column lead_pt: "), errors[0]["message"])
        self.assertIn("cart.py", errors[0]["traceback"])

    def test_a_column_of_the_wrong_length_or_kind(self):
        folder = study.make(self.root)
        text = (folder / "cart.py").read_text()
        text = text.replace('"q": "q"}', '"q": "q", "short": short, "text": text}').replace(
            "\n\nFILESET", '\n\ndef short(events):\n    return events.MET[:10]\n\n\n'
                           'def text(events):\n    return ak.Array(["x"] * len(events))\n\n\nFILESET')
        (folder / "cart.py").write_text("import awkward as ak\n" + text)
        checked = lines(study.rowver("check", folder, "--json").stdout)[1]
        columns = {c["name"]: c for c in checked["columns"]}
        self.assertEqual((columns["short"]["kind"], columns["short"]["note"]),
                         ("unsupported", "10 values for 159 kept events"))
        self.assertEqual((columns["text"]["kind"], columns["text"]["first"]), ("unsupported", []))

    def test_a_file_that_cannot_be_read(self):
        folder = study.make(self.root, files={"alpha": [str(self.root / "gone.root")]})
        result = study.rowver("check", folder, "--json")
        self.assertEqual(result.returncode, 1)
        error = lines(result.stdout)[1]
        self.assertEqual((error["event"], error["kind"], error["sample"]), ("error", "input", "alpha"))
        self.assertIn("gone.root", error["message"])

    def test_a_recipe_that_does_not_load_and_a_sample_that_is_not_there(self):
        folder = study.make(self.root)
        result = study.rowver("check", folder, "--sample", "gamma", "--json")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(lines(result.stdout)[0]["message"],
                         "The recipe has no sample named gamma. Its samples: alpha, beta.")
        (folder / "cart.py").write_text("def broken(:\n")
        error = lines(study.rowver("check", folder, "--json").stdout)[0]
        self.assertEqual((error["event"], error["kind"]), ("error", "recipe"))
        self.assertTrue(error["message"].startswith("cart.py: SyntaxError"), error["message"])

    def test_the_table(self):
        folder = study.make(self.root)
        result = study.rowver("check", folder)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("alpha: a.root\n  Read 200 events, kept 159", result.stdout)
        self.assertIn("  Branches read: MET, Muon_pt, nMuon, q", result.stdout)
        self.assertRegex(result.stdout, r"Muon_pt +jagged +var \* float32")

    def test_ctrl_c_stops_a_parallel_check_at_once(self):
        folder = study.make(self.root, extra="import time")
        study.point(folder, {f"s{i}": [folder / "a.root"] for i in range(8)})
        text = (folder / "cart.py").read_text()
        (folder / "cart.py").write_text(text.replace('    """At least one muon."""\n',
                                                     '    """At least one muon."""\n    time.sleep(3)\n'))
        # started as a terminal starts it, with Ctrl-C at its default: a CI job's shell may start
        # its commands with SIGINT ignored, which they inherit, and then no Ctrl-C reaches them
        process = subprocess.Popen([sys.executable, "-m", "rowver", "check", str(folder), "--all", "--parallel", "2"],
                                   env=study.environment(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, process_group=0,
                                   preexec_fn=lambda: signal.signal(signal.SIGINT, signal.SIG_DFL))
        time.sleep(4)  # the workers have started and are inside a sample
        sent = time.monotonic()
        os.killpg(process.pid, signal.SIGINT)  # what Ctrl-C does in a terminal
        try:
            _, stderr = process.communicate(timeout=30)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
        self.assertLess(time.monotonic() - sent, 3, stderr)
        self.assertEqual(process.returncode, 1)
        self.assertIn("The check was cancelled.", stderr)
        with self.assertRaises(ProcessLookupError):  # no worker is left in the group
            os.killpg(process.pid, 0)


class ShippedRecipes(Folder):
    """Each shipped recipe, checked on local files holding the branches its real files gave it."""

    def test_each_sample_reads_the_same_branches_and_makes_the_same_kinds(self):
        for name, samples in CANNED.items():
            with self.subTest(study=name):
                folder = self.root / name
                folder.mkdir()
                source = (study.REPO / "studies" / name / "cart.py").read_text(encoding="utf-8")
                found = recipe.recipe(source, folder)
                self.assertEqual([sample for sample, _ in found["fileset"]], list(samples))
                for seed, (sample, entry) in enumerate(found["fileset"]):
                    [[path, tree]] = entry["files"]
                    self.assertEqual(path, samples[sample]["path"])
                    local = folder / f"{seed}.root"
                    study.nano(local, samples[sample]["branches"], seed=seed)
                    entry["files"] = [[str(local), tree]]
                (folder / "cart.py").write_text(found["hooks"] + recipe.region(found), encoding="utf-8")
                result = study.rowver("check", folder, "--all", "--json", "--parallel", "2")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                for line in lines(result.stdout)[1:]:
                    canned = samples[line["sample"]]
                    self.assertEqual(line["touched"], sorted(canned["branches"]), line["sample"])
                    self.assertEqual(line["read"], 1000)
                    self.assertGreater(line["kept"], 0, line["sample"])
                    self.assertEqual([(c["name"], c["kind"]) for c in line["columns"]],
                                     [(column, kind) for column, kind, _ in canned["columns"]])
                self.assertEqual(list(folder.rglob("__pycache__")), [])


if __name__ == "__main__":
    unittest.main()
