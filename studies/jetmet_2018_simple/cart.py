"""Minimal 2018 DATA + MC study using one CMS DAS file per sample."""
from rowver import Cart


def KEEP(events):
    return (events.nJet >= 1) & (events.MET_pt > 175.0)


FILESET = {
    "MET 2018A DATA": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/data/Run2018A/MET/NANOAOD/"
            "UL2018_MiniAODv2_NanoAODv9-v2/110000/"
            "4C8B6DD1-441E-1E4C-87E9-B7D97060E508.root": "Events",
        },
        # DATA events are counts, so RowVer must not weight them.
        "metadata": {"kind": "data", "weights": None},
    },
    "TTJets DiLept MC": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/mc/"
            "RunIISummer20UL18NanoAODv9/"
            "TTJets_DiLept_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
            "106X_upgrade2018_realistic_v16_L1v1-v1/2820000/"
            "43816440-01B0-B441-9B0B-E17009EAF562.root": "Events",
        },
        # MC events use the per-event generator weight stored in NanoAOD.
        "metadata": {"kind": "mc", "weights": "genWeight"},
    },
}


CART = Cart(
    fileset=FILESET,
    keep=KEEP,
    columns={
        "MET": "MET_pt",
        "MET_phi": "MET_phi",
        "Njet": "nJet",
        "Nmu": "nMuon",
        "Nele": "nElectron",
    },
    title="2018 MET DATA + TTJets MC",
    note="One CMS DAS file per sample; MET > 175 GeV and at least one jet.",
)
