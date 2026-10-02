"""rowver run --onto: the recipe's new columns added to a cartridge already pressed, the columns it
holds left as the bytes they are, and every case where nothing may be added refused."""
import json
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import study  # noqa: E402
from test_press import Folder, lines  # noqa: E402

sys.path.insert(0, str(study.REPO))
from rowver import rvr  # noqa: E402

MORE = ('"lead_pt": lead, "twice": lambda e: e.MET * 2, "Muon_pt2": "Muon_pt",', '"q": "q", "n2": "nMuon"}')


def edit(folder, *pairs):
    """Replace text in a study's recipe."""
    path = Path(folder) / "cart.py"
    text = path.read_text(encoding="utf-8")
    for old, new in pairs:
        assert old in text, old
        text = text.replace(old, new)
    path.write_text(text, encoding="utf-8")


def more(folder):
    """Give the recipe three more columns: two among the ones it has, one last."""
    edit(folder, ('"lead_pt": lead,', MORE[0]), ('"q": "q"}', MORE[1]))


class Extend(Folder):
    def pressed(self):
        """A study, and the cartridge pressed from it."""
        folder = study.make(self.root)
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        return folder, folder / "tiny.rvr"

    def refused(self, folder, onto, *args):
        """The message of an add that stops as differs, having written nothing."""
        out = folder / "added.rvr"
        result = study.rowver("run", folder, "--onto", onto, "--out", out, "--progress", "json", *args)
        self.assertEqual(result.returncode, 1, result.stdout)
        last = lines(result.stdout)[-1]
        self.assertEqual((last["event"], last["kind"]), ("error", "differs"), last)
        self.assertFalse(out.exists())
        return last["message"]

    def test_new_columns_join_the_ones_held_as_a_full_press_lays_them(self):
        folder, first = self.pressed()
        before, section = rvr.read(first)
        more(folder)
        out = folder / ".rowver" / "pressing.rvr"
        # asked first, the presser names the columns and reads nothing
        asked = study.rowver("run", folder, "--onto", first, "--out", out, "--plan", "--progress", "json")
        self.assertEqual((asked.returncode, lines(asked.stdout)), (0, [
            {"event": "plan", "columns": ["twice", "Muon_pt2", "n2"], "files": 2, "skipped": []}]))
        self.assertFalse(out.exists())
        for workers in ("1", "2"):
            result = study.rowver("run", folder, "--onto", first, "--out", out, "--progress", "json",
                                  "--parallel", workers)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            wrote = lines(result.stdout)[-1]
            self.assertEqual((wrote["event"], wrote["columns"], wrote["added"], wrote["failed"]),
                             ("wrote", 8, ["twice", "Muon_pt2", "n2"], []))
            self.assertEqual(study.rowver("validate", out).returncode, 0)
            header, added = rvr.read(out)
            self.assertEqual([c["name"] for c in header["data"]["cols"]],
                             ["MET", "Nmuon", "lead_pt", "twice", "Muon_pt2", "Muon_pt", "q", "n2"])
            # a column held is the bytes it was, wherever it now sits
            for old in before["data"]["cols"]:
                new = next(c for c in header["data"]["cols"] if c["name"] == old["name"])
                for a, b in ((old, new),) if old["kind"] != "jagged" else ((old["offsets"], new["offsets"]),
                                                                           (old["content"], new["content"])):
                    self.assertEqual(bytes(section[a["at"]:a["at"] + a["size"]]),
                                     bytes(added[b["at"]:b["at"] + b["size"]]))
            self.assertTrue(np.allclose(rvr.column(header, added, "twice"), 2 * rvr.column(header, added, "MET")))
            self.assertEqual(header["made"]["cart"]["source"], (folder / "cart.py").read_text(encoding="utf-8"))
            self.assertEqual(sorted(header["made"]["cart"]["hooks"]["columns"]), sorted(c["name"] for c in header["data"]["cols"]))
        # the same data section as pressing the new recipe whole
        whole = folder / "whole.rvr"
        self.assertEqual(study.rowver("run", folder, "--out", whole).returncode, 0)
        self.assertEqual(rvr.read(whole)[0]["data"], header["data"])

    def test_a_view_in_the_cartridge_comes_along_and_labels_follow_the_recipe(self):
        folder, first = self.pressed()
        header, section = rvr.read(first)
        header["rowver_web"] = {"kept": "as it is"}
        rvr.write(first, header, [bytes(section)])
        more(folder)
        edit(folder, ('-1: "negative"', '-1: "minus"'))
        out = folder / "added.rvr"
        self.assertEqual(study.rowver("run", folder, "--onto", first, "--out", out).returncode, 0)
        added = rvr.read_header(out)
        self.assertEqual(added["rowver_web"], {"kept": "as it is"})
        self.assertEqual(next(c for c in added["data"]["cols"] if c["name"] == "q")["names"],
                         {"-1": "minus", "1": "positive"})

    def test_a_changed_keep_or_column_is_refused_and_a_comment_or_a_new_column_beside_a_lambda_is_not(self):
        folder, first = self.pressed()
        more(folder)
        added = folder / "tiny2.rvr"
        self.assertEqual(study.rowver("run", folder, "--onto", first, "--out", added).returncode, 0)
        # a comment in keep, and one more column on the line the lambda is written on, change no mark
        edit(folder, ('    return events.nMuon >= 1', '    # still one muon\n    return events.nMuon >= 1'),
             ('"n2": "nMuon"}', '"n2": "nMuon", "n3": "nMuon"}'))
        self.assertEqual(study.rowver("run", folder, "--onto", added, "--out", folder / "tiny3.rvr").returncode, 0)
        edit(folder, ("events.nMuon >= 1", "events.nMuon >= 2"), ("e.MET * 2", "e.MET * 3"),
             ('"n3": "nMuon"}', '"n3": "nMuon", "n4": "nMuon"}'))
        self.assertEqual(self.refused(folder, added), "keep and the column twice changed since the cartridge "
                         "was pressed. Press again to make the cartridge anew.")

    def test_a_mark_follows_what_a_hook_names_wraps_and_closes_over(self):
        folder = study.make(self.root, extra="""
import functools

SCALE = 2


def helper(x):
    return x * SCALE


def pick(name, events):
    return events[name]


def loud(function):
    @functools.wraps(function)
    def wrapper(events):
        return function(events)
    return wrapper


def scaled(k):
    return lambda events: events.MET * k


@loud
def wrapped(events):
    return helper(events.MET)


def nests(events):
    def inner(x):
        return x
    return inner(events.MET) * 2
""")
        edit(folder, ('"q": "q"}', '"q": "q", "w": wrapped, "s": scaled(2), "n": nests, '
                                   '"p": functools.partial(pick, "MET"), "m": lambda e, metadata: e.MET}'))
        self.assertEqual(study.rowver("run", folder).returncode, 0)
        first, new = folder / "tiny.rvr", ('"m": lambda e, metadata: e.MET}', '"m": lambda e, metadata: e.MET, "z": "MET"}')
        edit(folder, new)
        self.assertEqual(study.rowver("run", folder, "--onto", first, "--out", folder / "added.rvr").returncode, 0)
        (folder / "added.rvr").unlink()
        for old, changed, named in (("SCALE = 2", "SCALE = 3", "the column w"),
                                    ("return events[name]", "return events[name] * 1", "the column p"),
                                    ("return helper(events.MET)", "return helper(events.MET) + 1", "the column w"),
                                    ("scaled(2)", "scaled(3)", "the column s"),
                                    ("return inner(events.MET) * 2", "return inner(events.MET) * 3", "the column n"),
                                    ("'metadata': {}", "'metadata': {'xsec': 2}", "the samples' metadata")):
            edit(folder, (old, changed))
            self.assertEqual(self.refused(folder, first),
                             f"{named} changed since the cartridge was pressed. Press again to make the cartridge anew.")
            edit(folder, (changed, old))

    def test_an_add_over_the_cartridge_itself_leaves_it_whole_when_the_write_fails(self):
        folder, first = self.pressed()
        before = first.read_bytes()
        more(folder)
        (folder / "tiny.rvr.adding").mkdir()  # the write cannot start
        result = study.rowver("run", folder, "--onto", first, "--progress", "json")
        self.assertEqual((result.returncode, lines(result.stdout)[-1]["kind"]), (1, "write"))
        self.assertEqual(first.read_bytes(), before)
        (folder / "tiny.rvr.adding").rmdir()
        self.assertEqual(study.rowver("run", folder, "--onto", first).returncode, 0)
        self.assertEqual(len(rvr.read_header(first)["data"]["cols"]), 8)
        self.assertEqual([p.name for p in folder.glob("*.adding")], [])

    def test_a_cartridge_with_no_marks_is_proven_by_its_entry_numbers(self):
        folder, first = self.pressed()
        header, section = rvr.read(first)
        del header["made"]["cart"]["hooks"]
        header["made"]["sha256"] = rvr.made_checksum(header["made"])
        rvr.write(first, header, [bytes(section)])
        more(folder)
        self.assertEqual(study.rowver("run", folder, "--onto", first, "--out", folder / "added.rvr").returncode, 0)
        self.assertNotIn("hooks", rvr.read_header(folder / "added.rvr")["made"]["cart"])
        (folder / "added.rvr").unlink()
        edit(folder, ("events.nMuon >= 1", "events.nMuon >= 2"))
        self.assertIn("keep and prepare now choose other events in", self.refused(folder, first))

    def test_nothing_new_a_column_gone_or_other_files_are_refused(self):
        folder, first = self.pressed()
        nothing = lines(study.rowver("run", folder, "--onto", first, "--plan", "--progress", "json").stdout)[-1]
        self.assertEqual((nothing["kind"], nothing["message"]),
                         ("nothing", "The cartridge already holds every column of the recipe."))
        edit(folder, ('"Nmuon": "nMuon", ', ""), ('"q": "q"}', '"q": "q", "n2": "nMuon"}'))
        self.assertIn("The recipe no longer makes Nmuon, which the cartridge holds.", self.refused(folder, first))
        edit(folder, ('"MET": "MET", ', '"MET": "MET", "Nmuon": "nMuon", '))
        study.tree(folder / "a.root", 200, 7)
        self.assertIn("a.root is not the file the cartridge was pressed from.", self.refused(folder, first))

    def test_a_file_the_cartridge_was_not_pressed_from_is_named_and_not_read(self):
        folder, first = self.pressed()
        study.tree(folder / "c.root", 50, 3)
        study.point(folder, {"alpha": [folder / "a.root"], "beta": [folder / "b.root", folder / "c.root"]})
        more(folder)
        result = study.rowver("run", folder, "--onto", first, "--out", folder / "added.rvr", "--progress", "json")
        self.assertEqual(result.returncode, 0, result.stdout)
        events = lines(result.stdout)
        self.assertEqual(events[0]["files"], 2)
        self.assertEqual([(f["sample"], Path(f["path"]).name) for f in events[-1]["failed"]], [("beta", "c.root")])
        self.assertEqual(rvr.read_header(folder / "added.rvr")["data"]["rows"], 257)


if __name__ == "__main__":
    unittest.main()
