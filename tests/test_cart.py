from __future__ import annotations

import tempfile
import textwrap
import unittest
from pathlib import Path

import awkward as ak
import numpy as np
import uproot

from rowver import rvr6
from rowver import Cart, CartError


class CartTests(unittest.TestCase):
    def test_every_sample_must_declare_weights_even_when_all_are_silent(self):
        cart = Cart(fileset={"forgotten mc": {"files": {"x.root": "Events"},
                                                  "metadata": {}}},
                    keep=lambda events: events.x > 0, columns={"x": "x"})
        with self.assertRaisesRegex(CartError, "explicitly set"):
            cart.validate()

    def test_copied_directory_names_the_output_and_builds_from_one_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ntuple = root / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {
                    "pt": np.array([5.0, 15.0, 25.0, 35.0]),
                    "njet": np.array([0, 1, 2, 3], dtype=np.int32),
                    "genWeight": np.array([0.5, 1.0, 2.0, 4.0]),
                }
            study = root / "studies" / "mine"
            study.mkdir(parents=True)
            (study / "cart.py").write_text(textwrap.dedent(f"""
                from cart import Cart

                def keep(events, metadata):
                    return events.pt >= metadata["threshold"]

                CART = Cart(
                    fileset={{
                        "simulation": {{
                            "files": {{{str(ntuple)!r}: "Events"}},
                            "metadata": {{"threshold": 10, "weights": "genWeight"}},
                        }},
                        "data": {{
                            "files": {{{str(ntuple)!r}: "Events"}},
                            "metadata": {{"threshold": 20, "weights": None}},
                        }},
                    }},
                    keep=keep,
                    columns={{"pt": "pt", "njet": "njet"}},
                )
            """), encoding="utf-8")

            cart = Cart.load("mine", root / "studies")
            self.assertEqual(cart.name, "mine")
            result = cart.build(root / "mine.rvr", step_size=2)
            meta, samples = rvr6.read(result["file"])
            split_meta, split = rvr6.read_by_sample(result["file"])

        self.assertEqual(meta["about"]["name"], "mine")
        self.assertEqual([s["rows"] for s in meta["data"]["samples"]], [3, 2])
        self.assertEqual([s["weighted"] for s in meta["data"]["samples"]], [True, False])
        self.assertEqual(split_meta["made"]["cart"]["sha256"],
                         meta["made"]["cart"]["sha256"])
        np.testing.assert_array_equal(split["simulation"]["pt"], [15, 25, 35])
        np.testing.assert_array_equal(split["data"]["pt"], [25, 35])
        np.testing.assert_array_equal(split["data"]["weight"], [1, 1])

    def test_progress_reports_raw_tree_entries_even_when_prepare_drops_rows(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"pt": np.array([5.0, 15.0, 25.0, 35.0])}
            cart = Cart(
                fileset={"mc": {"files": {str(ntuple): "Events"},
                                "metadata": {"weights": None}}},
                prepare=lambda events: events[events.pt > 10],
                keep=lambda events: events.pt < 30,
                columns={"pt": "pt"},
            )
            seen = []
            cart.skim(step_size=2, progress=lambda *chunk: seen.append(chunk))
        # raw counts the tree entries each chunk covered (the dashboard's totals
        # currency), kept counts what survived PREPARE + KEEP
        self.assertEqual(seen, [("mc", 2, 1), ("mc", 2, 1)])

    def test_a_build_that_cannot_fit_the_cartridge_refuses_at_the_first_chunk(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"pt": np.arange(64.0)}
            cart = Cart(
                fileset={"mc": {"files": {str(ntuple): "Events"},
                                "metadata": {"weights": None}}},
                keep=lambda events: events.pt >= 0,
                columns={"pt": "pt"},
            )
            from rowver import rvr6 as _rvr6
            original = _rvr6.MAX_DATA_BYTES
            # 64 rows x (4 index + 8 f8 column) = 768 > 256; no weight slab (data)
            _rvr6.MAX_DATA_BYTES = 256
            try:
                with self.assertRaisesRegex(CartError, "no longer fit"):
                    cart.skim(step_size=16)
            finally:
                _rvr6.MAX_DATA_BYTES = original

    def test_build_seeds_saved_view_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ntuple = root / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"pt": np.array([25.0])}
            study = root / "mine"
            study.mkdir()
            (study / "cart.py").write_text(textwrap.dedent(f"""
                from rowver import Cart

                CART = Cart(
                    fileset={{"tt": {{"files": {{{str(ntuple)!r}: "Events"}},
                                      "metadata": {{"weights": None}}}}}},
                    keep=lambda events: events.pt > 0,
                    columns={{"pt": "pt"}},
                    view={{"groups": {{"of": {{"tt": "top"}}, "order": ["top"]}}}},
                )
            """), encoding="utf-8")

            result = Cart.load(study).build(root / "mine.rvr")
            meta, _ = rvr6.read(result["file"])

        self.assertEqual(meta["view"], {
            "groups": {"of": {"tt": "top"}, "order": ["top"]},
        })

    def test_byte_budget_chunks_follow_the_branches_the_recipe_touches(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                tree = output.mktree("Events", {"a": np.int32, "heavy": "var * float64"})
                tree.extend({"a": np.arange(5000, dtype=np.int32),
                             "heavy": ak.Array([[1.0] * 30] * 5000)})

            def keep(events):
                if ak.any(events.a > 1500):       # reaches for heavy only later in the file
                    return ak.num(events.heavy) > 0
                return events.a >= 0

            cart = Cart(
                fileset={"mc": {"files": {str(ntuple): "Events"},
                                "metadata": {"weights": None}}},
                keep=keep, columns={"a": "a"},
            )
            seen: list[int] = []
            rows, _ = cart.skim(step_size="4 kB",
                                progress=lambda sample, raw, kept: seen.append(raw))
        # the 1024-entry probe touches only `a` (~4 B/entry), so a 4 kB budget is ~1000
        # entries; once `heavy` (~250 B/entry) is touched the budget is re-derived
        self.assertEqual(seen[0], 1024)
        self.assertGreater(seen[1], 500)
        self.assertLess(max(seen[2:]), 100)
        self.assertEqual(sum(seen), 5000)
        np.testing.assert_array_equal(rows["mc"]["a"], np.arange(5000))

    def test_nested_rntuple_fields_survive_the_batched_read(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            pt = np.arange(3000, dtype=np.float64)
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"a": np.arange(3000, dtype=np.int32),
                                    "Jet": ak.zip({"pt": ak.Array(np.stack([pt, pt / 2], 1)),
                                                   "eta": ak.Array(np.zeros((3000, 2)))})}
            cart = Cart(
                fileset={"mc": {"files": {str(ntuple): "Events"},
                                "metadata": {"weights": None}}},
                keep=lambda events: ak.num(events.Jet.pt) > 1,
                columns={"lead": lambda events: ak.firsts(events.Jet.pt)},
            )
            seen: list[int] = []
            rows, _ = cart.skim(step_size="2 kB",
                                progress=lambda sample, raw, kept: seen.append(raw))
        # bounded chunks even without per-branch totals, never one whole-file remainder
        self.assertGreater(len(seen), 2)
        self.assertLess(max(seen), 3000 - 1024)
        np.testing.assert_array_equal(rows["mc"]["lead"], pt)

    def test_touched_branches_carry_across_files_that_lack_them(self):
        with tempfile.TemporaryDirectory() as directory:
            first, second = Path(directory) / "a.root", Path(directory) / "b.root"
            with uproot.recreate(first) as output:
                output["Events"] = {"a": np.arange(2000, dtype=np.int32),
                                    "b": np.ones(2000, dtype=np.int32)}
            with uproot.recreate(second) as output:
                output["Events"] = {"a": np.arange(2000, 2500, dtype=np.int32)}

            def keep(events):
                if "b" in events.fields:
                    return (events.a >= 0) & (events.b > 0)
                return events.a >= 0

            cart = Cart(
                fileset={"mc": {"files": {str(first): "Events", str(second): "Events"},
                                "metadata": {"weights": None}}},
                keep=keep, columns={"a": "a"},
            )
            rows, _ = cart.skim(step_size="1 kB")
        np.testing.assert_array_equal(rows["mc"]["a"], np.arange(2500))


if __name__ == "__main__":
    unittest.main()
