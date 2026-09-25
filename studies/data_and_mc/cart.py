"""2017 single-muon data against W+jets MC, both from CMS DAS. Data has no generator
weight, so each sample's metadata names its weight branch, and data gets a weight of 1."""

import awkward as ak
import numpy as np

from rowver import Cart

DATA = ("root://cms-xrd-global.cern.ch//store/data/Run2017F/SingleMuon/NANOAOD/"
        "UL2017_MiniAODv2_NanoAODv9-v1/120000/E540DF0D-BBF8-CA4F-9560-69ADFC15E2B9.root")
WJETS = ("root://cms-xrd-global.cern.ch//store/mc/RunIISummer20UL17NanoAODv9/"
         "WJetsToLNu_TuneCP5_13TeV-madgraphMLM-pythia8/NANOAODSIM/"
         "106X_mc2017_realistic_v9-v1/280000/B241D117-66F8-5641-9898-14D648CF7CED.root")


def keep(events):
    """The single-muon trigger fired and the leading muon is tight with pT above 30 GeV."""
    return (events.HLT_IsoMu27 & (events.nMuon >= 1)
            & ak.fill_none(ak.firsts(events.Muon_tightId & (events.Muon_pt > 30.0)), False))


def weight(events, metadata):
    """The sample's weight branch, or 1 for every event when it has none."""
    if metadata["weight"] is None:
        return np.ones(len(events), dtype=np.float32)
    return events[metadata["weight"]]


def mt(events):
    """Transverse mass of the leading muon and the MET."""
    pt, phi = events.Muon_pt[:, 0], events.Muon_phi[:, 0]
    return np.sqrt(2 * pt * events.MET_pt * (1 - np.cos(phi - events.MET_phi)))


def jet_bin(events):
    """0, 1, or 2 for two or more jets."""
    return np.minimum(events.nJet, 2)


FILESET = {
    "SingleMuon 2017F": {"files": {DATA: "Events"}, "metadata": {"weight": None}},
    "W to l nu (2017)": {"files": {WJETS: "Events"}, "metadata": {"weight": "genWeight"}},
}

CART = Cart(
    fileset=FILESET,
    keep=keep,
    columns={
        "MET": "MET_pt",
        "Muon_pt_lead": lambda events: events.Muon_pt[:, 0],
        "mT": mt,
        "jet_bin": jet_bin,
        "weight": weight,
    },
    labels={"jet_bin": {0: "0 jets", 1: "1 jet", 2: "2+ jets"}},
    title="Data and MC example",
    note="2017 SingleMuon data and W+jets MC from CMS DAS; one tight muon above 30 GeV. Data weight is 1.",
)
