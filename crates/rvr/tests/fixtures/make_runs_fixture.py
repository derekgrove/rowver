"""Write runs.root: 1,200 entries of tree Events in 24 baskets of 50, jets of 0 to 11 members (up
to 55 pairs an event), and each event's run and number (unique), for the tests that press the same
file in runs of every size and compare files event by event.

    mamba run -n rowver python tests/fixtures/make_runs_fixture.py OUT
"""
import pathlib
import sys

import awkward as ak
import numpy as np
import uproot

r = np.random.default_rng(15)
numbers = r.choice(10**12, 24 * 50, replace=False).astype("i8")
# uproot's recreate writes over a file without making it shorter: start from none.
pathlib.Path(sys.argv[1]).unlink(missing_ok=True)
with uproot.recreate(sys.argv[1], compression=uproot.ZLIB(9)) as out:
    for basket in range(24):
        n = r.integers(0, 12, 50)
        total = int(n.sum())
        jets = ak.zip({
            "pt": ak.unflatten(r.integers(15, 200, total).astype("f4"), n),
            "eta": ak.unflatten((r.integers(-20, 21, total) / 8).astype("f4"), n),
            "phi": ak.unflatten((r.integers(-25, 26, total) / 8).astype("f4"), n),
        })
        events = {"Jet": jets, "MET": (r.integers(0, 800, 50) / 4).astype("f4"), "weight": r.integers(1, 9, 50) / 8, "flag": r.random(50) > 0.5,
                  "run": r.integers(1, 4, 50).astype("i4"), "event": numbers[basket * 50:(basket + 1) * 50]}
        if basket == 0:
            types = {"Jet": jets.type.content, "MET": "float32", "weight": "float64", "flag": "bool", "run": "int32", "event": "int64"}
            out.mktree("Events", types)
        out["Events"].extend(events)
