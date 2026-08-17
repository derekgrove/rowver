"""Read and inspect a fileset — a study's list of files, in coffea's shape.

    {sample name: {"files": {path: tree name, ...}, "metadata": {...}}}

A path may be a glob, so a NanoAOD sample split over 32 files is one entry, and each
path carries its own tree name — which is how one file holding 231 SMS mass-point trees
can supply several samples. A path may also be a remote URL — root:// through an XRootD
redirector, or https:// — which is read in place, never globbed. study.py owns finding
the file; this owns understanding it.
"""
from pathlib import Path
import glob
import os

import uproot


def remote(path: str) -> bool:
    """A path with a scheme — root://, https:// — lives on the network, not this disk."""
    return "://" in path


def expand(fileset: dict[str, dict]) -> dict[str, dict]:
    """Turn every glob in a freshly read fileset into the real paths it matches."""
    for sample in fileset.values():
        expanded = {}
        for pattern, tree in sample["files"].items():
            local_glob = not remote(pattern) and any(c in pattern for c in "*?[")
            hits = sorted(glob.glob(pattern)) if local_glob else [pattern]
            expanded |= {h: tree for h in hits}
        sample["files"] = expanded
    return fileset


def files(fileset: dict[str, dict], names: list[str] | None = None) -> dict[str, dict[str, str]]:
    """{sample: {path: tree}} for pikl, for the named samples or all of them."""
    return {n: fileset[n]["files"] for n in (names or fileset)}



def check(fileset: dict[str, dict], where: str | Path | None = None) -> int:
    """Confirm every sample matches at least one file, and every file holds its tree."""
    bad = []
    for name, paths in files(fileset).items():
        if not paths:
            bad.append(f"{name}: matched no files")
        for path, tree in paths.items():
            if not remote(path) and not os.path.exists(path):
                bad.append(f"{name}: no such file {path}")
                continue
            # a remote path has no exists() — opening it IS the check, and the error
            # names what a remote read needs (the xrootd package, a grid proxy)
            try:
                held = uproot.open(path)
            except Exception as e:
                bad.append(f"{name}: could not open {path} — {e}")
                continue
            if tree not in held:
                bad.append(f"{name}: {os.path.basename(path)} has no tree {tree!r}")
    # a fresh clone points at whoever built it, so say which file to edit rather than
    # only that nothing matched
    assert not bad, ("bad fileset:\n  " + "\n  ".join(bad)
                     + (f"\n\nPaths are absolute and machine-specific. Edit {where} to point "
                        f"at your own files; see 'Adding a study' in README.md."
                        if where else ""))
    return sum(len(p) for p in files(fileset).values())


def meta(fileset: dict[str, dict], names: list[str] | None = None) -> dict[str, dict]:
    """{sample: its whole metadata dict} — what pikl hands to KEEP and PREPARE."""
    return {n: fileset[n].get("metadata", {}) for n in (names or fileset)}


def metadata(fileset: dict[str, dict], key: str) -> dict[str, object]:
    """{sample: metadata[key]} for one key, None where a sample does not set it."""
    return {n: s.get("metadata", {}).get(key) for n, s in fileset.items()}


def summary(fileset: dict[str, dict]) -> str:
    """One line per sample: how many files, which tree, and whatever metadata it carries."""
    out = []
    for name, sample in fileset.items():
        trees = sorted(set(sample["files"].values()))
        meta = "  ".join(f"{k}={v}" for k, v in sample.get("metadata", {}).items())
        out.append(f"  {name:24s} {len(sample['files']):>3} files  "
                   f"{','.join(trees):14s} {meta}")
    return "\n".join(out)


def discover(directory: str, tree: str = "Events") -> dict[str, dict]:
    """Build a fileset from every subdirectory of `directory` that holds .root files."""
    out = {}
    for sub in sorted(Path(directory).iterdir()):
        if sub.is_dir() and list(sub.glob("*.root")):
            out[sub.name] = {"files": {f"{sub}/*.root": tree}, "metadata": {}}
    return out
