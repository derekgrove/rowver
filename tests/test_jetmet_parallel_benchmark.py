import os
from pathlib import Path
from unittest import TestCase, mock

from rowver import Cart, CartError


STUDY = Path(__file__).parents[1] / "studies" / "jetmet_parallel_benchmark"


class JetMETParallelBenchmarkTests(TestCase):
    def load(self, count: str) -> Cart:
        with mock.patch.dict(os.environ, {"ROWVER_JETMET_FILES": count}):
            return Cart.load(STUDY)

    def test_supported_sizes_select_nested_distinct_sources(self):
        previous: list[str] = []
        for count in (4, 8, 16, 32, 64, 128):
            files = list(self.load(str(count)).fileset["JetMET 2018 DATA"]["files"])
            self.assertEqual(len(files), count)
            self.assertEqual(len(set(files)), count)
            self.assertEqual(files[:len(previous)], previous)
            previous = files

    def test_default_is_four_files(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            cart = Cart.load(STUDY)
        self.assertEqual(len(cart.fileset["JetMET 2018 DATA"]["files"]), 4)

    def test_invalid_size_is_rejected_without_opening_sources(self):
        with self.assertRaisesRegex(CartError, "must be one of"):
            self.load("5")
