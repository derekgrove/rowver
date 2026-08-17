"""Load a study — one directory holding everything one plot needs.

    studies/<name>/
        fileset.json    which files, which trees, per-sample metadata
        config.py       KEEP and COLUMNS, and optionally PREPARE
        view.py         how the page opens: VIEW, GROUPS, FOM, COLOUR      (optional)

Two studies that share physics should import it rather than copy it — a config is an
ordinary module, so `from studies.template.config import KEEP` works.
"""
import importlib.util
import json
from pathlib import Path
from types import ModuleType

import samples

STUDIES: Path = Path(__file__).with_name("studies")


def available() -> list[str]:
    """Names of every study in studies/ that has a fileset and a config."""
    return sorted(p.name for p in STUDIES.iterdir()
                  if (p / "fileset.json").exists() and (p / "config.py").exists())


def module(path: Path, name: str) -> ModuleType:
    """Import a .py file that is not on the import path."""
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(mod)
    except ModuleNotFoundError as e:
        raise ModuleNotFoundError(
            f"{path} imports {e.name!r}, which is not installed.\n"
            f"  A config is ordinary python and may import whatever your selection "
            f"needs, so a study can require packages this tool does not. "
            f"studies/template/ needs nothing beyond uproot and awkward — copy that "
            f"one as a starting point. See 'Adding a study' in README.md.") from e
    return mod


def load(name: str) -> dict:
    """Read studies/<name>/ into {name, dir, fileset, config, view}."""
    here = STUDIES / name
    assert here.is_dir(), f"no study {name!r} in {STUDIES} — have: {available()}"
    for needed in ("fileset.json", "config.py"):
        assert (here / needed).exists(), f"{name}: no {needed}"

    fileset = samples.expand(json.loads((here / "fileset.json").read_text()))
    config = module(here / "config.py", f"studies.{name}.config")
    for needed in ("KEEP", "COLUMNS"):
        assert hasattr(config, needed), f"{name}/config.py defines no {needed}"
    assert config.COLUMNS, f"{name}/config.py: COLUMNS is empty"
    for column, spec in config.COLUMNS.items():
        assert isinstance(spec, str) or callable(spec), \
            f"{name}/config.py: COLUMNS[{column!r}] is neither a branch name nor a lambda"
    weights(config, f"{name}/config.py")

    view = (module(here / "view.py", f"studies.{name}.view")
            if (here / "view.py").exists() else None)
    assert view is None or hasattr(view, "VIEW"), f"{name}/view.py defines no VIEW"
    return {"name": name, "dir": here, "fileset": fileset,
            "config": config, "view": view}


def weights(config: ModuleType, where: str) -> str | None:
    """
    Which COLUMNS entry holds the per-event weights, or None when there are none.

        HAS_WEIGHTS    = True         # False -> real data; nothing scales an event
        WEIGHTS_BRANCH = "genWeight"  # only when HAS_WEIGHTS; defaults to "weight"

    Both defaults are the old behaviour, so a config that says nothing keeps working.
    The two ways of saying it must agree: a config that declares no weights while still
    computing a weights column raises here rather than silently shipping one of them.
    """
    has = bool(getattr(config, "HAS_WEIGHTS", True))
    name = getattr(config, "WEIGHTS_BRANCH", "weight")
    if not has:
        assert name not in config.COLUMNS, (
            f"{where}: HAS_WEIGHTS is False, but COLUMNS still defines {name!r}. Drop that "
            f"column, or set HAS_WEIGHTS = True — a file cannot both have and not have "
            f"weights, and guessing which you meant is how a yield goes quietly wrong.")
        return None
    assert name in config.COLUMNS, (
        f"{where}: WEIGHTS_BRANCH is {name!r}, which COLUMNS does not define — it has "
        f"{sorted(config.COLUMNS)}. Name one of those, or set HAS_WEIGHTS = False.")
    return name


def summary(study: dict) -> str:
    """The study on a few lines: samples, files, columns, and how the page opens."""
    cfg, dflt = study["config"], study["view"]
    lines = [f"study {study['name']}  ({study['dir']})",
             samples.summary(study["fileset"]),
             f"  config: {len(cfg.COLUMNS)} columns, KEEP: {(cfg.KEEP.__doc__ or '').strip()}"]
    if dflt:
        lines.append(f"  view: {len(getattr(dflt, 'VIEW', []))} tabs "
                     f"({', '.join(x['name'] for x in getattr(dflt, 'VIEW', []))}), "
                     f"groups={sorted(getattr(dflt, 'GROUPS', {}) or {})}")
    return "\n".join(lines)
