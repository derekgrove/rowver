from __future__ import annotations

import copy
import gzip
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np

from rowver import rvr
from support import snapshot


class RvrTests(unittest.TestCase):
    def setUp(self):
        self.meta, self.buf = rvr.pack(
            {"mc": {"pt": np.array([10.5, 20.0]), "njet": np.array([0, 1])},
             "data": {"pt": np.array([30.0, 40.0]), "njet": np.array([2, 3])}},
            weighted={"mc": True, "data": False},
            weights={"mc": np.array([0.5, 2.0])},
            made=snapshot("mc", "data"),
        )

    def refused(self, change, message=None):
        meta = copy.deepcopy(self.meta)
        change(meta)
        with self.assertRaisesRegex(rvr.FormatError, message or ""):
            rvr.check(meta, self.buf)

    def test_percentile_ladder_required_and_exact(self):
        # every packed p-key is np.percentile over the CONCATENATED samples
        cols = {c["name"]: c for c in self.meta["data"]["cols"]}
        whole = {"pt": np.array([10.5, 20.0, 30.0, 40.0]),
                 "njet": np.array([0, 1, 2, 3])}
        for name, values in whole.items():
            for q in rvr.PCTS:
                self.assertEqual(cols[name][f"p{q}"],
                                 float(np.percentile(values, q)),
                                 f"{name} p{q}")
        # DELETING any one of the thirteen refuses, not just nulling it
        for q in rvr.PCTS:
            self.refused(lambda m: m["data"]["cols"][0].pop(f"p{q}"), f"p{q}")

    def test_wire_seed_is_little_endian_and_pinned(self):
        self.assertEqual(len(self.buf), 96)
        self.assertEqual(hashlib.sha256(self.buf).hexdigest(),
                         "cc354b0a4695692a6f7032c2882f3709a16f51c448c916a3261c6a3673f7049c")
        self.assertEqual(self.meta["data"]["weight_at"], 0)
        self.assertEqual(self.meta["data"]["sample_at"], 80)
        self.assertEqual(list(self.meta), ["rowver", "data", "made"])

    def test_round_trip_and_unknown_keys_preserve_buffer(self):
        self.meta["probe"] = {"unknown": True}
        self.meta["made"]["probe"] = [1, 2, 3]
        self.meta["made"]["sha256"] = rvr.made_digest(self.meta["made"])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr.write(path, self.meta, self.buf)
            back, buf = rvr.read(path)
        self.assertEqual(back["probe"], {"unknown": True})
        self.assertEqual(back["made"]["probe"], [1, 2, 3])
        self.assertEqual(buf, self.buf)

    def test_sample_split_keeps_data_and_mc_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr.write(path, self.meta, self.buf)
            meta, samples = rvr.read_by_sample(path)
        self.assertEqual([s["weighted"] for s in meta["data"]["samples"]], [True, False])
        np.testing.assert_array_equal(samples["mc"]["weight"], [0.5, 2.0])
        np.testing.assert_array_equal(samples["data"]["weight"], [1.0, 1.0])
        np.testing.assert_array_equal(samples["data"]["pt"], [30.0, 40.0])

    def test_hostile_input_fails_cleanly_not_with_a_crash(self):
        # a corrupted deflate stream inside a plausible gzip file
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr.write(path, self.meta, self.buf)
            hurt = bytearray(path.read_bytes())
            hurt[len(hurt) // 2] ^= 0xFF
            path.write_bytes(bytes(hurt))
            with self.assertRaises(rvr.FormatError):
                rvr.read(path)
        # an enormous integer literal in the header trips the digit-limit guard
        digits = b'{"rowver": 9, "zz": ' + b"9" * 5000 + b"}"
        with self.assertRaises(rvr.FormatError):
            rvr.unframe(len(digits).to_bytes(4, "little") + digits)
        # unhashable values wherever the format names an option
        self.refused(lambda m: m["data"]["cols"][0].update(kind={}))
        self.refused(lambda m: m["data"]["cols"][0].update(source={"class": {}}))
        self.refused(lambda m: m["made"]["samples"].update(x={}))
        self.refused(lambda m: m["made"].update(keep={"kind": {}}))
        # an integer below the digit-parse limit but far above float64 range
        self.refused(lambda m: m["data"]["cols"][0].update(p95=10 ** 400))
        # pathological nesting inside the byte limit (Python 3.13 parses a few thousand
        # levels fine; fifty thousand exhausts the decoder's recursion budget)
        deep = b"[" * 50_000 + b"]" * 50_000
        hdr = b'{"rowver": 9, "zz": ' + deep + b"}"
        with self.assertRaises(rvr.FormatError):
            rvr.unframe(len(hdr).to_bytes(4, "little") + hdr)

    def test_frame_refuses_non_string_mapping_keys(self):
        # json.dumps would coerce 1 to "1" and silently collide with the real "1"
        meta = copy.deepcopy(self.meta)
        meta["zz"] = {1: "integer-key", "1": "string-key"}
        with self.assertRaises(rvr.FormatError):
            rvr.frame(meta, self.buf)
        # a circular reference is left for json.dumps to refuse, not a stack overflow
        meta = copy.deepcopy(self.meta)
        cycle: dict = {}
        cycle["self"] = cycle
        meta["zz"] = cycle
        with self.assertRaises(rvr.FormatError):
            rvr.frame(meta, self.buf)

    def test_wire_numbers_read_identically_everywhere(self):
        # 1e400 is inf to Python and null to a JavaScript save; 2**53 + 1 silently
        # rounds in a JavaScript reader — neither may enter or leave a cartridge
        for hdr in (b'{"rowver": 9, "zz": 1e400}',
                    b'{"rowver": 9, "zz": 9007199254740993}'):
            with self.assertRaises(rvr.FormatError):
                rvr.unframe(len(hdr).to_bytes(4, "little") + hdr)
        meta = copy.deepcopy(self.meta)
        meta["zz"] = 2 ** 53 + 1
        with self.assertRaises(rvr.FormatError):
            rvr.frame(meta, self.buf)

    def test_unpaired_surrogates_have_no_utf8_and_are_refused(self):
        # the escape is legal JSON but the string has no UTF-8 encoding and no digest
        for hdr in (b'{"rowver": 9, "zz": "\\ud800"}', b'{"rowver": 9, "\\udc00": 1}'):
            with self.assertRaisesRegex(rvr.FormatError, "surrogate"):
                rvr.unframe(len(hdr).to_bytes(4, "little") + hdr)
        meta = copy.deepcopy(self.meta)
        meta["zz"] = {"name": "\ud800"}
        with self.assertRaisesRegex(rvr.FormatError, "surrogate"):
            rvr.frame(meta, self.buf)
        self.refused(lambda m: m["made"].update(note="\ud800"), "surrogate")
        with self.assertRaisesRegex(rvr.FormatError, "surrogate"):
            rvr.made_digest({"note": "\ud800"})
        # a paired surrogate escape is an ordinary character
        hdr = b'{"rowver": 9, "zz": "\\ud83d\\ude00"}'
        self.assertEqual(rvr.unframe(len(hdr).to_bytes(4, "little") + hdr)[0]["zz"], "\U0001f600")

    def test_subnormal_columns_pack(self):
        meta, buf = rvr.pack({"data": {"x": np.array([5e-324])}},
                             weighted={"data": False}, made=snapshot("data"))
        rvr.check(meta, buf)

    def test_cli_survives_hostile_names(self):
        # an escape sequence in the study name would repaint the terminal; the CLI
        # prints it defused, and a lone-surrogate header is refused without a crash
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            meta = copy.deepcopy(self.meta)
            meta["made"]["study_name"] = "\x1b[2Jname"
            meta["made"]["sha256"] = rvr.made_digest(meta["made"])
            rvr.write(path, meta, self.buf)
            self.assertEqual(rvr._main(["check", str(path)]), 0)
            meta["made"]["study_name"] = "\ud800name"
            header = json.dumps(meta, ensure_ascii=True, separators=(",", ":")).encode("ascii")
            raw = len(header).to_bytes(4, "little") + header + self.buf
            path.write_bytes(gzip.compress(raw, mtime=0))
            with self.assertRaises(SystemExit) as stop:
                rvr._main(["check", str(path)])
            self.assertEqual(stop.exception.code, 1)

    def test_refuses_structural_and_numeric_lies(self):
        cases = [
            lambda m: m["data"]["cols"][0].update(kind="wat"),
            lambda m: m["data"]["cols"][0].update(at=34),
            lambda m: m["data"]["cols"][1].update(name="pt"),
            lambda m: m["data"]["samples"][0].pop("weighted"),
            lambda m: m["data"]["samples"][0].update(rows=1),
            lambda m: m["data"]["cols"][0].update(lim=[40, 10]),
            lambda m: m["data"]["cols"][0].update(nice=[11, 40]),
            lambda m: m["data"]["cols"][0].update(p95=41),
            lambda m: m["data"]["cols"][1].update(vals=[3, 2, 1, 0]),
            lambda m: m["data"]["cols"][0].update(vals=[10.5, 20.0, 30.0, 40.0]),
            lambda m: m.update(rowver=8),
            lambda m: m.update(rowver=9.0),
            lambda m: m.update(about={"name": "study"}),
            lambda m: m.update(view={}),
            lambda m: m.update(tabs=[]),
            lambda m: m.update(opens_on=0),
            lambda m: m.update(tool="not an object"),
            lambda m: m.pop("made"),
        ]
        for change in cases:
            with self.subTest(change=change):
                self.refused(change)

    def test_refuses_buffer_lies(self):
        invalid_index = bytearray(self.buf)
        invalid_index[80:84] = (9).to_bytes(4, "little", signed=True)
        with self.assertRaises(rvr.FormatError):
            rvr.check(self.meta, invalid_index)

        invalid_weight = bytearray(self.buf)
        invalid_weight[0:8] = np.array([np.nan], dtype="<f8").tobytes()
        with self.assertRaises(rvr.FormatError):
            rvr.check(self.meta, invalid_weight)

        invalid_column = bytearray(self.buf)
        invalid_column[32:40] = np.array([np.inf], dtype="<f8").tobytes()
        with self.assertRaises(rvr.FormatError):
            rvr.check(self.meta, invalid_column)

    def test_refuses_integer_values_float64_cannot_represent_exactly(self):
        with self.assertRaisesRegex(rvr.FormatError, "fit exactly"):
            rvr.pack({"sample": {"event": np.array([2 ** 53 + 1], dtype=np.uint64)}},
                     weighted={"sample": False}, made=snapshot("sample"))

    def test_digests_pin_every_byte_of_the_buffer(self):
        data = self.meta["data"]
        self.assertEqual(data["sha256"], hashlib.sha256(self.buf).hexdigest())
        for col in data["cols"]:
            span = self.buf[col["at"]:col["at"] + rvr._length(col["kind"], data["rows"])]
            self.assertEqual(col["sha256"], hashlib.sha256(span).hexdigest())
        # a weight edit keeps every lim and vals true; only the buffer digest can see it
        edited = bytearray(self.buf)
        edited[0:8] = np.array([0.6], dtype="<f8").tobytes()
        with self.assertRaisesRegex(rvr.FormatError, "data.sha256"):
            rvr.check(self.meta, edited)
        # a value moved inside lim is caught by its column digest
        edited = bytearray(self.buf)
        edited[40:48] = np.array([25.0], dtype="<f8").tobytes()
        meta = copy.deepcopy(self.meta)
        meta["data"]["sha256"] = hashlib.sha256(edited).hexdigest()
        with self.assertRaisesRegex(rvr.FormatError, 'column "pt" sha256'):
            rvr.check(meta, edited)
        self.refused(lambda m: m["data"].pop("sha256"))
        self.refused(lambda m: m["data"]["cols"][0].pop("sha256"))

    def test_made_is_required_in_full_and_digested(self):
        made = self.meta["made"]
        self.assertEqual((list(made)[0], list(made)[-1]), ("study_name", "sha256"))
        # pack digests the snapshot itself, replacing whatever the caller carried
        stale, _ = rvr.pack({"s": {"x": np.array([1.0])}}, weighted={"s": False},
                            made={**snapshot("s"), "sha256": "0" * 64})
        self.assertEqual(stale["made"]["sha256"], rvr.made_digest(stale["made"]))
        self.assertEqual(made["sha256"], hashlib.sha256(json.dumps(
            {k: v for k, v in made.items() if k != "sha256"}, sort_keys=True,
            separators=(",", ":"), ensure_ascii=False).encode("utf-8")).hexdigest())
        for key in ("study_name", "by", "built", "sha256", "samples", "keep", "prepare", "cart"):
            self.refused(lambda m, key=key: m["made"].pop(key), key)
        # any edit without a fresh digest is caught; with one, it passes
        self.refused(lambda m: m["made"].update(title="edited"), "made.sha256")
        meta = copy.deepcopy(self.meta)
        meta["made"]["title"] = "edited"
        meta["made"]["sha256"] = rvr.made_digest(meta["made"])
        rvr.check(meta, self.buf)
        # float lexemes in the snapshot are digested in repr(float) spelling
        meta["made"]["samples"]["mc"]["metadata"] = {"xsec": 1.0, "k": 1e-05, "zero": 0.0}
        meta["made"]["sha256"] = rvr.made_digest(meta["made"])
        rvr.check(meta, self.buf)
        self.assertIn('{"k":1e-05,"xsec":1.0,"zero":0.0}',
                      json.dumps({k: v for k, v in meta["made"].items() if k != "sha256"},
                                 sort_keys=True, separators=(",", ":"), ensure_ascii=False))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "study.rvr"
            rvr.write(path, meta, self.buf)
            back, _ = rvr.read(path)
        self.assertEqual(back["made"], meta["made"])

    def test_made_shapes_are_typed(self):
        cases = {
            "study_name blank": lambda m: m["made"].update(study_name=" "),
            "by not text": lambda m: m["made"].update(by=9),
            "title not text": lambda m: m["made"].update(title=1),
            "note not text": lambda m: m["made"].update(note=["a"]),
            "keep null": lambda m: m["made"].update(keep=None),
            "prepare bad kind": lambda m: m["made"].update(prepare={"kind": "shell"}),
            "samples a list": lambda m: m["made"].update(samples=[]),
            "samples missing one": lambda m: m["made"]["samples"].pop("mc"),
            "samples extra": lambda m: m["made"]["samples"].update(extra={"files": {}, "weights": None, "metadata": {}}),
            "sample no weights": lambda m: m["made"]["samples"]["mc"].pop("weights"),
            "sample metadata list": lambda m: m["made"]["samples"]["mc"].update(metadata=[]),
            "files a list": lambda m: m["made"]["samples"]["mc"].update(files=[]),
            "file with directory": lambda m: m["made"]["samples"]["mc"]["files"].update(
                u={"file": "dir/a.root", "tree": "Events", "entries": 4}),
            "file with backslash": lambda m: m["made"]["samples"]["mc"]["files"].update(
                u={"file": "dir\\a.root", "tree": "Events", "entries": 4}),
            "file entries bool": lambda m: m["made"]["samples"]["mc"]["files"].update(
                u={"file": "a.root", "tree": "Events", "entries": True}),
            "file entries negative": lambda m: m["made"]["samples"]["mc"]["files"].update(
                u={"file": "a.root", "tree": "Events", "entries": -1}),
            "file no tree": lambda m: m["made"]["samples"]["mc"]["files"].update(
                u={"file": "a.root", "entries": 4}),
            "cart without source": lambda m: m["made"].update(cart={"sha256": "0" * 64}),
            "cart digest wrong": lambda m: m["made"].update(cart={"sha256": "0" * 64, "source": "x"}),
        }
        for label, change in cases.items():
            with self.subTest(label):
                meta = copy.deepcopy(self.meta)
                change(meta)
                meta["made"]["sha256"] = rvr.made_digest(meta["made"])
                with self.assertRaises(rvr.FormatError):
                    rvr.check(meta, self.buf)
        meta = copy.deepcopy(self.meta)
        meta["made"]["samples"]["mc"]["files"]["6f0c1a2e"] = {"file": "a.root", "tree": "Events", "entries": 4}
        meta["made"]["cart"] = {"sha256": rvr.cart_digest("CART = 1\n"), "source": "CART = 1\n"}
        meta["made"]["sha256"] = rvr.made_digest(meta["made"])
        rvr.check(meta, self.buf)

    def test_column_sources_are_shape_checked_only(self):
        sources = {"pt": {"class": "copy", "branch": "Muon_pt", "dtype": "float32"},
                   "njet": {"class": "derived", "from": ["Jet_pt"], "reproducible": "source",
                            "python": "lambda events: ak.num(events.Jet_pt)"}}
        meta, buf = rvr.pack(
            {"mc": {"pt": np.array([10.5, 20.0]), "njet": np.array([0, 1])}},
            weighted={"mc": False}, sources=sources, made=snapshot("mc"))
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
                with self.assertRaises(rvr.FormatError):
                    rvr.check(bad, buf)

    def test_vals_are_uncapped_by_the_format_and_the_presser_warns(self):
        many = np.arange(rvr.MAX_VALS + 1, dtype=np.int32)
        with self.assertWarnsRegex(UserWarning, 'column "n" has more than') as caught:
            meta, buf = rvr.pack({"s": {"n": many, "few": many % 2, "f": many * 0.5}},
                                 weighted={"s": False}, made=snapshot("s"))
        self.assertEqual(len(caught.warnings), 1)
        cols = {c["name"]: c for c in meta["data"]["cols"]}
        self.assertNotIn("vals", cols["n"])
        self.assertEqual(cols["few"]["vals"], [0, 1])
        # a header that enumerates them anyway is valid when the list is true
        cols["n"]["vals"] = [int(x) for x in many]
        rvr.check(meta, buf)
        cols["n"]["vals"] = [int(x) for x in many[:-1]]
        with self.assertRaisesRegex(rvr.FormatError, "vals"):
            rvr.check(meta, buf)

    def test_names_label_every_value_once_and_live_on_the_column(self):
        meta, buf = rvr.pack({"s": {"lepton": np.array([11, 13, 15, 13])}},
                             weighted={"s": False}, made=snapshot("s"),
                             labels={"lepton": {11: "electron", 13: "muon", 15: "tau"}})
        col = meta["data"]["cols"][0]
        self.assertEqual(col["names"], {"11": "electron", "13": "muon", "15": "tau"})
        self.assertEqual(set(meta), {"rowver", "data", "made"})
        for names in ({"11": "electron"}, {"11": "e", "13": "m", "15": "t", "17": "?"},
                      {"11": "e", "13": "m", "15": 3}, ["e", "m", "t"]):
            bad = copy.deepcopy(meta)
            bad["data"]["cols"][0]["names"] = names
            with self.subTest(names=names), self.assertRaisesRegex(rvr.FormatError, "names"):
                rvr.check(bad, buf)
        bad = copy.deepcopy(meta)
        del bad["data"]["cols"][0]["vals"]
        with self.assertRaisesRegex(rvr.FormatError, "names"):
            rvr.check(bad, buf)
        for labels in ({"lepton": {11: "electron"}}, {"missing": {1: "x"}}):
            with self.subTest(labels=labels), self.assertRaises(rvr.FormatError):
                rvr.pack({"s": {"lepton": np.array([11, 13, 15, 13])}},
                         weighted={"s": False}, made=snapshot("s"), labels=labels)

    def test_empty_columns_carry_the_stub_for_every_kind(self):
        meta, buf = rvr.pack(
            {"s": {"f": np.empty(0), "i": np.empty(0, np.int32), "b": np.empty(0, bool),
                   "u": np.empty(0, np.uint16)}},
            weighted={"s": False}, made=snapshot("s"),
            sources={"u": {"class": "copy", "branch": "u", "dtype": "uint16"}})
        stub = {"lim": [0, 0], "nice": [0, 1], **{f"p{q}": 0 for q in rvr.PCTS}}
        for col in meta["data"]["cols"]:
            expected = stub if col["kind"] == "f8" else {**stub, "vals": []}
            self.assertEqual({k: col[k] for k in expected}, expected, col["name"])
        self.assertEqual([c["kind"] for c in meta["data"]["cols"]], ["f8", "i4", "b1", "u2"])
        for index, change in ((0, {"lim": [0, 1], "nice": [0, 1]}), (0, {"nice": [-1, 1]}),
                              (0, {"p50": 1}), (1, {"vals": None}), (2, {"vals": None}),
                              (3, {"vals": [0]}), (0, {"vals": []})):
            bad = copy.deepcopy(meta)
            bad["data"]["cols"][index].update(change)
            if change.get("vals", 0) is None:
                del bad["data"]["cols"][index]["vals"]
            with self.subTest(index=index, change=change), self.assertRaises(rvr.FormatError):
                rvr.check(bad, buf)

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
        with self.assertWarnsRegex(UserWarning, 'column "u8"'):
            meta, buf = rvr.pack({"s": values}, weighted={"s": False}, sources=sources,
                                 made=snapshot("s"))
        kinds = {c["name"]: c["kind"] for c in meta["data"]["cols"]}
        self.assertEqual(kinds, {"f4": "f4", "u8": "u8", "i8": "i8", "i2": "i2", "u1": "u1",
                                 "f2": "f8", "d8": "i4", "bit": "b1"})
        self.assertEqual(len(buf), 3 * (4 + 4 + 8 + 8 + 2 + 1 + 8 + 4) + 1)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "native.rvr"
            rvr.write(path, meta, buf)
            back, payload = rvr.read(path)
        columns = rvr.columns(back, payload)
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
            self.assertEqual(col["at"] % rvr.WIDTH[col["kind"]], 0, col["name"])
        self.assertEqual(back["data"]["sample_at"] % 4, 0)
        self.assertLess(back["data"]["sample_at"], min(c["at"] for c in back["data"]["cols"]
                                                       if c["kind"] in ("i2", "u1", "b1")))
        small = {name: values[name] for name in ("f4", "i2", "u1", "f2", "d8", "bit")}
        without_sources, _ = rvr.pack({"s": small}, weighted={"s": False}, made=snapshot("s"))
        self.assertEqual({c["name"]: c["kind"] for c in without_sources["data"]["cols"]},
                         {"f4": "f8", "i2": "i4", "u1": "i4", "f2": "f8", "d8": "i4", "bit": "b1"})

    def test_a_copy_without_an_exact_common_dtype_is_refused(self):
        copy_source = {"n": {"class": "copy", "branch": "n", "dtype": "uint64"}}
        made = snapshot("a", "b")
        for other in (np.array([-1], dtype=np.int64), np.array([1.5], dtype=np.float32)):
            with self.subTest(other=other.dtype), \
                    self.assertRaisesRegex(rvr.FormatError, "no exact common dtype"):
                rvr.pack({"a": {"n": np.array([7], dtype=np.uint64)}, "b": {"n": other}},
                         weighted={"a": False, "b": False}, sources=copy_source, made=made)
        # the same samples without a copy claim take the derived promotion
        meta, _ = rvr.pack({"a": {"n": np.array([7], dtype=np.uint64)},
                            "b": {"n": np.array([-1], dtype=np.int64)}},
                           weighted={"a": False, "b": False}, made=made)
        self.assertEqual(meta["data"]["cols"][0]["kind"], "i4")
        # the dtype a branch was read as decides the kind even where every kept value is narrower
        wider, _ = rvr.pack({"a": {"n": np.array([7], dtype=np.int32)}}, weighted={"a": False},
                            sources={"n": {"class": "copy", "branch": "n", "dtype": "int64"}},
                            made=snapshot("a"))
        self.assertEqual(wider["data"]["cols"][0]["kind"], "i8")
        with self.assertRaisesRegex(rvr.FormatError, "not a NumPy dtype"):
            rvr.pack({"a": {"n": np.array([7], dtype=np.int32)}}, weighted={"a": False},
                     sources={"n": {"class": "copy", "branch": "n", "dtype": "Int_t"}},
                     made=snapshot("a"))
        with self.assertRaisesRegex(rvr.FormatError, "fit exactly"):
            rvr.pack({"a": {"n": np.array([2 ** 63], dtype=np.uint64)},
                      "b": {"n": np.array([-1], dtype=np.int64)}},
                     weighted={"a": False, "b": False}, made=made)
        # a bound the header cannot hold as a float is a format error, not a crash
        with self.assertRaisesRegex(rvr.FormatError, "finite"):
            rvr.pack({"a": {"x": np.array([1.75e308])}}, weighted={"a": False}, made=snapshot("a"))

    def test_an_empty_sample_never_promotes_a_copy_before_it_is_stored(self):
        meta, buf = rvr.pack({"a": {"n": np.array([2 ** 62 + 1], dtype=np.int64)},
                              "b": {"n": np.empty(0, dtype=np.float64)}},
                             weighted={"a": False, "b": False}, made=snapshot("a", "b"),
                             sources={"n": {"class": "copy", "branch": "n", "dtype": "int64"}})
        self.assertEqual(meta["data"]["cols"][0]["kind"], "i8")
        np.testing.assert_array_equal(rvr.columns(meta, buf)["n"], [2 ** 62 + 1])


if __name__ == "__main__":
    unittest.main()
