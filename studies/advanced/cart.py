"""Z to mu mu in Run 2 and Run 3, data and MC in one file, all from CMS DAS.

Run 2 (NanoAOD v9) and Run 3 (NanoAOD v12) spell some branches differently, and
data has no generator weight. Each sample's metadata says which spelling it uses,
which trigger to require, and which branch holds its weight; the hooks read it."""

import awkward as ak
import numpy as np

from rowver import Cart

DAS = "root://cms-xrd-global.cern.ch/"

BRANCH = {
    "v9": {"rho": "fixedGridRhoFastjetAll", "ele_wp90": "Electron_mvaFall17V2Iso_WP90"},
    "v12": {"rho": "Rho_fixedGridRhoFastjetAll", "ele_wp90": "Electron_mvaIso_WP90"},
}


def branch(events, metadata, key):
    """This sample's spelling of a branch."""
    return events[BRANCH[metadata["nano"]][key]]


def prepare(events):
    """Tight, isolated muons above 15 GeV, kept for keep and the columns."""
    good = events.Muon_tightId & (events.Muon_pfRelIso04_all < 0.15) & (events.Muon_pt > 15.0)
    events["good_pt"] = events.Muon_pt[good]
    events["good_eta"] = events.Muon_eta[good]
    events["good_phi"] = events.Muon_phi[good]
    events["good_charge"] = events.Muon_charge[good]
    return events


def keep(events, metadata):
    """The sample's trigger fired and there are exactly two good muons of opposite charge."""
    two = ak.num(events.good_pt) == 2
    opposite = ak.sum(events.good_charge, axis=1) == 0
    return events[metadata["trigger"]] & two & opposite


def dimuon(events):
    """Mass and pT of the two good muons, treated as massless."""
    pt, eta, phi = events.good_pt, events.good_eta, events.good_phi
    px, py, pz = ak.sum(pt * np.cos(phi), axis=1), ak.sum(pt * np.sin(phi), axis=1), ak.sum(pt * np.sinh(eta), axis=1)
    e = ak.sum(pt * np.cosh(eta), axis=1)
    return np.sqrt(np.maximum(e**2 - px**2 - py**2 - pz**2, 0)), np.hypot(px, py)


def weight(events, metadata):
    """The sample's weight branch, or 1 for every event when it has none."""
    if metadata["weight"] is None:
        return np.ones(len(events), dtype=np.float32)
    return events[metadata["weight"]]


def era(events, metadata):
    """The sample's era as a small integer."""
    return np.full(len(events), metadata["era"], dtype=np.int32)


FILESET = {
    "SingleMuon 2018D": {
        "files": {f"{DAS}/store/data/Run2018D/SingleMuon/NANOAOD/UL2018_MiniAODv2_NanoAODv9-v1/280000/5202499B-3C6F-1F4A-BD6B-FC71C540291F.root": "Events"},
        "metadata": {"nano": "v9", "trigger": "HLT_IsoMu24", "weight": None, "era": 0},
    },
    "DY to LL (2018)": {
        "files": {f"{DAS}/store/mc/RunIISummer20UL18NanoAODv9/DYJetsToLL_M-50_TuneCP5_13TeV-amcatnloFXFX-pythia8/NANOAODSIM/106X_upgrade2018_realistic_v16_L1v1-v2/230000/F06B02A3-7656-3D4D-9D51-B593DEB2E98D.root": "Events"},
        "metadata": {"nano": "v9", "trigger": "HLT_IsoMu24", "weight": "genWeight", "era": 0},
    },
    "Muon 2022C": {
        "files": {f"{DAS}/store/data/Run2022C/Muon/NANOAOD/22Sep2023-v1/50000/53995cbd-0258-4ec6-9873-e3ef184ac1c7.root": "Events"},
        "metadata": {"nano": "v12", "trigger": "HLT_IsoMu24", "weight": None, "era": 1},
    },
    "DY to 2L (2022)": {
        "files": {f"{DAS}/store/mc/Run3Summer22NanoAODv12/DYto2L-2Jets_MLL-50_TuneCP5_13p6TeV_amcatnloFXFX-pythia8/NANOAODSIM/130X_mcRun3_2022_realistic_v5-v5/120000/cbdb11cb-4d93-4080-86a4-4560d30cd92a.root": "Events"},
        "metadata": {"nano": "v12", "trigger": "HLT_IsoMu24", "weight": "genWeight", "era": 1},
    },
    "TT to 2L2Nu (2022)": {
        "files": {f"{DAS}/store/mc/Run3Summer22NanoAODv12/TTto2L2Nu_TuneCP5_13p6TeV_powheg-pythia8/NANOAODSIM/130X_mcRun3_2022_realistic_v5-v2/40000/c6f4f598-cd90-49e2-bc75-3ff6db17144d.root": "Events"},
        "metadata": {"nano": "v12", "trigger": "HLT_IsoMu24", "weight": "genWeight", "era": 1},
    },
}

CART = Cart(
    fileset=FILESET,
    prepare=prepare,
    keep=keep,
    columns={
        "mll": lambda events: dimuon(events)[0],
        "pT_ll": lambda events: dimuon(events)[1],
        "Muon_pt": lambda events: events.good_pt,
        "Muon_eta": lambda events: events.good_eta,
        "Jet_pt": "Jet_pt",
        "Nbjet": lambda events: ak.sum(events.Jet_btagDeepFlavB > 0.3, axis=1),
        "Nele_wp90": lambda events, metadata: ak.sum(branch(events, metadata, "ele_wp90"), axis=1),
        "rho": lambda events, metadata: branch(events, metadata, "rho"),
        "MET": "MET_pt",
        "era": era,
        "weight": weight,
    },
    labels={"era": {0: "2018 (Run 2)", 1: "2022 (Run 3)"}},
    title="Advanced example",
    note="Z to mu mu: 2018 and 2022 data and MC from CMS DAS, two tight isolated opposite-charge muons. Data weight is 1.",
)
