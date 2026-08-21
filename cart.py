"""A cart: one study's fileset and config in one object, and the .rvr it burns to.

A study is ONE file, studies/<name>/cart.py, holding a Cart named CART. There is no
fileset.json riding alongside — the fileset is an ordinary python dict in coffea's shape,

    {sample name: {"files": {path: tree name, ...}, "metadata": {...}}}

and `CART.fileset_json()` dumps it whenever you want to read it as JSON.

A path may be a glob, so a sample split over 32 files is one entry, and each path carries
its own tree name — which is how one file holding 231 mass-point trees supplies several
samples. A path may also be a remote URL (root://, https://), read in place, never globbed.

EVERY HOOK MAY TAKE A SECOND ARGUMENT, that sample's metadata: KEEP, PREPARE, and any
COLUMNS entry that is a function. Write the one-argument form and it still works. The
metadata handed over is the sample's own plus three keys the skim knows and the fileset
cannot — `sample`, `file` and `tree` — so a column can depend on which file it is reading.

WEIGHTS ARE PER SAMPLE. `metadata["weights"]` says what fills this sample's weights
column, in the same grammar as COLUMNS: a branch name, or a function. A sample that
declares none is UNWEIGHTED — its rows are 1.0 and the page never scales them by
luminosity — which is how real data and MC live in one cart.
"""
from __future__ import annotations

import glob
import hashlib
import inspect
import json
import os
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import uproot

import page
import pikl

STUDIES: Path = Path(__file__).with_name("studies")

#: keys the skim adds to every sample's metadata; a fileset may not also use them
RESERVED = ("sample", "file", "tree")

#: near-misses for `weights`; spelt any of these, a sample is silently unweighted
NEARLY_WEIGHTS = ("weight", "Weights", "WEIGHTS", "weigths", "WEIGHTS_BRANCH", "HAS_WEIGHTS")

#: column names that mean a config was half-converted — weights still live in COLUMNS
WEIGHT_COLUMNS = ("weight", "weights", "genWeight", "Weight", "evtWeight")


def is_remote(path: str) -> bool:
    """A path with a scheme — root://, https:// — lives on the network, not this disk."""
    return "://" in path


def expand_globs(fileset: dict) -> dict:
    """Turn every local glob into the real paths it matches, leaving remote urls alone."""
    out = {}
    for name, sample in fileset.items():
        assert isinstance(sample, dict) and "files" in sample, (
            f"sample {name!r} is {type(sample).__name__}, not "
            f"{{'files': {{path: tree}}, 'metadata': {{...}}}}")
        unknown = sorted(set(sample) - {"files", "metadata"})
        assert not unknown, (
            f"sample {name!r} has unknown key(s) {unknown}. An entry holds 'files' and "
            f"'metadata' — is one of those a typo? A misspelt 'metadata' is dropped whole, "
            f"weights and all, and the sample builds silently at weight 1.0.")
        assert isinstance(sample["files"], dict), (
            f"sample {name!r}: 'files' is {type(sample['files']).__name__}, not a dict. "
            f"Each path names its own tree: {{'/data/*.root': 'Events'}}")
        paths = {}
        for pattern, tree in sample["files"].items():
            local_glob = not is_remote(pattern) and any(c in pattern for c in "*?[")
            paths |= {hit: tree for hit in (sorted(glob.glob(pattern)) if local_glob
                                            else [pattern])}
        out[name] = {"files": paths, "metadata": dict(sample.get("metadata") or {})}
    return out


def recover_labels(header: dict) -> dict | None:
    """The LABELS a built file implies, read back off its chip columns."""
    return {c["name"]: dict(zip(c["vals"], c["names"]))
            for c in header["cols"] if c.get("names")} or None


def column_shape(header: dict) -> list[tuple]:
    """Each column a built file declares, as (name, kind), in order.

    The KIND matters as much as the name: pack() takes a column's type from the samples
    it is given, so merging an int column into a float one of the same name truncates it."""
    return [(c["name"], c["kind"]) for c in header["cols"]]


def describe(spec) -> str:
    """How something was made, in words: a branch name, a lambda's source, or a function."""
    if isinstance(spec, str):
        return spec
    if getattr(spec, "__name__", "<lambda>") != "<lambda>":
        doc = (spec.__doc__ or "").strip().splitlines()
        return f"{spec.__name__}()" + (f" — {doc[0]}" if doc else "")
    try:
        return " ".join(inspect.getsource(spec).split()).rstrip(",")
    except (OSError, TypeError):
        return "<lambda>"


