from __future__ import annotations

import base64
import copy
import hashlib
import json
import unittest
from pathlib import Path

import awkward as ak

from rowver import rvr

CORPUS = Path(__file__).parents[1] / "contract/v9/cases.json"


def apply(meta: dict, operations: list[dict]) -> dict:
    out = copy.deepcopy(meta)
    for operation in operations:
        target = out
        path = operation["path"]
        for part in path[:-1]:
            target = target[part]
        if operation.get("op", "set") == "set":
            target[path[-1]] = operation["value"]
        elif operation["op"] == "delete":
            del target[path[-1]]
        else:
            raise AssertionError(operation["op"])
    return out


class ContractCorpusTests(unittest.TestCase):
    # Full-file pin for the hand-synced corpus. A deliberate edit bumps this
    # AND the viewer's pin in the same sync; a mismatch means the copies were
    # edited without re-syncing.
    CORPUS_SHA256 = "a8f606952b53259ea97e043716fff07060ada8bda2f21944718a4f9bb7e3c632"

    def setUp(self):
        self.corpus = json.loads(CORPUS.read_text(encoding="utf-8"))
        self.buf = base64.b64decode(self.corpus["buffer_b64"])

    def test_corpus_pinned(self):
        self.assertEqual(hashlib.sha256(CORPUS.read_bytes()).hexdigest(), self.CORPUS_SHA256,
                         "corpus differs from the pin — bump both repos' pins and re-sync")
        self.assertEqual(hashlib.sha256(self.buf).hexdigest(), self.corpus["buffer_sha256"])

    def test_shared_cases(self):
        for case in self.corpus["cases"]:
            with self.subTest(case=case["id"]):
                meta = apply(self.corpus["base"], case.get("ops", []))
                buf = bytearray(self.buf)
                for at, value in case.get("bytes", []):
                    buf[at] = value
                if not case["accept"]:
                    with self.assertRaises(rvr.FormatError):
                        rvr.check(meta, buf)
                    continue
                arrays = rvr.columns(meta, buf)
                for name, values in self.corpus["expected"].items():
                    self.assertEqual(ak.to_list(arrays[name]), values, name)

    def test_shared_raw_lexemes(self):
        # These probe the byte-level scanner, so they must ride the RAW path
        # (unframe), never a parsed dict: the corpus carries the lexeme and the
        # verdict, and each reader splices it into a consumer key of its own
        # compact base header.
        base = json.dumps(self.corpus["base"], separators=(",", ":"))
        for case in self.corpus["raw_lexemes"]:
            header = ('{"zz": {"v": ' + case["lexeme"] + "}," + base[1:]).encode("utf-8")
            frame = len(header).to_bytes(4, "little") + header + self.buf
            with self.subTest(case=case["id"]):
                if case["accept"]:
                    rvr.check(*rvr.unframe(frame))
                else:
                    with self.assertRaises(rvr.FormatError):
                        rvr.unframe(frame)


if __name__ == "__main__":
    unittest.main()
