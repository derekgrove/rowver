"""One Run 2 Drell-Yan MC file from CMS DAS: events with two muons, a few branches copied as they are."""

from rowver import Cart

DY = ("root://cms-xrd-global.cern.ch//store/mc/RunIISummer20UL18NanoAODv9/"
      "DYJetsToLL_M-50_TuneCP5_13TeV-amcatnloFXFX-pythia8/NANOAODSIM/"
      "106X_upgrade2018_realistic_v16_L1v1-v2/230000/F06B02A3-7656-3D4D-9D51-B593DEB2E98D.root")


def keep(events):
    """At least two muons."""
    return events.nMuon >= 2


FILESET = {
    "DY to LL (2018)": {"files": {DY: "Events"}, "metadata": {}},
}

CART = Cart(
    fileset=FILESET,
    keep=keep,
    columns={
        "MET": "MET_pt",
        "Njet": "nJet",
        "Nmuon": "nMuon",
        "Muon_pt": "Muon_pt",
        "genWeight": "genWeight",
    },
    title="Simple example",
    note="One UL18 Drell-Yan NanoAOD file from CMS DAS, at least two muons.",
)
