"""Column facts: nice encloses lim, keeping the limit itself where two-digit rounding lands inside the range."""
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from rowver import rvr  # noqa: E402


class Facts(unittest.TestCase):
    def encloses(self, array):
        """Whether nice encloses lim, and the facts."""
        facts = rvr.facts(array, None)
        (lo, hi), (nice_lo, nice_hi) = facts["lim"], facts["nice"]
        return nice_lo <= lo and hi <= nice_hi, facts

    def test_nice_keeps_the_limit_where_rounding_fell_inside(self):
        """Each of these once got an end inside the range; the Rust RowVer pins the same values."""
        for array, nice in ((np.array([1.2, 5.0]), [1.2, 5.0]),
                            (np.array([0.1, 0.30000000000000004]), [0.1, 0.30000000000000004]),
                            (np.array([5, 120000001]), [5, 120000001]),
                            (np.array([1.1, 1.1], dtype="f4"), [1.1, 1.100000023841858]),
                            (np.array([5, 9200000000000000001], dtype="u8"), [5, 9200000000000000001]),
                            (np.array([-1.2, -1.2]), [-1.2000000000000002, -1.2]),
                            (np.array([-120000001, -7]), [-120000001, -7])):
            ok, facts = self.encloses(array)
            self.assertTrue(ok, facts)
            self.assertEqual(facts["nice"], nice)

    def test_nice_is_unchanged_where_it_already_enclosed(self):
        """Ordinary values keep the two-digit ends they had."""
        self.assertEqual(rvr.facts(np.array([150.1, 1583.2]), None)["nice"], [150.0, 1600.0])
        self.assertEqual(rvr.facts(np.array([0, 2]), None)["nice"], [0, 2])
        self.assertEqual(rvr.facts(np.array([-0.037, 41.0]), None)["nice"], [-0.037, 41.0])

    def test_random_values_are_enclosed(self):
        """Many magnitudes and kinds, each enclosed."""
        random = np.random.default_rng(7)
        for _ in range(4000):
            value = random.normal() * 10.0 ** random.uniform(-20, 20)
            for array in (np.array([value]), np.array([value], dtype="f4"), np.array([int(np.clip(value, -2 ** 62, 2 ** 62))])):
                ok, facts = self.encloses(array)
                self.assertTrue(ok, facts)


if __name__ == "__main__":
    unittest.main()
