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
                from rowver import Cart

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

    def test_provenance_names_each_column_source_and_each_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ntuple = root / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {
                    "pt": np.array([5.0, 15.0, 25.0, 35.0], dtype=np.float32),
                    "jet": ak.Array([[], [1.0], [2.0, 3.0], [4.0, 5.0, 6.0]]),
                }
            study = root / "mine"
            study.mkdir()
            (study / "cart.py").write_text(textwrap.dedent(f"""
                import awkward as ak
                from rowver import Cart

                def ht(events):
                    return ak.sum(events.jet, axis=1)

                CART = Cart(
                    fileset={{"mc": {{"files": {{{str(ntuple)!r}: "Events"}},
                                      "metadata": {{"weights": None}}}}}},
                    keep=lambda events: events.pt > 10,
                    columns={{"pt": "pt", "ht": ht, "twice": lambda events: 2 * events.pt}},
                )
            """), encoding="utf-8")
            result = Cart.load(study).build(root / "mine.rvr", step_size=2)
            meta, _ = rvr6.read(result["file"])
            with uproot.open(ntuple) as file:
                uuid = str(file.file.uuid)
            files = meta["made"]["samples"][0]["files"]
            self.assertEqual(Path(files[0]["path"]), ntuple.resolve())

        sources = {c["name"]: c["source"] for c in meta["data"]["cols"]}
        self.assertEqual(sources["pt"], {"class": "copy", "branch": "pt", "dtype": "float32"})
        self.assertEqual(sources["ht"]["from"], ["jet"])
        self.assertEqual(sources["ht"]["reproducible"], "source")
        self.assertTrue(sources["ht"]["python"].startswith("def ht(events):"))
        # KEEP already read pt, so only a fresh view can attribute it to the column
        self.assertEqual(sources["twice"]["from"], ["pt"])
        self.assertEqual(sources["twice"]["reproducible"], "cartridge")
        self.assertEqual([(f["tree"], f["uuid"], f["entries"]) for f in files],
                         [("Events", uuid, 4)])

    def test_inputs_include_a_branch_a_column_first_reads_later_in_the_file(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                tree = output.mktree("Events", {"a": np.int32, "heavy": "var * float64"})
                tree.extend({"a": np.arange(5000, dtype=np.int32),
                             "heavy": ak.Array([[1.0] * 30] * 5000)})

            def late(events):
                if ak.any(events.a > 1500):
                    return ak.num(events.heavy)
                return events.a * 0

            # KEEP reads heavy from the first chunk, so its later read by the column
            # is invisible to the batched-read bookkeeping and must be seen directly
            cart = Cart(
                fileset={"mc": {"files": {str(ntuple): "Events"},
                                "metadata": {"weights": None}}},
                keep=lambda events: ak.num(events.heavy) >= 0, columns={"a": "a", "late": late},
            )
            _, _, trace = cart.skim(step_size="4 kB")
        self.assertEqual(trace.inputs, {"a": {"a"}, "late": {"a", "heavy"}})
        self.assertEqual(trace.derived, {"late"})

    def test_inputs_follow_prepare_only_through_the_fields_it_makes(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"a": np.arange(4.0), "b": np.arange(4.0) - 1.5,
                                    "c": np.ones(4)}

            def prepare(events):
                kept = events[events.b > -1]
                kept["twice_c"] = 2 * kept.c
                return kept

            cart = Cart(
                fileset={"mc": {"files": {str(ntuple): "Events"},
                                "metadata": {"weights": None}}},
                prepare=prepare, keep=lambda events: events.a >= 0,
                columns={"a": "a", "plain": lambda e: e.a + 1, "made": lambda e: e.twice_c},
            )
            _, _, trace = cart.skim()
        # a column that reads only branches is not charged with PREPARE's own reads;
        # one that reads a field PREPARE made needs everything PREPARE read
        self.assertEqual(trace.inputs, {"a": {"a"}, "plain": {"a"}, "made": {"b", "c"}})
        self.assertEqual(trace.derived, {"plain", "made"})

    def test_the_events_array_records_selections_and_flags_whole_record_use(self):
        from rowver import press
        events = press._watched(ak.Array([{"a": 1.0, "b": 2.0}] * 3))
        probes = {
            "attribute": (lambda e: e.a, {"a"}),
            "item": (lambda e: e["a"], {"a"}),
            "several": (lambda e: e[["a", "b"]], {"a", "b"}),
            "row mask": (lambda e: e[e.a > 0], {"a"}),
            "zip of a field": (lambda e: ak.zip({"x": e.b}), {"b"}),
            "fields and len": (lambda e: (e.fields, len(e)), set()),
            "one record": (lambda e: e[0].b, {"*"}),
            "tuple with a row": (lambda e: e["b", 0], {"*", "b"}),
            "iteration": (lambda e: [r.b for r in e], {"*"}),
            "to_list": (lambda e: e.to_list(), {"*"}),
            "unzip": (lambda e: ak.unzip(e), {"*"}),
            "with_field": (lambda e: ak.with_field(e, e.a, "c"), {"*", "a"}),
        }
        for label, (probe, expected) in probes.items():
            press._READS.clear()
            probe(events)
            with self.subTest(label):
                self.assertEqual(press._READS, expected)

    def test_hooks_the_presser_cannot_see_through_widen_the_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"a": np.arange(4.0), "b": np.arange(4.0) + 10}

            def skim(prepare, columns):
                cart = Cart(fileset={"mc": {"files": {str(ntuple): "Events"},
                                            "metadata": {"weights": None}}},
                            prepare=prepare, keep=lambda events: events.a >= 0,
                            columns=columns)
                rows, _, trace = cart.skim()
                return rows["mc"], trace.inputs, cart._sources(trace)

            # a column that takes the record apart may have used any branch
            rows, inputs, sources = skim(None, {"a": "a", "d": lambda e: ak.unzip(e)[1]})
            np.testing.assert_array_equal(rows["d"], np.arange(4.0) + 10)
            self.assertEqual(inputs, {"a": {"a"}, "d": {"a", "b"}})
            self.assertEqual(sources["d"]["reproducible"], "source")
            self.assertEqual(sources["a"]["class"], "copy")
            # a PREPARE that returns a rebuilt record makes every column derived from what it read
            rows, inputs, sources = skim(lambda e: ak.zip({"a": e.b}),
                                         {"a": "a", "d": lambda e: e.a})
            np.testing.assert_array_equal(rows["a"], np.arange(4.0) + 10)
            self.assertEqual(inputs, {"a": {"a", "b"}, "d": {"a", "b"}})
            self.assertEqual({name: (s["class"], s["from"], s["reproducible"])
                              for name, s in sources.items()},
                             {"a": ("derived", ["a", "b"], "source"),
                              "d": ("derived", ["a", "b"], "source")})
            self.assertIn("ak.zip", sources["a"]["python"])
            # so does one that rewrites a branch's field under its own name: every column
            # keeps its own reads and adds PREPARE's, which are every branch after
            # with_field (it takes the whole record) and exactly b after an assignment
            for prepare, added in ((lambda e: ak.with_field(e, e.b, "a"), {"a", "b"}),
                                   (lambda e: (e.__setitem__("a", e.b), e)[1], {"b"})):
                rows, inputs, sources = skim(prepare, {"a": "a", "d": lambda e: e.a,
                                                       "c": lambda e: e.b})
                np.testing.assert_array_equal(rows["a"], np.arange(4.0) + 10)
                self.assertEqual(inputs, {"a": {"a"} | added, "d": {"a"} | added,
                                          "c": {"b"} | added})
                self.assertEqual({name: s["class"] for name, s in sources.items()},
                                 {"a": "derived", "d": "derived", "c": "derived"})
            # a column one sample derives and another copies is derived from both
            cart = Cart(fileset={"swapped": {"files": {str(ntuple): "Events"},
                                             "metadata": {"weights": None, "swap": True}},
                                 "plain": {"files": {str(ntuple): "Events"},
                                           "metadata": {"weights": None, "swap": False}}},
                        prepare=lambda e, m: ak.zip({"a": e.b}) if m["swap"] else e,
                        keep=lambda events: events.a >= 0, columns={"a": "a"})
            rows, _, trace = cart.skim()
            np.testing.assert_array_equal(rows["plain"]["a"], np.arange(4.0))
            self.assertEqual(cart._sources(trace)["a"]["from"], ["a", "b"])
            # a KEEP that assigns fields reshapes the record the columns see
            def keep_rewriting_a(e):
                e["a"] = e.b
                return e.a > 0

            def keep_making_new(e):
                e["new"] = e.b
                return e.a >= 0

            cart = Cart(fileset={"mc": {"files": {str(ntuple): "Events"},
                                        "metadata": {"weights": None}}},
                        keep=keep_rewriting_a, columns={"a": "a"})
            rows, _, trace = cart.skim()
            np.testing.assert_array_equal(rows["mc"]["a"], np.arange(4.0) + 10)
            source = cart._sources(trace)["a"]
            self.assertEqual((source["class"], source["from"]), ("derived", ["a", "b"]))
            self.assertIn("def keep_rewriting_a", source["python"])
            cart = Cart(fileset={"mc": {"files": {str(ntuple): "Events"},
                                        "metadata": {"weights": None}}},
                        keep=keep_making_new, columns={"new": "new"})
            rows, _, trace = cart.skim()
            source = cart._sources(trace)["new"]
            self.assertEqual((source["class"], source["from"]), ("derived", ["a", "b"]))
            self.assertIn("def keep_making_new", source["python"])
            # KEEP stays observed after PREPARE hands it a rebuilt record
            def keep_after_zip(e):
                e["a"] = 2 * e.b
                return e.a > 0

            cart = Cart(fileset={"mc": {"files": {str(ntuple): "Events"},
                                        "metadata": {"weights": None}}},
                        prepare=lambda e: ak.zip({"a": e.a, "b": e.b}), keep=keep_after_zip,
                        columns={"a": "a"})
            rows, _, trace = cart.skim()
            np.testing.assert_array_equal(rows["mc"]["a"], 2 * (np.arange(4.0) + 10))
            self.assertEqual(trace.writers, {("prepare",), ("keep",)})
            self.assertIn("def keep_after_zip", cart._sources(trace)["a"]["python"])
            # a record PREPARE names and equips with its own behavior keeps both
            class EventArray(ak.Array):
                @property
                def twice(self):
                    return 2 * self.a

            def named(e):
                return ak.with_name(e, "Event", behavior={("*", "Event"): EventArray})

            for prepare, expected in ((named, 2 * np.arange(1.0, 4.0)),
                                      (lambda e: named(e)[[3, 1]], np.array([6.0, 2.0]))):
                cart = Cart(fileset={"mc": {"files": {str(ntuple): "Events"},
                                            "metadata": {"weights": None}}},
                            prepare=prepare, keep=lambda e: e.twice > 0,
                            columns={"t": lambda e: e.twice, "a": "a"})
                rows, _, trace = cart.skim()
                np.testing.assert_array_equal(rows["mc"]["t"], expected)
                self.assertEqual({name: s["class"] for name, s in cart._sources(trace).items()},
                                 {"t": "derived", "a": "derived"})
            # a column named like a hook is still a column
            rows, inputs, sources = skim(lambda e: (e.__setitem__("a", e.b), e)[1],
                                         {"prepare": "a", "keep": lambda e: e.a})
            self.assertEqual({name: s["class"] for name, s in sources.items()},
                             {"prepare": "derived", "keep": "derived"})
            self.assertIn('e.__setitem__("a", e.b)', sources["prepare"]["python"])
            # a column that assigns a field reshapes the record for every column
            rows, inputs, sources = skim(None, {"rewrite": lambda e: (e.__setitem__("a", e.b), e.a)[1],
                                                "a": "a"})
            np.testing.assert_array_equal(rows["a"], np.arange(4.0) + 10)
            self.assertEqual(inputs, {"rewrite": {"a", "b"}, "a": {"a", "b"}})
            self.assertEqual(sources["a"]["class"], "derived")
            self.assertIn("rewrite", sources["a"]["python"])
            # a string column naming a field PREPARE made is derived, not a copy
            rows, inputs, sources = skim(lambda e: (e.__setitem__("twice_b", 2 * e.b), e)[1],
                                         {"a": "a", "d": "twice_b"})
            np.testing.assert_array_equal(rows["d"], 2 * (np.arange(4.0) + 10))
            self.assertEqual(inputs, {"a": {"a"}, "d": {"b"}})
            self.assertEqual(sources["a"]["class"], "copy")
            self.assertEqual((sources["d"]["class"], sources["d"]["from"]), ("derived", ["b"]))
            self.assertIn("twice_b", sources["d"]["python"])

    def test_copy_dtype_comes_from_the_chunks_that_ran(self):
        with tempfile.TemporaryDirectory() as directory:
            empty, full = Path(directory) / "empty.root", Path(directory) / "full.root"
            with uproot.recreate(empty) as output:
                output.mktree("Events", {"pt": np.float32})
            with uproot.recreate(full) as output:
                output["Events"] = {"pt": np.array([1.0, 2.0], dtype=np.float32)}
            cart = Cart(fileset={"none": {"files": {str(empty): "Events"},
                                          "metadata": {"weights": None}},
                                 "rejected": {"files": {str(full): "Events"},
                                              "metadata": {"weights": None}}},
                        keep=lambda events: events.pt < 0, columns={"pt": "pt"})
            rows, _, trace = cart.skim()
        self.assertEqual(rows["none"]["pt"].dtype, np.float64)
        self.assertEqual(cart._sources(trace)["pt"]["dtype"], "float32")

    def test_a_column_that_never_ran_has_no_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ntuple = root / "events.root"
            with uproot.recreate(ntuple) as output:
                output.mktree("Events", {"pt": np.float32})
            study = root / "mine"
            study.mkdir()
            (study / "cart.py").write_text(textwrap.dedent(f"""
                from rowver import Cart

                CART = Cart(
                    fileset={{"mc": {{"files": {{{str(ntuple)!r}: "Events"}},
                                      "metadata": {{"weights": None}}}}}},
                    keep=lambda events: events.pt > 0,
                    columns={{"pt": "pt", "twice": lambda events: 2 * events.pt}},
                )
            """), encoding="utf-8")
            result = Cart.load(study).build(root / "mine.rvr")
            meta, _ = rvr6.read(result["file"])
        self.assertEqual(meta["data"]["rows"], 0)
        self.assertEqual([("source" in col) for col in meta["data"]["cols"]], [False, False])
        self.assertEqual(meta["made"]["samples"][0]["files"][0]["entries"], 0)

    def test_file_identity_is_kept_per_tree_when_samples_share_a_file(self):
        with tempfile.TemporaryDirectory() as directory:
            ntuple = Path(directory) / "events.root"
            with uproot.recreate(ntuple) as output:
                output["Events"] = {"x": np.arange(5.0)}
                output["Runs"] = {"x": np.arange(2.0)}
            cart = Cart(
                fileset={"events": {"files": {str(ntuple): "Events"},
                                    "metadata": {"weights": None}},
                         "runs": {"files": {str(ntuple): "Runs"},
                                  "metadata": {"weights": None}}},
                keep=lambda events: events.x >= 0, columns={"x": "x"},
            )
            _, _, trace = cart.skim()
        self.assertEqual({key[1]: value["entries"] for key, value in trace.files.items()},
                         {"Events": 5, "Runs": 2})

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
            rows, _, _ = cart.skim(step_size="4 kB",
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
            rows, _, _ = cart.skim(step_size="2 kB",
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
            rows, _, _ = cart.skim(step_size="1 kB")
        np.testing.assert_array_equal(rows["mc"]["a"], np.arange(2500))


if __name__ == "__main__":
    unittest.main()
