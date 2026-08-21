#!/usr/bin/env python
"""Run a study: skim its files, then write the data file the viewer opens.

Everything a study produces lands in studies/<name>/, beside the cart.py that defined it:
<name>.pkl (the skim), <name>.rvr (columns + view + the cartridge stamp), <name>.html.

The .rvr is what you want almost always — drop it on viewer.html. The page is the same
data with the whole viewer wrapped around it, twice the size, so it is asked for.

    python run_rowver.py template                 skim, then write <name>.rvr
    python run_rowver.py template --html          also build the standalone <name>.html
    python run_rowver.py template --no-skim       reuse the pikl, rewrite the .rvr
    python run_rowver.py --list                   what studies exist
"""
import argparse
import os
import sys
import time

import cart as cart_module
import load
import page
import pikl


def parse(argv: list[str] | None = None) -> argparse.Namespace:
    """The command line."""
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("study", nargs="?", help="a directory under studies/")
    p.add_argument("--list", action="store_true", help="list studies and exit")
    p.add_argument("--no-skim", action="store_true", help="reuse the existing pikl")
    p.add_argument("--html", action="store_true",
                   help="also build the standalone html page (about twice the .rvr's size)")
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


def view_of(cart):
    """The study's view.py, or None when it has none."""
    path = cart.dir / "view.py" if cart.dir else None
    return cart_module.module(path, f"studies.{cart.name}.view") if path and path.exists() else None


def only(spec, names: list[str]):
    """A GROUPS or SUBGROUPS map with the samples a --samples subset dropped taken out."""
    if not spec:
        return spec
    kept = {g: [w for w in v if w in names or any(c in w for c in "*?[")]
            for g, v in spec.items()}
    return {g: v for g, v in kept.items() if v}


def tabs_for(meta: dict, mod, names: list[str]) -> tuple[list | None, dict | None]:
    """Turn a study's view.py into the page's tabs, or (None, None) when it has none."""
    if mod is None:
        return None, None
    subs = only(getattr(mod, "SUBGROUPS", None), names)
    tabs = page.tabs(meta, mod.VIEW, only(getattr(mod, "GROUPS", None), list(subs or {}) + names),
                     subs)
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
        print("studies:", ", ".join(cart_module.available()) or "none")
        return 0 if args.list else 2

    cores = os.cpu_count() or 1
    assert args.jobs <= cores, (
        f"--jobs {args.jobs} on {cores} cores. A worker holds a whole chunk — about a GB on "
        f"these skims — so the ceiling is memory before it is cores; --jobs 10 was killed by "
        f"the OS on a 26 GB machine.")

    cart = cart_module.Cart.load(args.study)
    name = args.out or cart.name
    print(cart.summary())
    print(cart.mixed_warning(), end="")

    view = view_of(cart)
    check_view(view, list(cart.fileset), f"{cart.dir}/view.py")
    print(f"\n{cart.check()} files across {len(cart.fileset)} samples")

    if args.check:
        per_sample = cart.files()
        smallest = min(per_sample, key=lambda n: len(per_sample[n]))
        t = time.time()
        pikl.verify_chunking(cart, smallest)
        print(f"check: chunked == unchunked on {smallest}  [{time.time()-t:.1f}s]")

    if not args.no_skim:
        t = time.time()
        rows = cart.skim(args.step, jobs=args.jobs, threads=args.threads)
        print(f"\nskim -> {cart.dir}/{cart.name}.pkl  [{time.time()-t:.0f}s]")
        # any column gives the row count — a weightless study has no "weight" to ask
        nrows = lambda v: len(next(iter(v.values())))
        for s, v in rows.items():
            print(f"  {s:26s} {nrows(v):>9,}")
        print(f"  {'TOTAL':26s} {sum(nrows(v) for v in rows.values()):>9,}")

    if args.viewer:
        print("\nviewer:", page.viewer())

    pikld = load.read(cart.name, cart.dir)
    rows, skim = pikld["results"], {k: pikld.get(k) for k in ("recipe", "skimmed")}
    if args.samples:
        want = [s.strip() for s in args.samples.split(",")]
        missing = [s for s in want if s not in rows]
        assert not missing, f"not in the pikl: {missing} — have {list(rows)}"
        rows = {s: rows[s] for s in want}

    meta, _ = page.pack(rows, cart.labels,
                        cart.weights_column if any(map(cart.weighted, rows)) else None)
    tabs, extra = tabs_for(meta, view, list(rows))
    if tabs:
        print(f"view.py: {len(tabs)} tabs — {', '.join(t['name'] for t in tabs)}")

    print("\ndata:", cart.write(rows, name=name, tabs=tabs, extra=extra, skim=skim))
    # the page used to be rebuilt every run, so it could not fall behind the data. It can
    # now: an old .html beside a fresh .rvr opens on the numbers from the previous skim.
    stale = cart.dir / f"{name}.html"
    if not args.html and stale.exists():
        print(f"  note: {stale.name} was NOT rebuilt — it still holds the data from the run "
              f"that made it. Add --html to rebuild it, or delete it")
    if args.html:
        counts = {s: len(next(iter(v.values()))) for s, v in rows.items()}
        print("page:", page.export(rows, name, title=f"RowVer — {cart.name}",
                                   labels=cart.labels, dest=cart.dir,
                                   weights=cart.weights_column if any(map(cart.weighted, rows))
                                           else None, tabs=tabs, extra=extra,
                                   cart=cart.stamp(counts, skim)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
