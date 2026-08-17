#!/usr/bin/env python
"""Run a study: skim its files, then build its page.

Everything a study produces lands in studies/<name>/, beside the fileset and config that
defined it: <name>.pkl (the skim), <name>.rvr (columns + view), <name>.html (the page).

    python run_rowver.py template                 skim and build the page
    python run_rowver.py template --no-page       skim only, leave the pikl
    python run_rowver.py template --no-skim       reuse the pikl, rebuild the page
    python run_rowver.py template --data          also write <name>.rvr
    python run_rowver.py --list                  what studies exist
"""
import argparse
import os
import sys
import time

import load
import page
import pikl
import samples
import study


def parse(argv: list[str] | None = None) -> argparse.Namespace:
    """The command line."""
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("study", nargs="?", help="a directory under studies/")
    p.add_argument("--list", action="store_true", help="list studies and exit")
    p.add_argument("--no-skim", action="store_true", help="reuse the existing pikl")
    p.add_argument("--no-page", action="store_true", help="skim only, build no html")
    p.add_argument("--data", action="store_true", help="also write a .rvr data file")
    p.add_argument("--viewer", action="store_true", help="also write the empty drop-target viewer")
    p.add_argument("--check", action="store_true", help="verify chunked == unchunked first")
    p.add_argument("--samples", help="comma-separated subset to put in the page")
    p.add_argument("--out", help="output name (default: the study's)")
    p.add_argument("--step", type=int, default=pikl.STEP, help="events per chunk")
    p.add_argument("--jobs", type=int, default=1,
                   help="samples to skim at once; each worker costs about a GB")
    p.add_argument("--threads", type=int, default=0,
                   help="uproot decompression threads within each sample")
    return p.parse_args(argv)


def tabs_for(meta: dict, mod) -> tuple[list | None, dict | None]:
    """Turn a study's view.py into the page's tabs, or (None, None) when it has none."""
    if mod is None:
        return None, None
    tabs = page.tabs(meta, mod.VIEW, getattr(mod, "GROUPS", None),
                     getattr(mod, "SUBGROUPS", None))
    extra = {"tab": getattr(mod, "OPENS_ON", 0), "colour": getattr(mod, "COLOUR", None)}
    extra.update(getattr(mod, "LOCKS", None) or {})
    return tabs, extra


def check_view(defaults, names: list[str], where) -> None:
    """Names in view.py that no sample answers to — before the skim, not after it."""
    if defaults is None:
        return
    subs = set(getattr(defaults, "SUBGROUPS", {}) or {})
    known = set(names) | subs
    bad = {}
    for key in ("GROUPS", "SHOWN"):
        spec = getattr(defaults, key, None)
        if spec is None:
            continue
        wanted = [w for v in spec.values() for w in v] if isinstance(spec, dict) else list(spec)
        missing = [w for w in wanted
                   if not any(c in w for c in "*?[") and w not in known]
        if missing:
            bad[key] = missing
    assert not bad, (
        f"{where} names samples this fileset does not define:\n  "
        + "\n  ".join(f"{k} -> {v}" for k, v in bad.items())
        + f"\n\nThe fileset defines: {names}\n"
          f"Rename one side or the other. Checked before the skim, which is the slow part.")


def main(argv: list[str] | None = None) -> int:
    """Skim, then build, reporting what each step cost."""
    args = parse(argv)
    if args.list or not args.study:
        print("studies:", ", ".join(study.available()) or "none")
        return 0 if args.list else 2

    cores = os.cpu_count() or 1
    assert args.jobs <= cores, (
        f"--jobs {args.jobs} on {cores} cores. A worker holds a whole chunk — about a GB on "
        f"these skims — so the ceiling is memory before it is cores; --jobs 10 was killed by "
        f"the OS on a 26 GB machine.")

    st = study.load(args.study)
    name = args.out or st["name"]
    print(study.summary(st))

    files = samples.files(st["fileset"])
    check_view(st["view"], list(files), st["dir"] / "view.py")
    print(f"\n{samples.check(st['fileset'], st['dir'] / 'fileset.json')} files "
          f"across {len(files)} samples")

    if args.check:
        one = min(files, key=lambda n: len(files[n]))
        t = time.time()
        pikl.check(st["config"], files[one], samples.meta(st["fileset"])[one])
        print(f"check: chunked == unchunked on {one}  [{time.time()-t:.1f}s]")

    if not args.no_skim:
        t = time.time()
        rows = pikl.pikl(name, files, st["config"], args.step,
                         samples.meta(st["fileset"]), dest=st["dir"], jobs=args.jobs, threads=args.threads)
        print(f"\nskim -> {st['dir']}/{name}.pkl  [{time.time()-t:.0f}s]")
        # any column gives the row count — a weightless study has no "weight" to ask
        nrows = lambda v: len(next(iter(v.values())))
        for s, v in rows.items():
            print(f"  {s:26s} {nrows(v):>9,}")
        print(f"  {'TOTAL':26s} {sum(nrows(v) for v in rows.values()):>9,}")

    if args.no_page and not (args.data or args.viewer):
        return 0

    if args.viewer:
        print("\nviewer:", page.viewer())

    rows = load.load(name, st["dir"])
    if args.samples:
        want = [s.strip() for s in args.samples.split(",")]
        missing = [s for s in want if s not in rows]
        assert not missing, f"not in the pikl: {missing} — have {list(rows)}"
        rows = {s: rows[s] for s in want}

    labels = getattr(st["config"], "LABELS", None)
    wcol = study.weights(st["config"], st["dir"] / "config.py")
    meta, _ = page.pack(rows, labels, wcol)
    tabs, extra = tabs_for(meta, st["view"])
    if tabs:
        print(f"view.py: {len(tabs)} tabs — {', '.join(t['name'] for t in tabs)}")

    if not args.no_page:
        print("\npage:", page.export(rows, name, title=f"RowVer — {st['name']}",
                                     labels=labels, dest=st["dir"],
                                     weights=wcol, tabs=tabs, extra=extra))
    if args.data:
        print("data:", page.data(rows, name, labels=labels, dest=st["dir"],
                                 weights=wcol, tabs=tabs, extra=extra))
    return 0


if __name__ == "__main__":
    sys.exit(main())
