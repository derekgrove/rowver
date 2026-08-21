"""A cart: one study's fileset and config in one object, and the .rvr it burns to.

A study is ONE file, studies/<name>/cart.py, holding a Cart named CART. There is no
fileset.json riding alongside — the fileset is an ordinary python dict in coffea's shape,

    {sample name: {"files": {path: tree name, ...}, "metadata": {...}}}

and `CART.json()` dumps it whenever you want to read it as JSON.

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

import uproot

import page
import pikl

STUDIES: Path = Path(__file__).with_name("studies")

#: keys the skim adds to every sample's metadata; a fileset may not also use them
RESERVED = ("sample", "file", "tree")


def remote(path: str) -> bool:
    """A path with a scheme — root://, https:// — lives on the network, not this disk."""
    return "://" in path


def expand(fileset: dict) -> dict:
    """Turn every local glob into the real paths it matches, leaving remote urls alone."""
    out = {}
    for name, sample in fileset.items():
        paths = {}
        for pattern, tree in sample["files"].items():
            local_glob = not remote(pattern) and any(c in pattern for c in "*?[")
            paths |= {hit: tree for hit in (sorted(glob.glob(pattern)) if local_glob
                                            else [pattern])}
        out[name] = {"files": paths, "metadata": dict(sample.get("metadata") or {})}
    return out


def describe(spec) -> str:
    """How something was made, in words: a branch name, a lambda's source, or a function."""
    if isinstance(spec, str):
        return spec
    if getattr(spec, "__name__", "<lambda>") != "<lambda>":
        first = (spec.__doc__ or "").strip().splitlines()
        return f"{spec.__name__}()" + (f" — {first[0]}" if first else "")
    try:
        return " ".join(inspect.getsource(spec).split()).rstrip(",")
    except (OSError, TypeError):
        return "<lambda>"


def available() -> list[str]:
    """Names of every study in studies/ that has a cart.py."""
    return sorted(p.name for p in STUDIES.iterdir() if (p / "cart.py").exists())


def module(path: Path, name: str):
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
                 weights: str = "weight", title: str = ""):
        """Hold a study, with its globs expanded and everything about it checked."""
        self.name = name
        self.fileset = expand(fileset)
        self.columns = columns
        self.keep = keep
        self.prepare = prepare
        self.labels = labels
        self.title = title
        # the column exists only if some sample declares weights; nothing else to set
        self.weights = weights if any(map(self.weights_spec, self.fileset)) else None
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
        for column, spec in self.columns.items():
            assert isinstance(spec, str) or callable(spec), (
                f"{self.name}: columns[{column!r}] is neither a branch name nor a function")
        assert self.weights not in self.columns, (
            f"{self.name}: columns defines {self.weights!r}, which is the weights column. "
            f"Weights are per sample now — put the branch name in each sample's "
            f"metadata as {{'weights': {self.weights!r}}} and drop it from columns. A "
            f"sample that declares none is data: its rows are 1.0 and never scaled.")

    def weights_spec(self, sample: str):
        """What fills this sample's weights column — a branch, a function, or None."""
        return self.fileset[sample]["metadata"].get("weights")

    def weighted(self, sample: str) -> bool:
        """Whether this sample carries real weights, or is data counted one event at a time."""
        return self.weights_spec(sample) is not None

    def files(self) -> dict[str, dict[str, str]]:
        """{sample: {path: tree}} — the fileset with the metadata stripped off."""
        return {name: entry["files"] for name, entry in self.fileset.items()}

    def meta(self, sample: str, path: str = "", tree: str = "") -> dict:
        """What a hook is handed: this sample's metadata, plus the file being read."""
        return {**self.fileset[sample]["metadata"],
                "sample": sample, "file": path, "tree": tree}

    def check(self) -> int:
        """Confirm every sample matches a file and every file holds its tree; count them."""
        bad = []
        for name, paths in self.files().items():
            if not paths:
                bad.append(f"{name}: matched no files")
            for path, tree in paths.items():
                if not remote(path) and not os.path.exists(path):
                    bad.append(f"{name}: no such file {path}")
                    continue
                try:
                    held = uproot.open(path)
                except Exception as e:
                    bad.append(f"{name}: could not open {path} — {e}")
                    continue
                if tree not in held:
                    bad.append(f"{name}: {os.path.basename(path)} has no tree {tree!r}")
        assert not bad, ("bad fileset:\n  " + "\n  ".join(bad)
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

    def unweighted(self) -> list[str]:
        """Every sample carrying no weights — the ones the page counts one event at a time."""
        return [s for s in self.fileset if not self.weighted(s)]

    def warn_mixed(self) -> str:
        """The line to print when a cart holds both weighted and unweighted samples."""
        data, mc = self.unweighted(), [s for s in self.fileset if self.weighted(s)]
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
        return pikl.pikl(self, self.name, step, dest or self.dir, jobs, threads)

    def stamp(self, counts: dict[str, int] | None = None) -> dict:
        """What goes in META['cart']: everything this file can still say about itself."""
        counts = counts or {}
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
        return {"name": self.name,
                "built": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "samples": samples,
                "columns": {c: describe(s) for c, s in self.columns.items()},
                "keep": describe(self.keep),
                "prepare": describe(self.prepare) if self.prepare else None,
                "config": self.config_stamp()}

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

    def json(self) -> str:
        """The fileset as JSON — for reading, for diffing, for handing to something else."""
        return json.dumps(self.fileset, indent=2, default=describe)

    def write(self, rows: dict, dest: str | Path | None = None, name: str = "",
              tabs: list | None = None, extra: dict | None = None) -> dict:
        """Burn the cart: the skimmed columns and everything known about them, as one .rvr."""
        counts = {s: len(next(iter(c.values()))) for s, c in rows.items()}
        return page.data(rows, name or self.name, labels=self.labels,
                         dest=dest or self.dir, weights=self.weights,
                         tabs=tabs, extra=extra, cart=self.stamp(counts))

    @classmethod
    def load(cls, name: str) -> "Cart":
        """Read studies/<name>/cart.py and hand back the CART it defines."""
        path = STUDIES / name / "cart.py"
        assert path.exists(), f"no cart at {path} — have: {available()}"
        mod = module(path, f"studies.{name}.cart")
        assert hasattr(mod, "CART"), f"{path} defines no CART"
        return mod.CART

    @classmethod
    def open(cls, path: str | Path) -> "Cart":
        """Read a built .rvr back: what is on it, where it came from, and its columns."""
        header, data = page.read(path)
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
        cart.labels, cart.title = None, header.get("title", "")
        cart.weights = header.get("weights") or None
        cart.source = (stamp.get("config") or {}).get("path")
        cart.dir, cart.built = None, stamp.get("built")
        cart.header, cart.data = header, data
        return cart

    @staticmethod
    def discover(directory: str, tree: str = "Events") -> dict:
        """A fileset from every subdirectory of `directory` that holds .root files."""
        return {sub.name: {"files": {f"{sub}/*.root": tree}, "metadata": {}}
                for sub in sorted(Path(directory).iterdir())
                if sub.is_dir() and list(sub.glob("*.root"))}


if __name__ == "__main__":
    import sys
    what = sys.argv[1] if len(sys.argv) > 1 else ""
    if not what:
        sys.exit(f"usage: python cart.py <study | file.rvr> [--json]\n"
                 f"studies: {', '.join(available()) or 'none'}")
    held = Cart.open(what) if what.endswith(".rvr") else Cart.load(what)
    print(held.json() if "--json" in sys.argv else held.summary())
