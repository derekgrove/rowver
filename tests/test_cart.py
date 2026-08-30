from __future__ import annotations

import tempfile
import textwrap
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
