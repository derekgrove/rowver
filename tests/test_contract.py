from __future__ import annotations

import base64
import copy
import hashlib
import json
import unittest
from pathlib import Path

import rvr6


def apply(meta: dict, operations: list[dict]) -> dict:
    out = copy.deepcopy(meta)
    for operation in operations:
        target = out
        path = operation["path"]
        for part in path[:-1]:
            target = target[part]
        if operation["op"] == "set":
            target[path[-1]] = operation["value"]
        elif operation["op"] == "delete":
            del target[path[-1]]
        else:
            raise AssertionError(operation["op"])
    return out


class ContractCorpusTests(unittest.TestCase):
    def test_shared_v6_cases(self):
        corpus = json.loads((Path(__file__).parents[1] / "contract/v6/cases.json").read_text())
        buf = base64.b64decode(corpus["buffer_b64"])
        self.assertEqual(hashlib.sha256(buf).hexdigest(), corpus["buffer_sha256"])
        for case in corpus["cases"]:
            meta = apply(corpus["base"], case["ops"])
            with self.subTest(case=case["id"]):
                if case["accept"]:
                    rvr6.check(meta, buf)
                else:
                    with self.assertRaises(rvr6.FormatError):
                        rvr6.check(meta, buf)

    def test_region_recipes_have_hard_complexity_limits(self):
        corpus = json.loads((Path(__file__).parents[1] / "contract/v6/cases.json").read_text())
        buf = base64.b64decode(corpus["buffer_b64"])
        grid = copy.deepcopy(corpus["base"])
        grid["tabs"] = [{"name": "large", "stage": [{"kind": "map", "grid": {"vars": [
            {"col": "pt", "bands": list(range(64))},
            {"col": "njet", "bands": list(range(64))},
        ]}}]}]
        with self.assertRaisesRegex(rvr6.FormatError, "too many regions"):
            rvr6.check(grid, buf)

        root = None
        for _ in range(66):
            root = {"cut": {"col": "pt", "lt": 20}, "yes": None, "no": root}
        deep = copy.deepcopy(corpus["base"])
        deep["tabs"] = [{"name": "deep", "stage": [{"kind": "map", "root": root}]}]
        with self.assertRaisesRegex(rvr6.FormatError, "too deep"):
            rvr6.check(deep, buf)


if __name__ == "__main__":
    unittest.main()
