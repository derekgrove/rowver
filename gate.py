"""The battery. Everything this repo can get wrong that would be SILENT.

    python gate.py                 every lane
    python gate.py build merge     just those

Each lane returns a one-line report or raises. The bar for a lane is the one this repo
learned the hard way: a check that cannot go red is not a check, so every lane here is
one that has actually caught something, or is pinned against a number derived from
outside itself.

`smoke.py` used to live here and was mostly the page — booting a template in a JS engine,
dropping files on it, comparing readouts. The page moved to the rowver_dmg repo and took
those lanes with it; what follows is the half that was ever about the data.
"""
from __future__ import annotations

import sys
import tempfile
from pathlib import Path

import numpy as np

import load
import rvr6
from cart import Cart

HERE = Path(__file__).resolve().parent

#: studies with a pikl on disk that need no network — the lanes run on these
LOCAL = ("dimuon_demo", "mixed_demo", "grid_example")


def lane_format() -> str:
    """The format's own self-check: the frame, the six refusals, the lock, the lumi ladder."""
    rvr6.demo()
    return "frame, refusals, promote-and-shadow and the outermost ladder all pinned"


def lane_build() -> str:
    """A pikl through the packer and back, and the counts must survive it exactly."""
    out = []
    for study in LOCAL:
        cart = Cart.load(study)
        rows = load.load(study, cart.dir)
        expect = {s: len(next(iter(c.values()))) for s, c in rows.items()}
        with tempfile.TemporaryDirectory() as d:
            cart.write(rows, dest=Path(d), name=study)
            meta, back = rvr6.read_by_sample(Path(d) / f"{study}.rvr")
        got = {s["name"]: s["rows"] for s in meta["data"]["samples"]}
        assert got == expect, f"{study}: rows changed through the packer, {got} != {expect}"
        for sample, cols in back.items():
            for name, values in cols.items():
                assert np.array_equal(values, rows[sample][name]), \
                    f"{study}: column {name} of {sample} did not survive the round trip"
        out.append(f"{study} {sum(expect.values()):,}")
    return "columns element-identical through write+read: " + ", ".join(out)


def lane_merge() -> str:
    """Split a study into one file per sample and merge it back: it must equal the one-shot."""
    cart = Cart.load("mixed_demo")
    rows = load.load("mixed_demo", cart.dir)
    with tempfile.TemporaryDirectory() as d:
        parts = [cart.write({n: c}, dest=Path(d), name=f"part{i}")["file"]
                 for i, (n, c) in enumerate(rows.items())]
        cart.write(rows, dest=Path(d), name="oneshot")
        a, A = rvr6.read_by_sample(Path(d) / "oneshot.rvr")
        b, B = rvr6.read_by_sample(Cart.merge(parts, name="remerged", dest=Path(d))["file"])
    assert sorted(A) == sorted(B), "merge lost or renamed a sample"
    for s in A:
        for c in A[s]:
            assert np.array_equal(A[s][c], B[s][c]), f"merge changed {c} of {s}"
    # the one that actually bit: a data sample gains a column of ones on the way in, and
    # must NOT come back out reading as weighted MC
    was = {s["name"]: s["weighted"] for s in a["data"]["samples"]}
    now = {s["name"]: s["weighted"] for s in b["data"]["samples"]}
    assert was == now, f"merge changed what is weighted: {was} -> {now}"
    return f"merge(A,B) == one-shot on {len(A)} samples, and `weighted` survived it"


def lane_unweighted() -> str:
    """A sample that declares no weights is DATA: no luminosity ever multiplies it."""
    cart = Cart.load("mixed_demo")
    rows = load.load("mixed_demo", cart.dir)
    import yields
    with tempfile.TemporaryDirectory() as d:
        cart.write(rows, dest=Path(d), name="mixed")
        meta, buf = rvr6.read(Path(d) / "mixed.rvr")
        meta["view"]["groups"] = {"of": {"Data 2012 DoubleMuParked": "data",
                                         "MC stand-in x2": "MC"},
                                  "sub": {}, "order": ["data", "MC"]}
        seen = {}
        for lumi in (10, 400):
            meta["view"]["lumis"] = {"sample": {}, "sub": {},
                                     "group": {"data": lumi, "MC": lumi}}
            rvr6.write(Path(d) / "l.rvr", meta, buf)
            got = yields.dump(Path(d) / "l.rvr", {"all": []})["all"]
            seen[lumi] = (got["data"]["nominal"][1], got["MC"]["nominal"][1])
    assert seen[10][0] == seen[400][0], (
        f"observed data was scaled by luminosity: {seen[10][0]} at 10 became "
        f"{seen[400][0]} at 400 — this is the whole reason `weighted` is declared")
    assert abs(seen[400][1] / seen[10][1] - 40) < 1e-9, \
        f"MC did NOT scale with luminosity: {seen[10][1]} -> {seen[400][1]}"
    return (f"data reads {seen[10][0]:,.0f} at BOTH 10 and 400 while MC scales "
            f"{seen[10][1]:,.0f} -> {seen[400][1]:,.0f}")


def lane_keys() -> str:
    """A writer must carry what it does not understand — the measured v5 failure."""
    cart = Cart.load("dimuon_demo")
    rows = load.load("dimuon_demo", cart.dir)
    with tempfile.TemporaryDirectory() as d:
        cart.write(rows, dest=Path(d), name="probe")
        meta, buf = rvr6.read(Path(d) / "probe.rvr")
        sites = [meta, meta["about"], meta["view"], meta["made"]]
        for i, site in enumerate(sites):
            site["zz_probe"] = i
        rvr6.write(Path(d) / "probe.rvr", meta, buf)
        back, buf2 = rvr6.read(Path(d) / "probe.rvr")
        for i, site in enumerate([back, back["about"], back["view"], back["made"]]):
            assert site.get("zz_probe") == i, \
                "a writer rebuilt a drawer from a literal and dropped an unknown key"
        assert buf2 == buf, "the column buffer did not survive a rewrite"
    return f"an unknown key survives at {len(sites)} sites, and the buffer is untouched"


LANES = {"format": lane_format, "build": lane_build, "merge": lane_merge,
         "unweighted": lane_unweighted, "keys": lane_keys}


def main(argv: list[str]) -> int:
    want = argv or list(LANES)
    unknown = [a for a in want if a not in LANES]
    if unknown:
        sys.exit(f"no such lane {unknown} — have: {list(LANES)}")
    bad = 0
    for name in want:
        try:
            print(f"  {name:11s} ok — {LANES[name]()}")
        except Exception as e:                      # a lane reports, it does not traceback
            bad += 1
            print(f"  {name:11s} FAILED — {type(e).__name__}: {e}")
    print(f"\n{len(want) - bad}/{len(want)} lanes green")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
