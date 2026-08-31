"""Larger 2018 DATA + MC study using public CMS NanoAOD files.

The six samples use nine files from the CMS data catalog, all streamed through
the global XRootD redirector. Requires a valid CMS grid proxy in the environment.

The keep cut (MET > 175, >= 1 jet) is a skim, not a trigger-plateau argument; set
the luminosity on the page if a scaled yield is wanted, and read shapes with the
usual below-plateau caution.
"""
from rowver import Cart

CMS = "root://cms-xrd-global.cern.ch/"
MC = {"kind": "mc", "weights": "genWeight"}
OBSERVED = {"kind": "data", "weights": None}


# helper functions
def _sample(metadata: dict, *paths: str) -> dict:
    return {"files": {f"{CMS}{path}": "Events" for path in paths},
            "metadata": metadata}


def KEEP(events):
    return (events.nJet >= 1) & (events.MET_pt > 175.0)


FILESET = {
    "MET 2018A DATA": _sample(
        OBSERVED,
        "/store/data/Run2018A/MET/NANOAOD/UL2018_MiniAODv2_NanoAODv9-v2/"
        "110000/4C8B6DD1-441E-1E4C-87E9-B7D97060E508.root",
    ),
    "MET 2018D DATA": _sample(
        OBSERVED,
        "/store/data/Run2018D/MET/NANOAOD/UL2018_MiniAODv2_NanoAODv9-v1/"
        "130000/CB492305-1627-D441-A488-4EF2E89F4A0D.root",
    ),
    "TTJets DiLept MC": _sample(
        MC,
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "TTJets_DiLept_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/2820000/"
        "43816440-01B0-B441-9B0B-E17009EAF562.root",
    ),
    "TTTo2L2Nu MC": _sample(
        MC,
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "TTTo2L2Nu_TuneCP5_13TeV-powheg-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/130000/"
        "1656732C-0CD4-F54B-B39D-19CA08E18A77.root",
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "TTTo2L2Nu_TuneCP5_13TeV-powheg-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/130000/"
        "44187D37-0301-3942-A6F7-C723E9F4813D.root",
    ),
    "DYJets M-50 MC": _sample(
        MC,
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "DYJetsToLL_M-50_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/280000/"
        "79CBCFF6-EE37-D34B-AF8B-1AC9A2562EB1.root",
    ),
    "WJets HT 400-600 MC": _sample(
        MC,
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "WJetsToLNu_HT-400To600_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/270000/"
        "71AB3D1B-7283-6B46-A0BE-04769BC211A8.root",
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "WJetsToLNu_HT-400To600_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/280000/"
        "E88DADB8-819A-9240-A7A3-A5AE35627754.root",
        "/store/mc/RunIISummer20UL18NanoAODv9/"
        "WJetsToLNu_HT-400To600_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
        "106X_upgrade2018_realistic_v16_L1v1-v1/280000/"
        "143A2595-60A4-E441-A0E4-9C1A6BE4EBD4.root",
    ),
}


CART = Cart(
    fileset=FILESET,
    keep=KEEP,
    columns={
        "MET": "MET_pt",
        "MET_phi": "MET_phi",
        "PT_jet_lead": lambda events: events.Jet_pt[:, 0],
        "AbsEta_jet_lead": lambda events: abs(events.Jet_eta[:, 0]),
        "Njet": "nJet",
        "Nmu": "nMuon",
        "Nele": "nElectron",
        "Npv": "PV_npvs",
    },
    title="2018 MET DATA + public UL18 MC",
    note="Nine public CMS NanoAOD files; MET > 175 GeV and at least one jet.",
)
