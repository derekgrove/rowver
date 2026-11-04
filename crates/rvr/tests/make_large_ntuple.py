"""Write a NanoAOD-like ntuple of ROWS entries, tree Events, for the test that presses it at several
sizes (what_a_press_holds_does_not_grow_with_the_events_it_keeps), into a folder it deletes after:

    mamba run -n rowver python tests/make_large_ntuple.py OUT ROWS
"""
import pathlib
import sys

import awkward as ak
import numpy as np
import uproot

out, rows = sys.argv[1], int(sys.argv[2])
r = np.random.default_rng(7)
# uproot's recreate writes over a file without making it shorter: start from none.
pathlib.Path(out).unlink(missing_ok=True)
with uproot.recreate(out, compression=uproot.ZLIB(1)) as f:
    for first in range(0, rows, 100_000):
        n = min(100_000, rows - first)
        jets = r.poisson(3, n)
        total = int(jets.sum())
        jet = ak.zip({
            "pt": ak.unflatten((r.integers(120, 1600, total) / 8).astype("f4"), jets),
            "eta": ak.unflatten((r.integers(-20, 21, total) / 8).astype("f4"), jets),
        })
        events = {
            "Jet": jet,
            "run": np.ones(n, "u4"),
            "luminosityBlock": (np.arange(first, first + n) // 1000 + 1).astype("u4"),
            "event": np.arange(first, first + n, dtype="u8"),
            "MET": (r.integers(0, 1600, n) / 8).astype("f4"),
            "weight": (r.integers(1, 9, n) / 8).astype("f4"),
        }
        events.update({f"x{i}": (r.integers(0, 256, n) / 4).astype("f4") for i in range(10)})
        if first == 0:
            f.mktree("Events", {name: (value.type.content if name == "Jet" else value.dtype) for name, value in events.items()})
        f["Events"].extend(events)