def study_names() -> list[str]:
    """Names of every study in studies/ that has a cart.py."""
    return sorted(p.name for p in STUDIES.iterdir() if (p / "cart.py").exists())


def import_file(path: Path, name: str):
    """Import a .py file that is not on the import path."""
    import importlib.util
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(mod)
    except ModuleNotFoundError as e:
        raise ModuleNotFoundError(
            f"{path} imports {e.name!r}, which is not installed.\n"
            f"  A cart is ordinary python and may import whatever your selection needs, "
            f"so a study can require packages this tool does not. studies/template/ needs "
            f"nothing beyond uproot and awkward — copy that one as a starting point.") from e
    return mod


class Cart:
    """One study: its fileset, its selection, its columns — and the .rvr it burns to."""

    def __init__(self, name: str, fileset: dict, columns: dict, keep,
                 prepare=None, labels: dict | None = None,
                 weights_column: str = "weight"):
        """Hold a study, with its globs expanded and everything about it checked."""
        self.name = name
        self.fileset = expand_globs(fileset)
        self.columns = columns
        self.keep = keep
        self.prepare = prepare
        self.labels = labels
        declared = any(map(self.weights_spec, self.fileset))
        assert weights_column or not declared, (
            f"{name}: weights_column is None while samples declare weights — every one of "
            f"those declarations would be dropped and every event stored at 1.0.")
        # the column exists only if some sample declares weights; nothing else to set
        self.weights_column = weights_column if declared else None
        # the file the CART was written in, for the stamp — its caller's, not this module's
        caller = inspect.currentframe().f_back
        self.source = caller.f_globals.get("__file__") if caller else None
        self.dir = Path(self.source).parent if self.source else None
        self.validate()

    def validate(self) -> None:
        """Refuse a cart that cannot skim, before anything slow starts."""
        assert self.fileset, f"{self.name}: the fileset is empty"
        assert self.columns, f"{self.name}: COLUMNS is empty"
        assert callable(self.keep), f"{self.name}: keep is not a function"
        assert self.prepare is None or callable(self.prepare), \
            f"{self.name}: prepare is not a function"
        for sample, entry in self.fileset.items():
            clash = sorted(set(RESERVED) & set(entry["metadata"]))
            assert not clash, (
                f"{self.name}: sample {sample!r} has metadata {clash}, which the skim sets "
                f"itself — every hook is handed the file it is reading under those names. "
                f"Rename them.")
            nearly = sorted(set(NEARLY_WEIGHTS) & set(entry["metadata"]))
            assert not nearly, (
                f"{self.name}: sample {sample!r} has metadata {nearly}. The key is "
                f"'weights', plural — spelt any other way the sample is silently given "
                f"weight 1.0 and its yield is quietly wrong.")
            spec = entry["metadata"].get("weights")
            assert spec is None or callable(spec) or (isinstance(spec, str) and spec), (
                f"{self.name}: sample {sample!r} metadata['weights'] is "
                f"{spec!r} — it must be a branch name, a function, or None for real data.")
        for column, spec in self.columns.items():
            assert isinstance(spec, str) or callable(spec), (
                f"{self.name}: columns[{column!r}] is neither a branch name nor a function")
        self.check_weights_declared()

    def check_weights_declared(self) -> None:
        """Refuse a fileset that is silent about weights, which is how a yield goes wrong.

        Silence and "no weights" look identical in a dict, so every sample must SAY which
        it is once any sample does. The half-converted config — weights still sitting in
        COLUMNS, nothing in any metadata — is the case that used to build cleanly and
        ship every MC sample at weight 1.0."""
        said = [s for s, e in self.fileset.items() if "weights" in e["metadata"]]
        missing = sorted(set(self.fileset) - set(said))
        assert not said or not missing, (
            f"{self.name}: {len(said)} of {len(self.fileset)} samples declare weights, so "
            f"{len(missing)} are silent — and silence is indistinguishable from data, "
            f"which is how MC gets weight 1.0. Every sample must say: a branch name (or a "
            f"function) for MC, or 'weights': None for real data. Silent: {missing[:6]}")
        if self.weights_column:
            assert self.weights_column not in self.columns, (
                f"{self.name}: columns defines {self.weights_column!r}, which is the "
                f"weights column. Put the branch name in each sample's metadata as "
                f"{{'weights': '<branch>'}} and drop it from columns.")
            return
        stray = sorted(set(WEIGHT_COLUMNS) & set(self.columns))
        assert not stray, (
            f"{self.name}: no sample declares weights, but columns defines {stray} — this "
            f"cart looks half-converted, and it would build cleanly with every event at "
            f"weight 1.0. Put the branch name in each sample's metadata as "
            f"{{'weights': '<branch>'}} and drop {stray} from columns; a real-data sample "
            f"says {{'weights': None}}.")

    def weights_spec(self, sample: str):
        """What fills this sample's weights column — a branch, a function, or None."""
        return self.fileset[sample]["metadata"].get("weights")

    def weighted(self, sample: str) -> bool:
        """Whether this sample carries real weights, or is data counted one event at a time."""
        return self.weights_spec(sample) is not None

    def files(self) -> dict[str, dict[str, str]]:
        """{sample: {path: tree}} — the fileset with the metadata stripped off."""
        return {name: entry["files"] for name, entry in self.fileset.items()}

    def hook_meta(self, sample: str, path: str = "", tree: str = "") -> dict:
        """What a hook is handed: this sample's metadata, plus the file being read."""
        return {**self.fileset[sample]["metadata"],
                "sample": sample, "file": path, "tree": tree}

    def count_files(self) -> int:
        """Confirm every sample matches a file and every file holds its tree; count them."""
        problems = []
        for name, paths in self.files().items():
            if not paths:
                problems.append(f"{name}: matched no files")
            for path, tree in paths.items():
                if not is_remote(path) and not os.path.exists(path):
                    problems.append(f"{name}: no such file {path}")
                    continue
                try:
                    rootfile = uproot.open(path)
                except Exception as e:
                    problems.append(f"{name}: could not open {path} — {e}")
                    continue
                if tree not in rootfile:
                    problems.append(f"{name}: {os.path.basename(path)} has no tree {tree!r}")
        assert not problems, ("bad fileset:\n  " + "\n  ".join(problems)
                         + f"\n\nPaths are absolute and machine-specific. Edit "
                           f"{self.source} to point at your own files.")
        return sum(len(p) for p in self.files().values())

    def summary(self) -> str:
        """The cartridge label: what is on this cart and what was done to it."""
        lines = [f"cart {self.name}" + (f"  ({self.source})" if self.source else "")]
        for name, entry in self.fileset.items():
            trees = ",".join(sorted(set(entry["files"].values())))
            spec = self.weights_spec(name)
            lines.append(f"  {name:34s} {len(entry['files']):>3} files  {trees:14s} "
                         f"weights={describe(spec) if spec is not None else 'NONE (data)'}")
        lines.append(f"  {len(self.columns)} columns, keep: {describe(self.keep)}")
        return "\n".join(lines)

    def unweighted_samples(self) -> list[str]:
        """Every sample carrying no weights — the ones the page counts one event at a time."""
        return [s for s in self.fileset if not self.weighted(s)]

    def mixed_warning(self) -> str:
        """The line to print when a cart holds both weighted and unweighted samples."""
        data, mc = self.unweighted_samples(), [s for s in self.fileset if self.weighted(s)]
        if not (data and mc):
            return ""
        return (f"\n  NOTE: {len(data)} sample(s) declare no weights and are counted one "
                f"event at a time,\n        never scaled by luminosity: "
                f"{', '.join(data[:4])}{' ...' if len(data) > 4 else ''}\n"
                f"        Group them apart from the {len(mc)} weighted sample(s), or the "
                f"page will flag the mix.")

    def skim(self, step: int = pikl.STEP, dest: str | Path | None = None,
             jobs: int = 1, threads: int = 0) -> dict:
        """Read every sample, keep what passes, and write the pikl of columns."""
        return pikl.skim_all(self, self.name, step, dest or self.dir, jobs, threads)

    def recipe(self) -> dict:
        """How this cart makes columns: the selection, the columns, and the file saying so."""
        return {"columns": {c: describe(spec) for c, spec in self.columns.items()},
                "keep": describe(self.keep),
                "prepare": describe(self.prepare) if self.prepare else None,
                "config": self.config_stamp()}

    def stamp(self, counts: dict[str, int] | None = None, skim: dict | None = None) -> dict:
        """What goes in META['cart']: everything this file can still say about itself."""
        counts, skim = counts or {}, skim or {}
        samples = {}
        # a subset build carries only the samples it wrote — the stamp describes THIS file
        for name in (counts or self.fileset):
            entry, spec = self.fileset[name], self.weights_spec(name)
            samples[name] = {
                "n": counts.get(name),
                "weights": describe(spec) if spec is not None else None,
                "files": entry["files"],
                "metadata": {k: describe(v) if callable(v) else v
                             for k, v in entry["metadata"].items()}}
        # the recipe the SKIM ran, when the pikl recorded one. cart.py may have been
        # edited since, and stamping today's selection over yesterday's columns is the one
        # way a cartridge can lie with a sha256 vouching for it
        return {"name": self.name,
                "built": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "skimmed": skim.get("skimmed"),
                "samples": samples,
                **(skim.get("recipe") or self.recipe())}

    def config_stamp(self) -> dict | None:
        """The file that defined this cart: where it was, when, its hash, and its text."""
        if not self.source or not Path(self.source).exists():
            return None
        text = Path(self.source).read_text()
        return {"path": str(self.source),
                "mtime": datetime.fromtimestamp(Path(self.source).stat().st_mtime,
                                                timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "sha256": hashlib.sha256(text.encode()).hexdigest(),
                "source": text}

    def fileset_json(self) -> str:
        """The fileset as JSON — for reading, for diffing, for handing to something else."""
        return json.dumps(self.fileset, indent=2, default=describe)

    def write(self, rows: dict, dest: str | Path | None = None, name: str = "",
              tabs: list | None = None, extra: dict | None = None,
              skim: dict | None = None) -> dict:
        """Burn the cart: the skimmed columns and everything known about them, as one .rvr."""
        counts = {s: len(next(iter(c.values()))) for s, c in rows.items()}
        # a SUBSET may hold none of the weighted samples, and then the file has no weights
        column = self.weights_column if any(map(self.weighted, rows)) else None
        return page.data(rows, name or self.name, labels=self.labels,
                         dest=dest or self.dir, weights=column,
                         tabs=tabs, extra=extra, cart=self.stamp(counts, skim))

    @classmethod
    def load(cls, name: str) -> "Cart":
        """Read studies/<name>/cart.py and hand back the CART it defines."""
        path = STUDIES / name / "cart.py"
        assert path.exists(), f"no cart at {path} — have: {study_names()}"
        mod = import_file(path, f"studies.{name}.cart")
        for retired in ("HAS_WEIGHTS", "WEIGHTS_BRANCH"):
            assert not hasattr(mod, retired), (
                f"{path} still defines {retired}, which nothing reads any more. Weights "
                f"are per sample: put the branch name in each sample's metadata as "
                f"{{'weights': '<branch>'}}, and {{'weights': None}} for real data. Left "
                f"as it is, every sample here would be built at weight 1.0.")
        assert hasattr(mod, "CART"), (
            f"{path} defines no CART. A study ends with one:\n\n"
            f"    CART = Cart(name='{name}', fileset=FILESET, columns=COLUMNS, keep=KEEP)\n")
        return mod.CART

    @classmethod
    def open(cls, path: str | Path) -> "Cart":
        """Read a built .rvr back: what is on it, where it came from, and its columns."""
        header, _ = page.read(path)
        stamp = header.get("cart") or {}
        cart = cls.__new__(cls)
        cart.name = stamp.get("name") or Path(path).stem
        # the stamp keeps each sample's weights in its own field, which is the schema and
        # the only place a builder-made file records them; put it back where the cart
        # reads it, so an opened file answers weights_spec() like the one that wrote it
        cart.fileset = {n: {"files": s.get("files") or {},
                            "metadata": {**(s.get("metadata") or {}),
                                         "weights": s.get("weights")}}
                        for n, s in (stamp.get("samples") or {}).items()}
        cart.columns = stamp.get("columns") or {c["name"]: "?" for c in header["cols"]}
        cart.keep = stamp.get("keep") or ""
        cart.prepare = stamp.get("prepare")
        cart.labels = recover_labels(header)
        cart.weights_column = header.get("weights") or None
        cart.source = (stamp.get("config") or {}).get("path")
        cart.dir, cart.header = None, header
        return cart

    @classmethod
    def merge(cls, paths: list, name: str = "merged",
              dest: str | Path | None = None) -> dict:
        """Put the samples of several .rvr files on one cart, re-skimming none of them.

        This is how a sample is ADDED to a study: skim the new one on its own, then merge.
        The columns are column-major, so the buffer is repacked either way — but repacking
        is a memcpy over arrays that are already read, while re-skimming is hours.

        It REFUSES rather than concatenating blind. Two files whose columns or whose
        selection disagree do not describe the same measurement, and a merged file that
        pretended otherwise would be a lie no reader could detect. That check is only
        possible because a cart stamps what it did."""
        assert len(paths) >= 2, "merge needs at least two files"
        loaded = [(str(path), *page.read_by_sample(path)) for path in paths]
        first_path, first, _ = loaded[0]

        for path, header, _ in loaded[1:]:
            assert column_shape(header) == column_shape(first), (
                f"{path} and {first_path} do not hold the same columns:\n"
                f"  {column_shape(first)}\n  {column_shape(header)}\n"
                f"These are different measurements; merging them would make a file whose "
                f"columns mean different things for different samples.")
            # names agreeing is not enough: two files can both call a column PT_lead and
            # have built it from different branches, and the stamp is what can tell
            for what in ("keep", "prepare", "columns"):
                said = [(h.get("cart") or {}).get(what) for h in (first, header)]
                assert said[0] == said[1], (
                    f"{path} disagrees with {first_path} about {what}:\n"
                    f"  {said[0]!r}\n  {said[1]!r}\n"
                    f"Merging them would put differently-made numbers in one column.")

        # one weights column across the merged file: whoever declares one must agree, and
        # a file declaring none contributes ones — which is exactly what its samples are
        declared = {h["weights"] for _, h, _ in loaded if h.get("weights")}
        assert len(declared) <= 1, f"the files disagree on the weights column: {sorted(declared)}"
        weights = declared.pop() if declared else None

        columns_by_sample, stamp_samples, merged_from = {}, {}, []
        for path, header, samples in loaded:
            source_stamp = header.get("cart") or {}
            for sample, columns in samples.items():
                assert sample not in columns_by_sample, (
                    f"both {path} and an earlier file hold a sample called {sample!r}. "
                    f"Rename one — a merged file cannot have two samples with one name.")
                if weights and weights not in columns:
                    columns = {weights: np.ones(len(next(iter(columns.values()))), np.float64),
                               **columns}
                columns_by_sample[sample] = columns
                # a source with no stamp still DECLARES whether it carries weights; an
                # empty entry would read as "no weights", i.e. data, and the page would
                # stop scaling genuinely weighted MC by luminosity
                stamp_samples[sample] = (source_stamp.get("samples") or {}).get(sample) or {
                    "n": int(len(next(iter(columns.values())))),
                    "weights": header.get("weights") or None,
                    "files": {}, "metadata": {}}
            merged_from.append({"file": Path(path).name,
                                "name": source_stamp.get("name"),
                                "built": source_stamp.get("built"),
                                "config": (source_stamp.get("config") or {}).get("sha256")})

        labels = recover_labels(first)
        first_stamp = first.get("cart") or {}
        stamp = {"name": name,
                 "built": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                 "samples": stamp_samples,
                 "columns": first_stamp.get("columns") or {},
                 "keep": first_stamp.get("keep", ""),
                 "prepare": first_stamp.get("prepare"),
                 "config": first_stamp.get("config"),
                 "merged": merged_from}
        # the first file's view carries over — its cuts index columns, and the columns are
        # the same list in the same order or this would have refused above
        kept = ("colour", "style", "grouping", "lockcuts", "lockplots", "lockshow")
        return page.data(columns_by_sample, name, labels=labels, dest=dest, weights=weights,
                         tabs=first.get("tabs"),
                         extra={"tab": first.get("tab", 0),
                                **{k: first[k] for k in kept if first.get(k)}},
                         cart=stamp)

USAGE = """usage:
    python cart.py <study>                    the cartridge label of a study
    python cart.py <file>.rvr                 what a BUILT file holds
    python cart.py <study | file>.rvr --json  its whole fileset, as JSON
    python cart.py merge a.rvr b.rvr [...]    one file holding all their samples
"""

if __name__ == "__main__":
    import sys
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    if not args:
        sys.exit(USAGE + f"\nstudies: {', '.join(study_names()) or 'none'}")
    if args[0] == "merge":
        out = Cart.merge(args[1:], name="merged", dest=Path(args[1]).parent)
        print(f"{out['file']}  —  {out['events']:,} events, {out['mb']:.1f} MB")
    else:
        opened = Cart.open(args[0]) if args[0].endswith(".rvr") else Cart.load(args[0])
        print(opened.fileset_json() if "--json" in sys.argv else opened.summary())
