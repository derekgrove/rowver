#!/usr/bin/env python
"""Run a study: skim its files, then write the .rvr.

Everything a study produces lands in studies/<name>/, beside the cart.py that defined it:
<name>.pkl (the skim) and <name>.rvr (the columns and where they came from, at rowver 6).

The file opens with NOTHING set up — no tabs, no plots. Configuring a page is the front
end's job (see the rowver_dmg repo); this repo builds the data and stamps its provenance.

    python run_rowver.py template                 skim, then write <name>.rvr
    python run_rowver.py template --no-skim       reuse the pikl, rewrite the .rvr
    python run_rowver.py --list                   what studies exist
"""
import argparse
import os
import sys
import time

import cart as cart_module
import load
import pikl


def parse(argv: list[str] | None = None) -> argparse.Namespace:
    """The command line."""
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("study", nargs="?", help="a directory under studies/")
    p.add_argument("--list", action="store_true", help="list studies and exit")
    p.add_argument("--no-skim", action="store_true", help="reuse the existing pikl")
    p.add_argument("--check", action="store_true", help="verify chunked == unchunked first")
    p.add_argument("--samples", help="comma-separated subset to put in the page")
    p.add_argument("--out", help="output name (default: the study's)")
    p.add_argument("--step", type=int, default=pikl.STEP, help="events per chunk")
    p.add_argument("--jobs", type=int, default=1,
                   help="samples to skim at once; each worker costs about a GB")
    p.add_argument("--threads", type=int, default=0,
                   help="uproot decompression threads within each sample")
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    """Skim, then build, reporting what each step cost."""
    args = parse(argv)
    if args.list or not args.study:
        print("studies:", ", ".join(cart_module.study_names()) or "none")
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

    stale_view = cart.dir / "view.py" if cart.dir else None
    if stale_view and stale_view.exists():
        print(f"\n  note: {stale_view.name} is NOT read any more — a .rvr opens with nothing set "
              f"up and the page saves its own configuration. Delete it, or keep it as a record.")
    print(f"\n{cart.count_files()} files across {len(cart.fileset)} samples")

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

    pikld = load.read(cart.name, cart.dir)
    rows, skim = pikld["results"], {k: pikld.get(k) for k in ("recipe", "skimmed")}
    if args.samples:
        want = [s.strip() for s in args.samples.split(",")]
        missing = [s for s in want if s not in rows]
        assert not missing, f"not in the pikl: {missing} — have {list(rows)}"
        rows = {s: rows[s] for s in want}

    print("\ndata:", cart.write(rows, name=name, skim=skim))
    return 0


if __name__ == "__main__":
    sys.exit(main())
