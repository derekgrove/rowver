"""v8 jagged copies: int64 offsets plus flat content, stored for readers other than the viewer."""
import copy
import hashlib
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import patch

import awkward as ak
import numpy as np
import uproot

from rowver import Cart, CartError, press, rvr6
from rowver.rvr6 import Jagged


def jagged(lists, dtype=None):
    array = ak.Array(lists)
    content = ak.to_numpy(ak.flatten(array, axis=1))
    return Jagged(ak.to_numpy(ak.num(array, axis=1)), content if dtype is None else content.astype(dtype))


class JaggedPackTests(unittest.TestCase):
    def setUp(self):
        self.jets = {"mc": [[1.5, 2.5], [], [3.5]], "data": [[4.5, 5.5, 6.5], [7.5]]}
        self.meta, self.buf = rvr6.pack(
            {"mc": {"jet": jagged(self.jets["mc"], np.float32), "pt": np.array([10.5, 20.0, 30.0])},
             "data": {"jet": jagged(self.jets["data"], np.float32), "pt": np.array([40.0, 50.0])}},
            weighted={"mc": True, "data": False}, weights={"mc": np.array([0.5, 2.0, 1.0])},
            sources={"jet": {"class": "copy", "branch": "Jet_pt", "dtype": "float32"}})
        self.jet = self.meta["data"]["cols"][0]

    def test_offsets_and_content_are_two_aligned_spans_placed_by_width(self):
        # 5 rows, 7 items: weights 40, offsets 48, pt 40, content 28, then the index 20
        self.assertEqual((self.jet["kind"], self.jet["at"]), ("jagged", 40))
        self.assertEqual(self.jet["content"], {"kind": "f4", "at": 128, "count": 7})
        self.assertEqual(self.jet["source"], {"class": "copy", "branch": "Jet_pt", "dtype": "float32"})
        self.assertEqual((self.meta["data"]["cols"][1]["at"], self.meta["data"]["sample_at"]), (88, 156))
        self.assertEqual(len(self.buf), 176)
        self.assertFalse({"lim", "nice", "p50", "vals"} & set(self.jet))
        offsets = np.frombuffer(self.buf, "<i8", 6, 40)
        np.testing.assert_array_equal(offsets, [0, 2, 2, 3, 6, 7])
        np.testing.assert_array_equal(np.frombuffer(self.buf, "<f4", 7, 128),
                                      [1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5])
        self.assertEqual(self.jet["sha256"], hashlib.sha256(self.buf[40:88] + self.buf[128:156]).hexdigest())
        self.assertEqual(self.meta["data"]["sha256"], hashlib.sha256(self.buf).hexdigest())

    def test_readers_rebuild_the_lists_and_split_them_by_sample(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "jagged.rvr"
            rvr6.write(path, self.meta, self.buf)
            back, payload = rvr6.read(path)
            _, by_sample = rvr6.read_by_sample(path)
        self.assertEqual(payload, self.buf)
        columns = rvr6.columns(back, payload)
        self.assertEqual(columns["jet"].to_list(), self.jets["mc"] + self.jets["data"])
        self.assertEqual(str(ak.type(columns["jet"])), "5 * var * float32")
        np.testing.assert_array_equal(columns["pt"], [10.5, 20.0, 30.0, 40.0, 50.0])
        for sample, lists in self.jets.items():
            self.assertEqual(by_sample[sample]["jet"].to_list(), lists)

    def test_derived_lists_promote_like_flat_columns_and_bits_pack(self):
        meta, buf = rvr6.pack(
            {"a": {"n": jagged([[1, 2], [3]]), "flag": jagged([[True, False, True], []]),
                   "none": jagged([[], []]), "big": jagged([[2 ** 62], []], np.int64)},
             "b": {"n": Jagged(np.zeros(0, np.int64), np.empty(0, np.int64)),
                   "flag": Jagged(np.zeros(0, np.int64), np.empty(0, bool)),
                   "none": Jagged(np.zeros(0, np.int64), np.empty(0)),
                   "big": Jagged(np.zeros(0, np.int64), np.empty(0, np.int64))}},
            weighted={"a": False, "b": False},
            sources={"big": {"class": "copy", "branch": "event", "dtype": "int64"}})
        cols = {c["name"]: c for c in meta["data"]["cols"]}
        self.assertEqual({name: c["content"]["kind"] for name, c in cols.items()},
                         {"n": "i4", "flag": "b1", "none": "f8", "big": "i8"})
        self.assertEqual({name: c["content"]["count"] for name, c in cols.items()},
                         {"n": 3, "flag": 3, "none": 0, "big": 1})
        # 2 rows: four offsets arrays of 24, n 12, big 8, index 8, then one byte of bits
        self.assertEqual(len(buf), 4 * 24 + 12 + 8 + 8 + 1)
        self.assertEqual(cols["flag"]["content"]["at"], len(buf) - 1)
        self.assertEqual(buf[-1], 0b101)
        columns = rvr6.columns(meta, buf)
        self.assertEqual(columns["n"].to_list(), [[1, 2], [3]])
        self.assertEqual(columns["flag"].to_list(), [[True, False, True], []])
        self.assertEqual(columns["none"].to_list(), [[], []])
        self.assertEqual(columns["big"].to_list(), [[2 ** 62], []])
        corrupt = bytearray(buf)
        corrupt[-1] |= 0b1000
        with self.assertRaisesRegex(rvr6.FormatError, "tail bits"):
            rvr6.check(meta, corrupt)

    def test_pack_refuses_lists_it_cannot_store(self):
        def packed(value, **columns):
            rvr6.pack({"s": {"jet": value, **columns}}, weighted={"s": False})

        for bad, message in ((Jagged(np.array([-1, 3]), np.arange(2.0)), "non-negative"),
                             (Jagged(np.array([1, 2]), np.arange(2.0)), "add up"),
                             (Jagged(np.array([2 ** 63 - 1, 2 ** 63 - 1, 2]), np.empty(0)), "add up"),
                             (Jagged(np.array([2 ** 64 - 1], dtype=np.uint64), np.empty(0)), "add up"),
                             (Jagged(np.array([1.0, 1.0]), np.arange(2.0)), "non-negative"),
                             (Jagged(np.array([1, 1]), np.array([1.0, np.nan])), "NaN"),
                             (Jagged(np.array([1, 1]), np.array(["a", "b"])), "numeric"),
                             (Jagged(np.array([1, 1]), np.ma.masked_array([1.0, 2.0], [0, 1])), "masked")):
            with self.subTest(message=message), self.assertRaisesRegex(rvr6.FormatError, message):
                packed(bad)
        with self.assertRaisesRegex(rvr6.FormatError, "different lengths"):
            packed(jagged([[1.0], [2.0]]), pt=np.arange(3.0))
        with self.assertRaisesRegex(rvr6.FormatError, "some samples"):
            rvr6.pack({"a": {"jet": jagged([[1.0]])}, "b": {"jet": np.array([1.0])}},
                      weighted={"a": False, "b": False})
        with self.assertRaisesRegex(rvr6.FormatError, "categorical"):
            rvr6.pack({"a": {"jet": jagged([[1]])}}, weighted={"a": False}, labels={"jet": {1: "one"}})

    def test_check_refuses_offsets_content_and_views_that_lie(self):
        def edited(change, fix_digest=False):
            meta, buf = copy.deepcopy(self.meta), bytearray(self.buf)
            change(meta, buf)
            if fix_digest:
                meta["data"]["sha256"] = hashlib.sha256(buf).hexdigest()
            return meta, buf

        cases = {
            "offsets start late": (lambda m, b: b.__setitem__(slice(40, 48), (1).to_bytes(8, "little")), "offsets"),
            "offsets decrease": (lambda m, b: b.__setitem__(slice(56, 64), (1).to_bytes(8, "little")), "offsets"),
            "offsets overrun": (lambda m, b: b.__setitem__(slice(80, 88), (8).to_bytes(8, "little")), "offsets"),
            "offsets wrap": (lambda m, b: b.__setitem__(slice(48, 64), (2 ** 63 - 1).to_bytes(8, "little")
                                                        + (-2).to_bytes(8, "little", signed=True)), "offsets"),
            "content nan": (lambda m, b: b.__setitem__(slice(128, 132), np.array([np.nan], "<f4").tobytes()), "NaN"),
            "content unaligned": (lambda m, b: m["data"]["cols"][0]["content"].update(at=129), "unaligned"),
            "content kind": (lambda m, b: m["data"]["cols"][0]["content"].update(kind="wat"), "content"),
            "content missing": (lambda m, b: m["data"]["cols"][0].pop("content"), "content"),
            "content count": (lambda m, b: m["data"]["cols"][0]["content"].update(count=6), "bytes"),
            "content width": (lambda m, b: m["data"]["cols"][0]["content"].update(kind="f8"), "outside"),
            "offsets unaligned": (lambda m, b: m["data"]["cols"][0].update(at=44), "unaligned"),
            "offsets overlap": (lambda m, b: m["data"]["cols"][0].update(at=88), "overlaps"),
            "cut": (lambda m, b: m["tabs"].append({"name": "t", "cuts": {"jet": [0, 1]}}), "not a column"),
            "plot": (lambda m, b: m["tabs"].append({"name": "t", "stage": [{"kind": "1d", "varx": "jet"}]}), "unknown x"),
            "grid": (lambda m, b: m["tabs"].append({"name": "t", "stage": [{"kind": "map", "grid": {
                "vars": [{"col": "jet", "bands": [1]}]}}]}), "grid"),
        }
        for label, (change, message) in cases.items():
            with self.subTest(label), self.assertRaisesRegex(rvr6.FormatError, message):
                rvr6.check(*edited(change, fix_digest=True))
        # a valid but different offsets array and a moved content value fail only their digest
        for change in (lambda m, b: b.__setitem__(slice(48, 56), (1).to_bytes(8, "little")),
                       lambda m, b: b.__setitem__(slice(128, 132), np.array([1.75], "<f4").tobytes())):
            with self.assertRaisesRegex(rvr6.FormatError, 'column "jet" sha256'):
                rvr6.check(*edited(change, fix_digest=True))
        meta, buf = edited(lambda m, b: None)
        meta["rowver"] = 7
        for col in meta["data"]["cols"]:
            del col["sha256"]
        with self.assertRaisesRegex(rvr6.FormatError, "unknown kind"):
            rvr6.check(meta, buf)


class JaggedPressTests(unittest.TestCase):
    def write_events(self, path, rows):
        jets = ak.Array([[float(i + j) for j in range(i % 3)] for i in range(rows)])
        with uproot.recreate(path) as output:
            output["Events"] = {
                "pt": np.arange(rows, dtype=np.float32),
                "jet": ak.values_astype(jets, np.float32),
                "flag": ak.values_astype(jets > 2, bool),
                "n": ak.values_astype(jets, np.int64),
            }
        return jets

    def test_copies_of_jagged_branches_press_in_their_dtype_and_the_estimate_knows_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            jets = self.write_events(root / "events.root", 40)
            with uproot.recreate(root / "empty.root") as output:
                output.mktree("Events", {"pt": np.float32, "jet": "var * float32",
                                         "flag": "var * bool", "n": "var * int64"})
            (root / "cart.py").write_text(textwrap.dedent("""
                import awkward as ak
                from rowver import Cart

                CART = Cart(
                    fileset={"mc": {"files": {"events.root": "Events"}, "metadata": {"weights": None, "min": 0}},
                             "rejected": {"files": {"events.root": "Events"}, "metadata": {"weights": None, "min": 99}},
                             "empty": {"files": {"empty.root": "Events"}, "metadata": {"weights": None, "min": 0}}},
                    keep=lambda events, metadata: events.pt >= metadata["min"],
                    columns={"jet": "jet", "flag": "flag", "n": "n", "pt": "pt",
                             "twice": lambda events: 2 * events.jet,
                             "njet": lambda events: ak.num(events.jet)},
                )
            """), encoding="utf-8")
            expected = jets.to_list()
            # 40 rows, 39 items: index 160 + pt 160 + njet 160, four offsets arrays of 328,
            # jet 156, flag 5, n 312, twice 312
            size = 160 * 3 + 4 * 328 + 156 + 5 + 312 + 312
            for workers in (None, 2):
                with self.subTest(workers=workers):
                    with patch.object(rvr6, "MAX_DATA_BYTES", size - 1):
                        with self.assertRaisesRegex(CartError, "no longer fit"):
                            Cart.load(root).build(root / "never.rvr", step_size=7, workers=workers)
                    with patch.object(rvr6, "MAX_DATA_BYTES", size):
                        out = Cart.load(root).build(root / f"{workers}.rvr", step_size=7, workers=workers)
                    meta, buf = rvr6.read(out["file"])
                    self.assertEqual(len(buf), size)
                    cols = {c["name"]: c for c in meta["data"]["cols"]}
                    self.assertEqual({name: (c["kind"], c.get("content", {}).get("kind")) for name, c in cols.items()},
                                     {"jet": ("jagged", "f4"), "flag": ("jagged", "b1"), "n": ("jagged", "i8"),
                                      "pt": ("f4", None), "twice": ("jagged", "f8"), "njet": ("i4", None)})
                    self.assertEqual({name: cols[name]["source"]["dtype"] for name in ("jet", "flag", "n")},
                                     {"jet": "float32", "flag": "bool", "n": "int64"})
                    self.assertEqual(cols["twice"]["source"]["from"], ["jet"])
                    self.assertEqual(cols["twice"]["source"]["reproducible"], "cartridge")
                    self.assertEqual([s["rows"] for s in meta["data"]["samples"]], [40, 0, 0])
                    columns = rvr6.columns(meta, buf)
                    self.assertEqual(columns["jet"].to_list(), expected)
                    self.assertEqual(columns["flag"].to_list(), (jets > 2).to_list())
                    self.assertEqual(columns["n"].to_list(), ak.values_astype(jets, np.int64).to_list())
                    self.assertEqual(columns["twice"].to_list(), (2 * jets).to_list())
                    np.testing.assert_array_equal(columns["njet"], np.arange(40) % 3)

    def test_a_column_keeps_one_shape_across_chunks_and_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.write_events(root / "events.root", 6)
            source = {"files": {str(root / "events.root"): "Events"}}

            def skim(columns, keep=lambda e: e.pt >= 0, samples=("a",), step_size=2):
                cart = Cart(fileset={s: {**source, "metadata": {"weights": None, "lists": s == "a"}}
                                     for s in samples}, keep=keep, columns=columns)
                return cart.skim(step_size=step_size)[0]

            with self.assertRaisesRegex(CartError, "some chunks"):
                skim({"x": lambda e: e.jet if e.pt[0] < 2 else e.pt})
            with self.assertRaisesRegex(CartError, "some samples"):
                skim({"x": lambda e, m: e.jet if m["lists"] else e.pt}, samples=("a", "b"))
            # an empty flat chunk beside lists is an empty list chunk
            rows = skim({"x": lambda e: np.array([]) if len(e) == 0 else e.jet}, keep=lambda e: e.pt >= 4)
            self.assertEqual(ak.to_list(ak.unflatten(rows["a"]["x"].content, rows["a"]["x"].counts)),
                             [[4.0], [5.0, 6.0]])
            for value, message in ((ak.Array([[[1.0], []]] * 6), "regular value"),
                                   (ak.Array([[1.0, None]] * 6), "missing"),
                                   (ak.Array([[{"x": 1.0}]] * 6), "numeric"),
                                   (ak.Array([["a"]] * 6), "numeric"),
                                   (np.zeros((6, 2)), "shape")):
                with self.subTest(message=message), self.assertRaisesRegex(CartError, message):
                    press._column(value, "s.x")
            with self.assertRaisesRegex(CartError, "returned 3 values"):
                skim({"x": lambda e: e.jet[:3]}, step_size=6)


if __name__ == "__main__":
    unittest.main()
