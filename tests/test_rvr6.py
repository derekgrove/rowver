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

    def test_percentile_ladder_required_and_exact(self):
        # every packed p-key is np.percentile over the CONCATENATED samples
        cols = {c["name"]: c for c in self.meta["data"]["cols"]}
        whole = {"pt": np.array([10.5, 20.0, 30.0, 40.0]),
                 "njet": np.array([0, 1, 2, 3])}
        for name, values in whole.items():
            for q in rvr6.PCTS:
                self.assertEqual(cols[name][f"p{q}"],
                                 float(np.percentile(values, q)),
                                 f"{name} p{q}")
        # DELETING any one of the thirteen refuses, not just nulling it
        for q in rvr6.PCTS:
            broken = copy.deepcopy(self.meta)
            del broken["data"]["cols"][0][f"p{q}"]
            with self.assertRaises(rvr6.FormatError, msg=f"p{q}"):
                rvr6.check(broken, self.buf)

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

    def test_v8_digests_pin_every_byte_of_the_buffer(self):
        data = self.meta["data"]
        self.assertEqual(data["sha256"], hashlib.sha256(self.buf).hexdigest())
        for col in data["cols"]:
            span = self.buf[col["at"]:col["at"] + rvr6._length(col["kind"], data["rows"])]
            self.assertEqual(col["sha256"], hashlib.sha256(span).hexdigest())
        # a weight edit keeps every lim and vals true; only the buffer digest can see it
        edited = bytearray(self.buf)
        edited[0:8] = np.array([0.6], dtype="<f8").tobytes()
        with self.assertRaisesRegex(rvr6.FormatError, "data.sha256"):
            rvr6.check(self.meta, edited)
        # a value moved inside lim is caught by its column digest
        edited = bytearray(self.buf)
        edited[40:48] = np.array([25.0], dtype="<f8").tobytes()
        meta = copy.deepcopy(self.meta)
        meta["data"]["sha256"] = hashlib.sha256(edited).hexdigest()
        with self.assertRaisesRegex(rvr6.FormatError, 'column "pt" sha256'):
            rvr6.check(meta, edited)
        for path in (("data", "sha256"), ("data", "cols", 0, "sha256")):
            meta = copy.deepcopy(self.meta)
            target = meta
            for part in path[:-1]:
                target = target[part]
            del target[path[-1]]
            with self.assertRaises(rvr6.FormatError):
                rvr6.check(meta, self.buf)
        # a version 7 header carries no digests and is not asked for any
        meta["rowver"] = 7
        del meta["data"]["cols"][1]["sha256"]
        rvr6.check(meta, self.buf)

    def test_column_sources_are_shape_checked_only(self):
        sources = {"pt": {"class": "copy", "branch": "Muon_pt", "dtype": "float32"},
                   "njet": {"class": "derived", "from": ["Jet_pt"], "reproducible": "source",
                            "python": "lambda events: ak.num(events.Jet_pt)"}}
        meta, buf = rvr6.pack(
            {"mc": {"pt": np.array([10.5, 20.0]), "njet": np.array([0, 1])}},
            weighted={"mc": False}, sources=sources)
        self.assertEqual({c["name"]: c["source"] for c in meta["data"]["cols"]}, sources)
        self.assertNotIn("source", self.meta["data"]["cols"][0])
        for broken in ("Muon_pt", {"class": "wat"},
                       {"class": "copy", "branch": "Muon_pt"},
                       {"class": "copy", "branch": "", "dtype": "float32"},
                       {"class": "derived", "from": ["a", "a"], "reproducible": "source", "python": ""},
                       {"class": "derived", "from": [{}], "reproducible": "source", "python": ""},
                       {"class": "derived", "from": ["a"], "reproducible": "maybe", "python": ""},
                       {"class": "derived", "from": ["a"], "reproducible": "cartridge"}):
            bad = copy.deepcopy(meta)
            bad["data"]["cols"][0]["source"] = broken
            with self.subTest(source=broken):
                with self.assertRaises(rvr6.FormatError):
                    rvr6.check(bad, buf)

    def test_made_file_identity_is_typed(self):
        meta = copy.deepcopy(self.meta)
        files = [{"path": "a.root", "tree": "Events", "uuid": "6f0c1a2e", "entries": 4}]
        meta["made"]["samples"] = [
            {"name": "mc", "rows": 2, "files": files, "metadata": {}, "weights": None},
            {"name": "data", "rows": 2, "files": [], "metadata": {}, "weights": None}]
        rvr6.check(meta, self.buf)
        for bad in ({"uuid": 1}, {"entries": True}, {"entries": -1}, {"tree": ""}):
            broken = copy.deepcopy(meta)
            broken["made"]["samples"][0]["files"][0].update(bad)
            with self.subTest(file=bad):
                with self.assertRaises(rvr6.FormatError):
                    rvr6.check(broken, self.buf)
                if bad != {"tree": ""}:
                    broken["rowver"] = 7
                    for col in broken["data"]["cols"]:
                        del col["sha256"]
                    rvr6.check(broken, self.buf)

    def test_copies_keep_their_dtype_and_derived_columns_still_widen(self):
        derived = {"class": "derived", "from": ["b"], "reproducible": "source", "python": "f"}
        values = {"f4": np.array([1.5, 2.5, 3.5], dtype=np.float32),
                  "u8": np.array([2 ** 63, 1, 2 ** 53 + 1], dtype=np.uint64),
                  "i8": np.array([-(2 ** 62), 0, 1], dtype=np.int64),
                  "i2": np.array([-3, 0, 3], dtype=np.int16),
                  "u1": np.array([0, 200, 255], dtype=np.uint8),
                  "f2": np.array([0.5, 1.0, 1.5], dtype=np.float16),
                  "d8": np.array([1, 2, 3], dtype=np.int64),
                  "bit": np.array([True, False, True])}
        sources = {name: {"class": "copy", "branch": "b", "dtype": str(value.dtype)}
                   for name, value in values.items() if name != "d8"} | {"d8": derived}
        meta, buf = rvr6.pack({"s": values}, weighted={"s": False}, sources=sources)
        kinds = {c["name"]: c["kind"] for c in meta["data"]["cols"]}
        self.assertEqual(kinds, {"f4": "f4", "u8": "u8", "i8": "i8", "i2": "i2", "u1": "u1",
                                 "f2": "f8", "d8": "i4", "bit": "b1"})
        self.assertEqual(len(buf), 3 * (4 + 4 + 8 + 8 + 2 + 1 + 8 + 4) + 1)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "native.rvr"
            rvr6.write(path, meta, buf)
            back, payload = rvr6.read(path)
        columns = rvr6.columns(back, payload)
        for name, expected in values.items():
            np.testing.assert_array_equal(columns[name], expected)
            if name not in ("f2", "d8", "bit"):
                self.assertEqual(columns[name].dtype, expected.dtype)
        # 64-bit values beyond 2**53 reach the header only as floats a JavaScript reader keeps
        u8 = {c["name"]: c for c in back["data"]["cols"]}["u8"]
        self.assertNotIn("vals", u8)
        self.assertIsInstance(u8["nice"][1], float)
        # the sample index sits after the 8-byte columns, so every span is aligned for odd rows
        for col in back["data"]["cols"]:
            self.assertEqual(col["at"] % rvr6.WIDTH[col["kind"]], 0, col["name"])
        self.assertEqual(back["data"]["sample_at"] % 4, 0)
        self.assertLess(back["data"]["sample_at"], min(c["at"] for c in back["data"]["cols"]
                                                       if c["kind"] in ("i2", "u1", "b1")))
        small = {name: values[name] for name in ("f4", "i2", "u1", "f2", "d8", "bit")}
        without_sources, _ = rvr6.pack({"s": small}, weighted={"s": False})
        self.assertEqual({c["name"]: c["kind"] for c in without_sources["data"]["cols"]},
                         {"f4": "f8", "i2": "i4", "u1": "i4", "f2": "f8", "d8": "i4", "bit": "b1"})

    def test_a_copy_without_an_exact_common_dtype_is_refused(self):
        copy = {"n": {"class": "copy", "branch": "n", "dtype": "uint64"}}
        for other in (np.array([-1], dtype=np.int64), np.array([1.5], dtype=np.float32)):
            with self.subTest(other=other.dtype), \
                    self.assertRaisesRegex(rvr6.FormatError, "no exact common dtype"):
                rvr6.pack({"a": {"n": np.array([7], dtype=np.uint64)}, "b": {"n": other}},
                          weighted={"a": False, "b": False}, sources=copy)
        # the same samples without a copy claim take the derived promotion
        meta, _ = rvr6.pack({"a": {"n": np.array([7], dtype=np.uint64)},
                             "b": {"n": np.array([-1], dtype=np.int64)}},
                            weighted={"a": False, "b": False})
        self.assertEqual(meta["data"]["cols"][0]["kind"], "i4")
        # the dtype a branch was read as decides the kind even where every kept value is narrower
        wider, _ = rvr6.pack({"a": {"n": np.array([7], dtype=np.int32)}}, weighted={"a": False},
                             sources={"n": {"class": "copy", "branch": "n", "dtype": "int64"}})
        self.assertEqual(wider["data"]["cols"][0]["kind"], "i8")
        with self.assertRaisesRegex(rvr6.FormatError, "not a NumPy dtype"):
            rvr6.pack({"a": {"n": np.array([7], dtype=np.int32)}}, weighted={"a": False},
                      sources={"n": {"class": "copy", "branch": "n", "dtype": "Int_t"}})
        with self.assertRaisesRegex(rvr6.FormatError, "fit exactly"):
            rvr6.pack({"a": {"n": np.array([2 ** 63], dtype=np.uint64)},
                       "b": {"n": np.array([-1], dtype=np.int64)}},
                      weighted={"a": False, "b": False})
        # a bound the header cannot hold as a float is a format error, not a crash
        with self.assertRaisesRegex(rvr6.FormatError, "finite"):
            rvr6.pack({"a": {"x": np.array([1.75e308])}}, weighted={"a": False})

    def test_an_empty_sample_never_promotes_a_copy_before_it_is_stored(self):
        meta, buf = rvr6.pack({"a": {"n": np.array([2 ** 62 + 1], dtype=np.int64)},
                               "b": {"n": np.empty(0, dtype=np.float64)}},
                              weighted={"a": False, "b": False},
                              sources={"n": {"class": "copy", "branch": "n", "dtype": "int64"}})
        self.assertEqual(meta["data"]["cols"][0]["kind"], "i8")
        np.testing.assert_array_equal(rvr6.columns(meta, buf)["n"], [2 ** 62 + 1])

    def test_native_kinds_are_version_8_only(self):
        meta, buf = rvr6.pack({"s": {"n": np.array([1, 2], dtype=np.uint8)}}, weighted={"s": False},
                              sources={"n": {"class": "copy", "branch": "n", "dtype": "uint8"}})
        self.assertEqual(meta["data"]["cols"][0]["vals"], [1, 2])
        meta["rowver"] = 7
        del meta["data"]["cols"][0]["sha256"]
        with self.assertRaisesRegex(rvr6.FormatError, "unknown kind"):
            rvr6.check(meta, buf)

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
