"""Regenerate the small native-reader fixtures with RowVer's public v13 writer."""
import hashlib
import os
import pathlib
import tempfile

import awkward as ak
import numpy as np

from rowver import derive, rvr

DERIVE_SOURCE = "nlep_twice = nlep * 2; weighted_met = MET * xsec (0 where a sample has none); Muon_pt_half = Muon_pt / 2\n"


def nlep_twice(columns):
    """Twice the lepton count, an integer column."""
    return columns["nlep"] * 2


def weighted_met(columns):
    """MET scaled by the sample column xsec, zero where a sample has none."""
    return np.nan_to_num(columns["xsec"]) * columns["MET"]


def muon_pt_half(columns):
    """Half of every muon pt, a jagged column."""
    return columns["Muon_pt"] / 2


def save(path, name):
    """Store one file's bytes as wrapped hexadecimal text."""
    fixture = path.read_bytes().hex()
    target = pathlib.Path(__file__).parent / "fixtures" / name
    target.parent.mkdir(exist_ok=True)
    target.write_text("\n".join(fixture[index:index + 96] for index in range(0, len(fixture), 96)) + "\n")


def main():
    """Press deterministic typed columns and retain only their textual hex fixture."""
    columns = {
        "nlep": np.array([2, 0, 1, 3, 2], dtype="i4"),
        "MET": np.array([20, 0, 40, 30, 10], dtype="f4"),
        "Muon_pt": ak.unflatten(np.array([10, 20, 30, 40, 50, 60], dtype="f4"), [2, 0, 1, 3, 0]),
        "signed_i1": np.array([-128, -1, 0, 1, 127], dtype="i1"),
        "signed_i2": np.array([-32768, -1, 0, 1, 32767], dtype="i2"),
        "signed_i8": np.array([-2**63, -1, 0, 2**53 + 1, 2**63 - 1], dtype="i8"),
        "unsigned_u1": np.array([0, 1, 127, 128, 255], dtype="u1"),
        "unsigned_u2": np.array([0, 1, 32767, 32768, 65535], dtype="u2"),
        "unsigned_u4": np.array([0, 1, 2**31, 2**32 - 2, 2**32 - 1], dtype="u4"),
        "unsigned_u8": np.array([0, 1, 2**53 + 1, 2**64 - 2, 2**64 - 1], dtype="u8"),
        "flag": np.array([False, True, False, True, True], dtype=bool),
        "double": np.array([-1.25, 0, 1e-5, 1e16, 1.2345678901234567], dtype="f8"),
    }
    samples = {
        "A": {"columns": {name: value[:3] for name, value in columns.items()}, "entry": np.array([2, 0, 4], dtype="i8")},
        "B": {"columns": {name: value[3:] for name, value in columns.items()}, "entry": np.array([2**53 + 1, 1], dtype="i8")},
    }
    files = [
        {"sample": "A", "path": "fixture-a.root", "rows": 2},
        {"sample": "A", "path": "fixture-empty.root", "rows": 0},
        {"sample": "A", "path": "fixture-a2.root", "rows": 1},
        {"sample": "B", "path": "fixture-b.root", "rows": 2},
    ]
    data, arrays = rvr.pack(samples, {"flag": {0: "off", 1: "on"}}, files)
    source = "# Synthetic reader test: π and μ\n"
    made = {
        "study": "native-reader-test", "built": "2026-10-05T00:00:00Z", "by": "rowver 13",
        "cart": {"source": source, "sha256": hashlib.sha256(source.encode()).hexdigest()},
        "files": {
            "A": {"files": {path: {"tree": "Events", "uuid": path.removesuffix(".root") + "-uuid", "entries": 10} for path in ["fixture-a.root", "fixture-empty.root", "fixture-a2.root"]}, "metadata": {"xsec": 87.3, "kind": "background"}},
            "B": {"files": {"fixture-b.root": {"tree": "Events", "uuid": "fixture-b-uuid", "entries": 2**53 + 10}}, "metadata": {}},
        },
        "fixture_floats": [1e-4, 1e-5, 1e15, 1e16, 1e20, 1.1e-5, 0.0, -0.0, 1.2345678901234567, 1e-308, 1345740272956748.25, -101568684769612.625],
    }
    data["sample_cols"] = rvr.sample_cols(list(samples), made["files"], list(columns))
    made["sha256"] = rvr.made_checksum(made)
    header = {"rowver": 13, "data": data, "made": made, "fixture_tool": {"preserved": "μ"}}
    with tempfile.TemporaryDirectory(prefix="renorm-fixture-") as temporary:
        path = pathlib.Path(temporary) / "fixture.rvr"
        rvr.write(path, header, arrays)
        save(path, "rowver-v13.hex")
        home = os.getcwd()
        os.chdir(temporary)  # so the derived file records its parent by name, not by a local path
        try:
            derive.derive("fixture.rvr", "derived.rvr", DERIVE_SOURCE,
                          {"nlep_twice": nlep_twice, "weighted_met": weighted_met, "Muon_pt_half": muon_pt_half},
                          by="renorm fixture", runtime="fixture")
            save(pathlib.Path("derived.rvr"), "rowver-v13-derived.hex")
        finally:
            os.chdir(home)

if __name__ == "__main__":
    main()
