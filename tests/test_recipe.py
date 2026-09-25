"""rowver recipe: reading cart.py for the app's form without running it, on the shipped recipes,
a hand-written one shaped like the LPC study's, and the hard cases of docs/recipe-dialect.md.

Runs under pytest or `python -m unittest discover tests`; the presser is always this checkout.
"""
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import recipe, run  # noqa: E402

SHIPPED = ("simple", "data_and_mc", "dimuon_open_data", "advanced")
LABELLED = {"data_and_mc": {"jet_bin": [0, 1, 2]}, "dimuon_open_data": {"charge": [-2, 0, 2]},
            "advanced": {"era": [0, 1]}}
LPC = Path(__file__).resolve().parent / "hand_written_cart.txt"  # the shape of the LPC study's cart.py

HOOKS = '''"""A study written for the form."""
from rowver import Cart


def keep(events):
    """Every event."""
    return events.nMuon >= 0


'''

REGION = '''# ---- recipe ----
FILESET = {
    "2018": {
        "files": {
            "a.root": {"tree": "Events", "stop": 2400000},
        },
        "metadata": {"xsec": 6077.22, "era": -1, "tags": ["a", "b"], "data": True, "weight": None},
    },
    "2017": {
        "files": {
            "b.root": "Events",
        },
        "metadata": {},
    },
}

COLUMNS = {
    "q": "q",
    "Nmuon": "nMuon",
}

LABELS = {
    "q": {-1: "negative", 0: "none", 1: "positive", 2: "two"},
}

CART = Cart(
    fileset=FILESET,
    columns=COLUMNS,
    keep=keep,
    prepare=None,
    labels=LABELS,
    title="Say \\"hi\\" to μμ\\\\",
    note=None,
)
'''


def cli(folder):
    """rowver recipe --json on a folder, as the app runs it; the parsed object."""
    result = study.rowver("recipe", folder, "--json")
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


