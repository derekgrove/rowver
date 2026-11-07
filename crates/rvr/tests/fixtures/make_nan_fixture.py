"""Write nan.root: five entries of `Events` whose floats are NaNs of either sign, with a payload, and signalling."""
import sys

import awkward as ak
import numpy as np
import uproot

f8 = np.array([0x3FF8000000000000, 0x7FF8000000000000, 0xFFF8000000000000, 0x7FF8000000000001, 0x7FF0000000000001], dtype=np.uint64).view(np.float64)
f4 = np.array([0x3FC00000, 0x7FC00000, 0xFFC00000, 0x7FC00001, 0x7F800001], dtype=np.uint32).view(np.float32)
v = ak.unflatten(f8[[0, 2, 2, 0, 3]], [1, 1, 0, 2, 1])
with uproot.recreate(sys.argv[1], compression=uproot.ZLIB(9)) as file:
    file.mktree("Events", {"x": "f8", "y": "f4", "v": "var * float64"})
    file["Events"].extend({"x": f8, "y": f4, "v": v})
