from __future__ import annotations

import copy
import gzip
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr6


class Rvr6Tests(unittest.TestCase):
    def setUp(self):
        self.meta, self.buf = rvr6.pack(
            {"mc": {"pt": np.array([10.5, 20.0]), "njet": np.array([0, 1])},
             "data": {"pt": np.array([30.0, 40.0]), "njet": np.array([2, 3])}},
            weighted={"mc": True, "data": False},
            weights={"mc": np.array([0.5, 2.0])},
            about={"name": "contract"},
            made={"built": "test", "cart": None},
        )

    def test_wire_seed_is_little_endian_and_pinned(self):
        self.assertEqual(len(self.buf), 96)
        self.assertEqual(hashlib.sha256(self.buf).hexdigest(),
                         "cc354b0a4695692a6f7032c2882f3709a16f51c448c916a3261c6a3673f7049c")
        self.assertEqual(self.meta["data"]["weight_at"], 0)
        self.assertEqual(self.meta["data"]["sample_at"], 80)

    def test_round_trip_and_unknown_keys_preserve_buffer(self):
        self.meta["probe"] = {"unknown": True}
        self.meta["made"]["probe"] = [1, 2, 3]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr6.write(path, self.meta, self.buf)
            back, buf = rvr6.read(path)
        self.assertEqual(back["probe"], {"unknown": True})
        self.assertEqual(back["made"]["probe"], [1, 2, 3])
        self.assertEqual(buf, self.buf)

    def test_sample_split_keeps_data_and_mc_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr6.write(path, self.meta, self.buf)
            meta, samples = rvr6.read_by_sample(path)
        self.assertEqual([s["weighted"] for s in meta["data"]["samples"]], [True, False])
        np.testing.assert_array_equal(samples["mc"]["weight"], [0.5, 2.0])
        np.testing.assert_array_equal(samples["data"]["weight"], [1.0, 1.0])
        np.testing.assert_array_equal(samples["data"]["pt"], [30.0, 40.0])

    def test_hostile_input_fails_cleanly_not_with_a_crash(self):
        # a corrupted deflate stream inside a plausible gzip file
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr6.write(path, self.meta, self.buf)
            hurt = bytearray(path.read_bytes())
            hurt[len(hurt) // 2] ^= 0xFF
            path.write_bytes(bytes(hurt))
            with self.assertRaises(rvr6.FormatError):
                rvr6.read(path)
        # an enormous integer literal in the header trips the digit-limit guard
        digits = b'{"rowver": 6, "zz": ' + b"9" * 5000 + b"}"
        with self.assertRaises(rvr6.FormatError):
            rvr6.unframe(len(digits).to_bytes(4, "little") + digits)
        # unhashable values wherever the format names an option
        for stage in ([{"kind": "board", "met": [{}]}],
                      [{"kind": "board", "channel": {}}],
                      [{"kind": "1d", "varx": {}}]):
            meta = copy.deepcopy(self.meta)
            meta["tabs"] = [{"name": "t", "stage": stage}]
            with self.assertRaises(rvr6.FormatError):
                rvr6.check(meta, self.buf)
        meta = copy.deepcopy(self.meta)
        meta["view"]["look"] = {"sample": {"data": {"draw": {}}}}
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(meta, self.buf)
        # unhashable values in the map grammar and in provenance names
        for stage in ([{"kind": "map", "srcs": ["mc"], "bkg": ["mc"],
                        "grid": {"vars": [{"col": {}, "bands": []}]}}],
                      [{"kind": "map", "srcs": ["mc"], "bkg": ["mc"],
                        "root": {"cut": {"col": {}, "lt": 1}, "yes": None, "no": None}}]):
            meta = copy.deepcopy(self.meta)
            meta["tabs"] = [{"name": "t", "stage": stage}]
            with self.assertRaises(rvr6.FormatError):
                rvr6.check(meta, self.buf)
        meta = copy.deepcopy(self.meta)
        meta["made"]["samples"] = [{"name": {}, "rows": 2, "files": [], "weights": None},
                                   {"name": "data", "rows": 2, "files": [], "weights": None}]
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(meta, self.buf)
        # an integer below the digit-parse limit but far above float64 range
        meta = copy.deepcopy(self.meta)
        meta["data"]["cols"][0]["p95"] = 10 ** 400
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(meta, self.buf)
        # pathological nesting inside the byte limit (Python 3.13 parses a few thousand
        # levels fine; fifty thousand exhausts the decoder's recursion budget)
        deep = b"[" * 50_000 + b"]" * 50_000
        hdr = b'{"rowver": 6, "zz": ' + deep + b"}"
        with self.assertRaises(rvr6.FormatError):
            rvr6.unframe(len(hdr).to_bytes(4, "little") + hdr)

    def test_frame_refuses_non_string_mapping_keys(self):
        # json.dumps would coerce 1 to "1" and silently collide with the real "1"
        meta = copy.deepcopy(self.meta)
        meta["zz"] = {1: "integer-key", "1": "string-key"}
        with self.assertRaises(rvr6.FormatError):
            rvr6.frame(meta, self.buf)
        # a circular reference is left for json.dumps to refuse, not a stack overflow
        meta = copy.deepcopy(self.meta)
        cycle: dict = {}
        cycle["self"] = cycle
        meta["zz"] = cycle
        with self.assertRaises(rvr6.FormatError):
            rvr6.frame(meta, self.buf)

    def test_wire_numbers_read_identically_everywhere(self):
        # 1e400 is inf to Python and null to a JavaScript save; 2**53 + 1 silently
        # rounds in a JavaScript reader — neither may enter or leave a cartridge
        for hdr in (b'{"rowver": 6, "zz": 1e400}',
                    b'{"rowver": 6, "zz": 9007199254740993}'):
            with self.assertRaises(rvr6.FormatError):
                rvr6.unframe(len(hdr).to_bytes(4, "little") + hdr)
        meta = copy.deepcopy(self.meta)
        meta["zz"] = 2 ** 53 + 1
        with self.assertRaises(rvr6.FormatError):
            rvr6.frame(meta, self.buf)

    def test_bool_is_not_a_row_count(self):
        meta = copy.deepcopy(self.meta)
        meta["made"]["samples"] = [
            {"name": "mc", "rows": True, "files": [], "weights": None},
            {"name": "data", "rows": 2, "files": [], "weights": None}]
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(meta, self.buf)

    def test_subnormal_columns_pack(self):
        meta, buf = rvr6.pack({"data": {"x": np.array([5e-324])}},
                              weighted={"data": False})
        rvr6.check(meta, buf)

    def test_cli_survives_hostile_names(self):
        # a lone surrogate is unprintable UTF-8 and an escape sequence repaints the
        # terminal; a validly framed file can carry both as ASCII escapes, and the CLI
        # must neither crash nor let the name touch the terminal state
        meta = copy.deepcopy(self.meta)
        meta["about"]["name"] = "\ud800\x1b[2Jname"
        header = json.dumps(meta, ensure_ascii=True, separators=(",", ":")).encode("ascii")
        raw = len(header).to_bytes(4, "little") + header + self.buf
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            path.write_bytes(gzip.compress(raw, mtime=0))
            self.assertEqual(rvr6._main(["check", str(path)]), 0)

    def test_refuses_structural_and_numeric_lies(self):
        mutations = []

        def case(change):
            value = copy.deepcopy(self.meta)
            change(value)
            mutations.append(value)

        case(lambda m: m["data"]["cols"][0].update(kind="wat"))
        case(lambda m: m["data"]["cols"][0].update(at=34))
        case(lambda m: m["data"]["cols"][1].update(name="pt"))
        case(lambda m: m["data"]["samples"][0].pop("weighted"))
        case(lambda m: m["data"]["samples"][0].update(rows=1))
        case(lambda m: m["data"]["cols"][0].update(lim=[40, 10]))
        case(lambda m: m["data"]["cols"][0].update(nice=[11, 40]))
        case(lambda m: m["data"]["cols"][0].update(p95=41))
        case(lambda m: m.update(opens_on=2))
        case(lambda m: m["tabs"].append({"name": "bad", "cuts": {"missing": [0, 1]}}))
        case(lambda m: m["tabs"].append({"name": "bad", "cuts": {"njet": [1, 1]}}))
        case(lambda m: m["tabs"].append({"name": "bad", "cuts": {"pt": {"in": [20]}}}))
        case(lambda m: m["tabs"].append({"name": "bad", "stage": [
            {"kind": "block", "layout": [3, 3], "cells": [
                {"kind": "block", "layout": [6], "cells": [{"kind": "1d"}]}, None]}]}))
        for meta in mutations:
            with self.subTest(meta=meta):
                with self.assertRaises(rvr6.FormatError):
                    rvr6.check(meta, self.buf)

    def test_refuses_buffer_lies(self):
        invalid_index = bytearray(self.buf)
        invalid_index[80:84] = (9).to_bytes(4, "little", signed=True)
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(self.meta, invalid_index)

        invalid_weight = bytearray(self.buf)
        invalid_weight[0:8] = np.array([np.nan], dtype="<f8").tobytes()
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(self.meta, invalid_weight)

        invalid_column = bytearray(self.buf)
        invalid_column[32:40] = np.array([np.inf], dtype="<f8").tobytes()
        with self.assertRaises(rvr6.FormatError):
            rvr6.check(self.meta, invalid_column)

    def test_luminosity_is_outermost_first(self):
        view = {"groups": {"of": {"mc": "all"}, "sub": {"mc": "era"}},
                "lumis": {"sample": {"mc": 3}, "sub": {"era": 20},
                          "group": {"all": 100}}}
        self.assertEqual(rvr6.lumi_of("mc", view), 100)
        view["lumis"]["group"]["all"] = None
        self.assertEqual(rvr6.lumi_of("mc", view), 20)

    def test_refuses_integer_values_float64_cannot_represent_exactly(self):
        with self.assertRaisesRegex(rvr6.FormatError, "fit exactly"):
            rvr6.pack({"sample": {"event": np.array([2 ** 53 + 1], dtype=np.uint64)}},
                      weighted={"sample": False}, about={"name": "bad"})

    def test_promote_and_shadow_does_not_delete_tab_state(self):
        meta = copy.deepcopy(self.meta)
        meta["tabs"] = [
            {"name": "a", "cuts": {"pt": [10, 20]}},
            {"name": "b", "cuts": {"pt": [30, 40]}},
        ]
        meta["opens_on"] = 0
        locked = rvr6.lock(meta, "cuts", 0)
        self.assertEqual(rvr6.effective(locked["tabs"][1], locked["view"], "cuts"),
                         {"pt": [10, 20]})
        self.assertEqual(rvr6.unlock(locked, "cuts")["tabs"][1]["cuts"], {"pt": [30, 40]})


if __name__ == "__main__":
    unittest.main()
