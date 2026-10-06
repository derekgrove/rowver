"""rowver.derive: a file of only the columns made from another file's columns, with the parent it
came from recorded under its checksums, and what `rowver validate` and `--onto` make of it."""
import hashlib
import sys
import unittest
from pathlib import Path

import awkward as ak
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402
from test_extend import edit, more  # noqa: E402
from test_press import Folder, lines  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import extend, rvr  # noqa: E402
from rowver.derive import Refused, derive, column_mark as written  # noqa: E402

SOURCE = "def double(columns):\n    return columns['MET'] * FACTOR\n"
FACTOR = 2


def double(columns):
    """Twice the MET."""
    return columns["MET"] * FACTOR


def soft(columns):
    """Each event's muons below 30."""
    muons = columns["Muon_pt"]
    return muons[muons < 30]


def many(columns):
    """Whether the event has two muons or more."""
    return columns["Nmuon"] >= 2


class Derive(Folder):
    def setUp(self):
        super().setUp()
        self.folder = study.make(self.root)
        self.assertEqual(study.rowver("run", self.folder).returncode, 0)
        self.parent, self.out = self.folder / "tiny.rvr", self.folder / "derived.rvr"

    def derived(self, **more):
        """The parent's derived file of three columns; its header."""
        return derive(self.parent, self.out, SOURCE, {"MET2": double, "soft": soft, "many": many},
                      by="test 1.0", parameters={"factor": FACTOR, "cuts": [30, None]},
                      labels={"many": {0: "one", 1: "two or more"}}, **more)

    def fails(self, result, *names):
        """Assert validate failed on exactly the checks that start with these names."""
        self.assertEqual(result.returncode, 1, result.stdout)
        failed = [line.strip()[5:] for line in result.stdout.splitlines() if line.startswith("  FAIL")]
        self.assertEqual(len(failed), len(names), result.stdout)
        for line, name in zip(failed, names):
            self.assertTrue(line.startswith(name), (line, name))

    def test_the_file_holds_only_the_derived_columns_and_names_its_parent(self):
        written = self.derived()
        above, held = rvr.read(self.parent)
        header, section = rvr.read(self.out)
        self.assertEqual(header, written)
        self.assertEqual(list(header), ["rowver", "data", "made", "derived"])
        data, old = header["data"], above["data"]
        self.assertEqual([(c["name"], c["kind"]) for c in data["cols"]],
                         [("MET2", "f4"), ("soft", "jagged"), ("many", "b1")])
        self.assertEqual([data[key] for key in ("rows", "samples", "files")],
                         [old[key] for key in ("rows", "samples", "files")])
        for key in ("sample", "entry"):
            self.assertEqual(data[key]["sha256"], old[key]["sha256"])
        self.assertEqual(data["entry"]["by_sample"], old["entry"]["by_sample"])
        self.assertEqual(data["sha256"], rvr.data_checksum(data))
        self.assertNotEqual(data["sha256"], old["sha256"])
        self.assertEqual(header["made"], above["made"])
        self.assertTrue(np.array_equal(rvr.column(header, section, "MET2"), 2 * rvr.column(above, held, "MET")))
        muons = rvr.column(above, held, "Muon_pt")
        self.assertEqual(rvr.column(header, section, "soft").tolist(), muons[muons < 30].tolist())
        self.assertEqual(data["cols"][2]["names"], {"0": "one", "1": "two or more"})
        self.assertEqual(len(data["cols"][1]["content"]["by_sample"]), 2)
        derived = header["derived"]
        self.assertEqual(list(derived), ["sha256", "built", "by", "parents", "source", "columns", "parameters", "runtime"])
        self.assertEqual(derived["sha256"], rvr.made_checksum(derived))
        self.assertRegex(derived["built"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertEqual((derived["by"], derived["source"]), ("test 1.0", SOURCE))
        self.assertEqual(derived["parameters"], {"factor": 2, "cuts": [30, None]})
        self.assertRegex(derived["runtime"], r"^Python [\d.]+\S* / NumPy \S+ / Awkward \S+$")
        self.assertEqual(derived["columns"], {"MET2": extend.mark(double), "soft": extend.mark(soft),
                                              "many": extend.mark(many)})
        jagged = next(c for c in old["cols"] if c["name"] == "Muon_pt")
        both = hashlib.sha256((jagged["offsets"]["sha256"] + jagged["content"]["sha256"]).encode()).hexdigest()
        self.assertEqual(derived["parents"], [{
            "data_sha256": old["sha256"], "made_sha256": above["made"]["sha256"], "derived_sha256": None,
            "rows": 257, "path": str(self.parent),
            "read": {"MET": next(c for c in old["cols"] if c["name"] == "MET")["sha256"], "Muon_pt": both,
                     "Nmuon": next(c for c in old["cols"] if c["name"] == "Nmuon")["sha256"]}}])
        self.assertEqual([p.name for p in self.folder.glob("*.deriving")], [])

    def test_validate_without_and_with_its_parent_and_dump(self):
        self.derived()
        alone = study.rowver("validate", self.out)
        self.assertEqual(alone.returncode, 0, alone.stdout)
        self.assertIn("  --   the parent was not checked (name it with --parent)\n", alone.stdout)
        self.assertIn("ok   derived columns name exactly the columns, in order", alone.stdout)
        self.assertNotIn("cart hooks mark keep and exactly the columns", alone.stdout)
        both = study.rowver("validate", self.out, "--parent", self.parent)
        self.assertEqual(both.returncode, 0, both.stdout)
        self.assertEqual(both.stdout.count("  ok   parent: "), 7)
        self.assertNotIn("was not checked", both.stdout)
        dumped = study.rowver("dump", self.out).stdout
        self.assertIn('"derived": {', dumped)
        self.assertIn("##### begin derived source #####\n" + SOURCE + "##### end derived source #####", dumped)
        self.assertIn("soft           jagged[257]", dumped)

    def test_another_parent_is_caught(self):
        self.derived()
        # the recipe pressed again with another MET: a valid file, and not the parent
        edit(self.folder, ('"MET": "MET"', '"MET": lambda e: e.MET * 3'))
        other = self.folder / "other.rvr"
        self.assertEqual(study.rowver("run", self.folder, "--out", other).returncode, 0)
        self.fails(study.rowver("validate", self.out, "--parent", other), "parent: data sha256", "parent: made is",
                   "parent: every column read")
        # the parent with one array's checksum changed in its header: not valid itself, and not what was read
        header, section = rvr.read(self.parent)
        header["data"]["cols"][0]["sha256"] = "0" * 64
        rvr.write(self.parent, header, [bytes(section)])
        self.fails(study.rowver("validate", self.out, "--parent", self.parent),
                   "MET: decompresses", "data sha256 matches", "parent: every column read")
        self.fails(study.rowver("validate", self.out, "--parent", self.folder / "missing.rvr"),
                   "a rowver 13 file", "parent")
        with self.assertRaisesRegex(Refused, "The parent's header does not match its own checksums"):
            derive(self.parent, self.folder / "again.rvr", SOURCE, {"MET2": double}, by="test")
        self.assertFalse((self.folder / "again.rvr").exists())

    def test_a_changed_derived_section_is_caught(self):
        self.derived()
        good, section = rvr.read(self.out)

        def changed(change):
            """Validate the file with its header changed."""
            header, _ = rvr.read(self.out)
            change(header)
            rvr.write(self.folder / "changed.rvr", header, [bytes(section)])
            return study.rowver("validate", self.folder / "changed.rvr", "--parent", self.parent)

        self.fails(changed(lambda h: h["derived"].update(source="def double(columns): return 0\n")),
                   "derived sha256 matches")

        def renamed(header):
            """Name the columns in another order, under a checksum that is right."""
            columns = header["derived"]["columns"]
            header["derived"]["columns"] = {name: columns[name] for name in ("soft", "MET2", "many")}
            header["derived"]["sha256"] = rvr.made_checksum(header["derived"])

        self.fails(changed(renamed), "derived columns name exactly the columns")

        def parents(header):
            """Record two parents."""
            header["derived"]["parents"] *= 2
            header["derived"]["sha256"] = rvr.made_checksum(header["derived"])

        self.fails(changed(parents), "derived has sha256")

        def remade(header):
            """Give the file another made, whole in itself."""
            header["made"]["study"] = "other"
            header["made"]["sha256"] = rvr.made_checksum(header["made"])

        self.fails(changed(remade), "made and rows are the ones derived records", "parent: made is")
        self.fails(changed(lambda h: h["derived"]["parents"][0].update(rows=3)),
                   "made and rows are the ones derived records", "derived sha256 matches", "parent: rows")
        self.assertEqual(rvr.read(self.out)[0], good)

    def test_what_is_refused_writes_nothing(self):
        cases = (({}, "no column to derive"),
                 ({"MET": double}, "The parent already holds MET"),
                 ({"short": lambda c: c["MET"][:10]}, "short has 10 entries; the parent has 257 rows"),
                 ({"text": lambda c: ak.Array(["x"] * 257)}, "neither one number per row"),
                 ({"square": lambda c: np.zeros((257, 2))}, "square has 2 dimensions"),
                 ({"ratio": lambda c: c["MET"] / 0}, "ratio holds a value that is not finite"),
                 ({"half": lambda c: c["MET"].astype(np.float16)}, "kind float16"))
        for columns, message in cases:
            with self.assertRaisesRegex(Refused, message), np.errstate(divide="ignore"):
                derive(self.parent, self.out, SOURCE, columns, by="test")
        with self.assertRaisesRegex(Refused, "not plain JSON"):
            derive(self.parent, self.out, SOURCE, {"MET2": double}, by="test", parameters={"x": object()})
        with self.assertRaises(KeyError):
            derive(self.parent, self.out, SOURCE, {"x": lambda c: c["nothing"]}, by="test")
        self.assertEqual(sorted(p.name for p in self.folder.iterdir()), ["a.root", "b.root", "cart.py", "tiny.rvr"])
        # a file already there is left as it is, unless replacing it was asked for
        self.derived()
        before = self.out.read_bytes()
        with self.assertRaises(FileExistsError):
            derive(self.parent, self.out, SOURCE, {"other": double}, by="test")
        with self.assertRaisesRegex(Refused, "The target is the parent itself"):
            derive(self.parent, self.parent, SOURCE, {"other": double}, by="test", replace=True)
        self.assertEqual(self.out.read_bytes(), before)
        replaced = derive(self.parent, self.out, SOURCE, {"other": double}, by="test", replace=True)
        self.assertEqual(list(replaced["derived"]["columns"]), ["other"])
        self.assertEqual([p.name for p in self.folder.glob("*.deriving")], [])

    def test_a_derived_parent_offers_only_its_own_columns(self):
        first = self.derived()
        again = self.folder / "again.rvr"
        with self.assertRaisesRegex(KeyError, "MET is not a column the parent itself holds"):
            derive(self.out, again, "", {"MET4": lambda columns: columns["MET"] * 4}, by="test")
        header = derive(self.out, again, "", {"MET4": lambda columns: columns["MET2"] * 2}, by="test")
        parent = header["derived"]["parents"][0]
        self.assertEqual(parent["derived_sha256"], first["derived"]["sha256"])
        self.assertEqual(parent["read"], {"MET2": first["data"]["cols"][0]["sha256"]})
        self.assertEqual(header["made"], first["made"])
        self.assertEqual(study.rowver("validate", again, "--parent", self.out).returncode, 0)
        # its parent's parent is not its parent
        self.fails(study.rowver("validate", again, "--parent", self.parent), "parent: data sha256",
                   "parent: derived sha256", "parent: every column read")

    def test_onto_refuses_a_derived_file(self):
        self.derived()
        before = self.out.read_bytes()
        more(self.folder)
        for extra in ((), ("--plan",)):
            result = study.rowver("run", self.folder, "--onto", self.out, "--out", self.folder / "added.rvr",
                                  "--progress", "json", *extra)
            last = lines(result.stdout)[-1]
            self.assertEqual((result.returncode, last["event"], last["kind"]), (1, "error", "differs"))
            self.assertEqual(last["message"], "The cartridge holds columns derived from another cartridge, not "
                             "pressed from a recipe, so nothing can be added to it.")
        self.assertFalse((self.folder / "added.rvr").exists())
        self.assertEqual(self.out.read_bytes(), before)


class Marks(unittest.TestCase):
    """A column made by code compiled from the saved source is marked by that code."""

    def test_a_changed_definition_changes_the_mark_and_an_unchanged_one_does_not(self):
        """The same def gives the same mark whatever surrounds it; another body gives another."""
        def compiled(text):
            scope = {}
            exec(text, scope)
            return scope["mass"]
        first = "def mass(columns):\n    return columns['a'] + STEP\n"
        other = "STEP = 2\n" + first
        self.assertEqual(written(compiled("STEP = 1\n" + first), first), written(compiled("STEP = 1\n" + first), first))
        self.assertNotEqual(written(compiled("STEP = 1\n" + first), "STEP = 1\n" + first), written(compiled(other), other))
        self.assertEqual(written(eval("lambda c: c['a']"), first), hashlib.sha256((first + "\n<lambda>").encode()).hexdigest())


if __name__ == "__main__":
    unittest.main()
