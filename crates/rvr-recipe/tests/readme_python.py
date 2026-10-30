"""The Python side of the README's "From Python to rvr": each Awkward expression, run on the
README's example events, gives the result the README shows (tests/readme.rs checks the rvr side
against the same results). Needs awkward and numpy: python tests/readme_python.py"""

import json
import pathlib
import re
import sys

import awkward as ak
import numpy as np

README = pathlib.Path(__file__).parent.parent / "README.md"


def f4(lists):
    return ak.values_astype(ak.Array(lists), np.float32)


# The table "The example events", with the types uproot reads from NanoAOD.
ev = ak.Array({
    "Jet_pt": f4([[65, 22, 41], [], [120, 35]]),
    "Jet_eta": f4([[0.5, -2.1, 1.2], [], [-0.2, 0.9]]),
    "Jet_phi": f4([[0.1, 2.9, -1.0], [], [3.0, -3.0]]),
    "Jet_mass": f4([[8, 4, 6], [], [12, 5]]),
    "nJet": np.array([3, 0, 2], np.uint32),
    "Muon_pt": f4([[44, 12], [25], []]),
    "Muon_eta": f4([[0.4, 1.9], [-0.3], []]),
    "Muon_phi": f4([[0.2, -2.8], [1.5], []]),
    "Muon_tightId": ak.Array([[True, False], [True], []]),
    "Muon_charge": ak.values_astype(ak.Array([[1, -1], [-1], []]), np.int32),
    "MET_pt": np.array([31.5, 85, 16], np.float32),
    "MET_phi": np.array([-0.5, 2.0, 1.0], np.float32),
})


def same(ours, shown):
    """Equal to the digits the README writes; float32 arithmetic is allowed a little more."""
    if isinstance(shown, list):
        return isinstance(ours, list) and len(ours) == len(shown) and all(map(same, ours, shown))
    if isinstance(shown, bool) or isinstance(ours, bool):
        return ours == shown
    return abs(ours - shown) <= 1e-5 * max(abs(shown), 1)


def run(code):
    """Run Python whose last line is the expression to give."""
    *setup, last = code.strip().split("\n")
    space = {"ak": ak, "np": np, "ev": ev}
    exec("\n".join(setup), space)
    return eval(last, space).tolist()


def examples(text):
    """(Python, result) for every row of the table and every longer example."""
    section = text.split("## From Python to rvr", 1)[1].split("\n## ", 1)[0]
    for row in re.findall(r"^\| `(.+?)` \| `.+?` \| `(.+?)` \|$", section, re.M):
        yield row
    for code, result in re.findall(r"```python\n(.*?)```.*?^Both give `(.+?)`", section, re.M | re.S):
        yield code, result


wrong, checked = [], 0
for code, result in examples(README.read_text()):
    checked += 1
    ours = run(code)
    if not same(ours, json.loads(result)):
        wrong.append(f"{code.strip()}\n  README shows {result}, Awkward gives {ours}")
if checked < 15 or wrong:
    sys.exit(f"{len(wrong)} of {checked} Python examples are wrong:\n" + "\n".join(wrong))
print(f"All {checked} Python examples give the result the README shows.")
