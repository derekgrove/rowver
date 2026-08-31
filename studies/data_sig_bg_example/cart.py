"""Minimal 2018 data + background + signal study using CMS DAS files."""
from rowver import Cart


def KEEP(events, metadata):
    threshold = 0.0 if metadata["kind"] == "signal" else 175.0
    return (events.nJet >= 1) & (events.MET_pt > threshold)


FILESET = {
    "JetMET_Data": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/data/Run2018A/MET/NANOAOD/"
            "UL2018_MiniAODv2_NanoAODv9-v2/110000/"
            "4C8B6DD1-441E-1E4C-87E9-B7D97060E508.root": "Events",
        },
        "metadata": {"kind": "data", "weights": None},
    },
    "Stealth_TChiWZ": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/mc/"
            "RunIISummer20UL18NanoAODv9/"
            "Stealth_TChiWZ_Sget90ToGluGlu_TuneCP5_13TeV-madgraphMLM-pythia8/"
            "NANOAODSIM/106X_upgrade2018_realistic_v16_L1v1-v1/120000/"
            "2706CE7F-7501-7C4A-B913-A515578D0933.root": "Events",
        },
        "metadata": {"kind": "signal", "weights": "genWeight"},
    },
    "TTJets": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/mc/"
            "RunIISummer20UL18NanoAODv9/"
            "TTJets_DiLept_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
            "106X_upgrade2018_realistic_v16_L1v1-v1/2820000/"
            "43816440-01B0-B441-9B0B-E17009EAF562.root": "Events",
        },
        "metadata": {"kind": "mc", "weights": "genWeight"},
    },
    "WJets": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/mc/"
            "RunIISummer20UL18NanoAODv9/"
            "WJetsToLNu_HT-400To600_TuneCP5_13TeV-madgraphMLM-pythia8/"
            "NANOAODSIM/106X_upgrade2018_realistic_v16_L1v1-v1/270000/"
            "71AB3D1B-7283-6B46-A0BE-04769BC211A8.root": "Events",
        },
        "metadata": {"kind": "mc", "weights": "genWeight"},
    },
    "DYJets": {
        "files": {
            "root://cms-xrd-global.cern.ch//store/mc/"
            "RunIISummer20UL18NanoAODv9/"
            "DYJetsToLL_M-50_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
            "106X_upgrade2018_realistic_v16_L1v1-v1/280000/"
            "79CBCFF6-EE37-D34B-AF8B-1AC9A2562EB1.root": "Events",
        },
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
    title="2018 MET data + backgrounds + signal",
    note="Five CMS DAS files; at least one jet, with MET > 175 GeV except signal.",
)
