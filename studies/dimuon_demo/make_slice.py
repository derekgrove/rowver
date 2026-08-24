"""One-time: slice CERN Open Data for the demo — no physics here, just fewer bytes.

Source: CMS DoubleMuParked 2012B+C, the AOD2NanoAODOutreachTool NanoAOD-derived file
on opendata.cern.ch (CC0). It holds 61.5M entries; the first N_ENTRIES stream over
HTTPS into open_data_slice.root next to this script, which is what the cart
points at. Needs uproot plus aiohttp: python studies/dimuon_demo/make_slice.py
"""
import pathlib

import uproot

SRC = ("https://opendata.cern.ch/eos/opendata/cms/derived-data/"
       "AOD2NanoAODOutreachTool/Run2012BC_DoubleMuParked_Muons.root")
OUT = pathlib.Path(__file__).with_name("open_data_slice.root")
N_ENTRIES = 600_000
BRANCHES = ["nMuon", "Muon_pt", "Muon_eta", "Muon_phi", "Muon_mass", "Muon_charge"]


def main() -> None:
    tree = uproot.open(SRC + ":Events")
    arrays = tree.arrays(BRANCHES, entry_stop=N_ENTRIES)
    with uproot.recreate(OUT) as out:
        out["Events"] = {name: arrays[name] for name in BRANCHES}
    print(f"wrote {OUT} — {len(arrays):,} entries")


if __name__ == "__main__":
    main()