class Folder(unittest.TestCase):
    """A fresh temporary folder per test, and a way to write a cart.py into it."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="rowver-test-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)

    def write(self, text, name="study"):
        folder = self.root / name
        folder.mkdir(exist_ok=True)
        (folder / "cart.py").write_text(text, encoding="utf-8")
        return folder

    def outside(self, text):
        """The reason a file leaves the dialect; fails if it does not."""
        found = cli(self.write(text))
        self.assertFalse(found["dialect"], found)
        return found["reason"]


class Shipped(unittest.TestCase):
    """The four shipped recipes are in the dialect, printed exactly as the form prints them."""

    def test_each_is_in_the_dialect_and_prints_back_to_itself(self):
        for name in SHIPPED:
            with self.subTest(study=name):
                folder = study.REPO / "studies" / name
                found = cli(folder)
                self.assertTrue(found["dialect"], found.get("reason"))
                self.assertTrue(found["canonical"])
                text = (folder / "cart.py").read_text(encoding="utf-8")
                self.assertTrue(text.startswith(found["hooks"]))
                self.assertEqual(found["hooks"] + recipe.region(found), text)
                self.assertEqual((found["siblings"], found["missing"]), ([], []))

    def test_what_it_reads_is_what_python_runs(self):
        """The reader agrees with importing the file: the fileset, the columns, the hooks, the labels."""
        for name in SHIPPED:
            with self.subTest(study=name):
                folder = study.REPO / "studies" / name
                found, cart = cli(folder), run.load(folder)
                fileset = {sample: {"files": dict(entry["files"]), "metadata": dict(entry["metadata"])}
                           for sample, entry in found["fileset"]}
                self.assertEqual(fileset, cart.fileset)
                self.assertEqual([column for column, _ in found["columns"]], list(cart.columns))
                for column, value in found["columns"]:
                    spec = cart.columns[column]
                    self.assertEqual(value, {"branch": spec} if isinstance(spec, str) else {"hook": spec.__name__})
                self.assertEqual({column: dict(pairs) for column, pairs in found["labels"]}, cart.labels)
                self.assertEqual(found["cart"]["keep"], cart.keep.__name__)
                self.assertEqual(found["cart"]["prepare"], cart.prepare and cart.prepare.__name__)
                self.assertEqual((found["cart"]["title"], found["cart"]["note"]), (cart.title, cart.note))

    def test_label_values_are_json_integers(self):
        for name, expected in LABELLED.items():
            with self.subTest(study=name):
                labels = cli(study.REPO / "studies" / name)["labels"]
                self.assertEqual([column for column, _ in labels], list(expected))
                for column, pairs in labels:
                    self.assertEqual([value for value, _ in pairs], expected[column])
                    self.assertTrue(all(type(value) is int for value, _ in pairs))

    def test_reading_leaves_no_bytecode(self):
        for name in SHIPPED:
            cli(study.REPO / "studies" / name)
            self.assertFalse((study.REPO / "studies" / name / "__pycache__").exists())


class HandWritten(Folder):
    def test_a_hand_written_recipe_is_text_and_names_its_sibling_imports(self):
        folder = self.write(LPC.read_text(encoding="utf-8"), "lpc_gg")
        found = cli(folder)
        self.assertEqual(found, {"dialect": False, "reason": found["reason"],
                                 "siblings": ["fileset", "helpers"], "missing": ["fileset", "helpers"]})
        self.assertIn("recipe ----", found["reason"])
        (folder / "fileset.py").write_text("FILESET = {}\n")
        (folder / "helpers.py").write_text("")
        self.assertEqual(cli(folder)["missing"], [])

    def test_a_sibling_import_in_the_dialect(self):
        folder = self.write(HOOKS.replace("from rowver import Cart\n", "from rowver import Cart\nfrom cuts import keep\n")
                            .replace('def keep(events):\n    """Every event."""\n    return events.nMuon >= 0\n', "")
                            + REGION)
        found = cli(folder)
        self.assertTrue(found["dialect"], found.get("reason"))
        self.assertEqual((found["siblings"], found["missing"], found["cart"]["keep"]), (["cuts"], ["cuts"], "keep"))
        self.assertEqual(found["functions"], [])
        self.assertIn("keep", found["bound"])

    def test_a_sibling_imported_inside_a_def_or_a_try(self):
        hooks = HOOKS.replace('    """Every event."""\n', '    """Every event."""\n    from cuts import floor\n')
        folder = self.write("try:\n    import tables\nexcept ImportError:\n    pass\n" + hooks + REGION)
        found = cli(folder)
        self.assertEqual((found["siblings"], found["missing"]), (["tables", "cuts"], ["tables", "cuts"]))

    def test_the_file_is_never_run(self):
        folder = self.write('open("ran", "w").close()\nraise SystemExit(3)\n' + HOOKS + REGION)
        found = cli(folder)
        self.assertTrue(found["dialect"], found.get("reason"))
        self.assertFalse((folder / "ran").exists())
        self.assertFalse(Path("ran").exists())

    def test_a_syntax_error_names_its_line(self):
        found = cli(self.write(HOOKS + REGION.replace("COLUMNS = {", "COLUMNS = {{")))
        self.assertFalse(found["dialect"])
        self.assertEqual(found["line"], 26)
        self.assertTrue(found["reason"].startswith("cart.py does not parse: "), found["reason"])


class HardCases(Folder):
    def test_the_printed_layout_round_trips(self):
        found = cli(self.write(HOOKS + REGION))
        self.assertTrue(found["dialect"], found.get("reason"))
        self.assertTrue(found["canonical"])
        self.assertEqual(found["cart"]["title"], 'Say "hi" to μμ\\')
        self.assertEqual([sample for sample, _ in found["fileset"]], ["2018", "2017"])
        self.assertEqual(dict(found["fileset"][0][1]["metadata"]),
                         {"xsec": 6077.22, "era": -1, "tags": ["a", "b"], "data": True, "weight": None})

    def test_underscores_in_a_number(self):
        folder = self.write(HOOKS + REGION.replace("2400000", "2_400_000"))
        found = cli(folder)
        self.assertTrue(found["dialect"], found.get("reason"))
        self.assertEqual(found["fileset"][0][1]["files"], [["a.root", {"tree": "Events", "stop": 2400000}]])
        self.assertFalse(found["canonical"])
        self.assertEqual(found["hooks"] + recipe.region(found), HOOKS + REGION)

    def test_keys_in_another_order(self):
        text = HOOKS + REGION.replace('{"tree": "Events", "stop": 2400000}', '{"stop": 2400000, "tree": "Events"}')
        found = cli(self.write(text))
        self.assertTrue(found["dialect"], found.get("reason"))
        self.assertFalse(found["canonical"])
        self.assertEqual(found["hooks"] + recipe.region(found), HOOKS + REGION)

    def test_negative_and_zero_label_keys_are_integers(self):
        found = cli(self.write(HOOKS + REGION))
        self.assertEqual(found["labels"], [["q", [[-1, "negative"], [0, "none"], [1, "positive"], [2, "two"]]]])
        self.assertTrue(all(type(value) is int for value, _ in found["labels"][0][1]))

    def test_an_f_string(self):
        reason = self.outside(HOOKS + REGION.replace('"b.root": "Events"', 'f"{DAS}b.root": "Events"'))
        self.assertIn("f-string", reason)
        self.assertIn("(line 20)", reason)

    def test_a_text_label_key(self):
        reason = self.outside(HOOKS + REGION.replace('-1: "negative"', '"-1": "negative"'))
        self.assertIn("label value of 'q' is not a whole number (line 32)", reason)

    def test_a_bool_or_float_label_key(self):
        self.outside(HOOKS + REGION.replace('0: "none"', 'False: "none"'))
        self.outside(HOOKS + REGION.replace('0: "none"', '0.5: "none"'))

    def test_what_else_leaves_the_dialect(self):
        cases = {
            "a lambda column": ('"q": "q"', '"q": lambda events: events.q'),
            "an unknown hook": ('"q": "q"', '"q": nowhere'),
            "a repeated key": ('"Nmuon": "nMuon"', '"q": "nMuon"'),
            "a nested metadata dict": ('"data": True', '"data": {"a": 1}'),
            "a tuple": ('["a", "b"]', '("a", "b")'),
            "an extra statement": ("CART = Cart(", "X = 1\nCART = Cart("),
            "a positional argument": ("    fileset=FILESET,\n", "    FILESET,\n"),
            "a stop of 0": ("2400000", "0"),
            "a file entry without a stop": ('{"tree": "Events", "stop": 2400000}', '{"tree": "Events"}'),
            "LABELS not passed": ("labels=LABELS", "labels=None"),
            "a missing keep": ("    keep=keep,\n", ""),
            "a label for no column": ('"q": {-1', '"x": {-1'),
        }
        for what, (old, new) in cases.items():
            with self.subTest(case=what):
                self.assertIn(old, HOOKS + REGION)
                self.outside(HOOKS + REGION.replace(old, new, 1))
        self.outside(HOOKS.replace("from rowver import Cart\n", "") + REGION)
        self.outside(HOOKS + REGION + REGION)
        self.outside(HOOKS + REGION.replace("# ---- recipe ----\n", ""))

    def test_the_new_study_skeleton(self):
        """WP7's starter cart.py: an empty FILESET and COLUMNS, printed as {}."""
        text = HOOKS + ('# ---- recipe ----\nFILESET = {}\n\nCOLUMNS = {}\n\nCART = Cart(\n    fileset=FILESET,\n'
                        '    columns=COLUMNS,\n    keep=keep,\n    prepare=None,\n    labels=None,\n'
                        '    title="my_study",\n    note=None,\n)\n')
        found = cli(self.write(text))
        self.assertTrue(found["dialect"], found.get("reason"))
        self.assertTrue(found["canonical"])
        self.assertEqual((found["fileset"], found["columns"], found["labels"]), ([], [], []))

    def test_control_characters_and_windows_line_ends(self):
        found = recipe.recipe(HOOKS + REGION, Path("."))
        found["cart"]["note"] = "tab\there\nnew line\x07 bell \x85 and ☃"
        text = found["hooks"] + recipe.region(found)
        self.assertIn(r'note="tab\there\nnew line\x07 bell \x85 and ☃"', text)
        folder = self.write(text.replace("\n", "\r\n"))
        again = cli(folder)
        self.assertTrue(again["canonical"])
        self.assertEqual(again["cart"]["note"], found["cart"]["note"])

    def test_the_human_summary(self):
        result = study.rowver("recipe", study.REPO / "studies" / "dimuon_open_data")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 sample, 1 file, 6 columns", result.stdout)
        folder = self.write(LPC.read_text(encoding="utf-8"), "lpc_gg")
        result = study.rowver("recipe", folder)
        self.assertIn("Imports from its folder: fileset, helpers", result.stdout)

    def test_a_missing_file(self):
        result = study.rowver("recipe", self.root / "nowhere", "--json")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)["kind"], "input")


if __name__ == "__main__":
    unittest.main()
