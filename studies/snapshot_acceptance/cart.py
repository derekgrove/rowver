"""Recipe-snapshot acceptance study, plan step 9.4.

Two local UL18 NanoAOD samples: TTJets DiLept weighted by the branch genWeight,
and WJetsToLNu weighted by a Python hook that scales genWeight by a k-factor
from the sample's metadata. PREPARE adds a lepton count, KEEP reads its MET
threshold from metadata, one integer column carries labels, and Jet_pt is a
jagged copy. ROWVER_SAMPLES names the directory holding the two UL18 sample
directories (default: ``samples`` beside this file). Press it, then run
reconstruct.py on the cartridge to lay the snapshot in ``made`` beside this file.
"""
import os

import numpy as np

from rowver import Cart

LOCAL = os.environ.get("ROWVER_SAMPLES", "samples")


def prepare(events):
    """Count the leptons once so KEEP and the columns share it."""
    events["nLep"] = events.nMuon + events.nElectron
    return events


def keep(events, metadata):
    """At least one jet and MET above the sample's threshold."""
    return (events.nJet >= 1) & (events.MET_pt > metadata["met_cut"])


def scaled(events, metadata):
    """The generator weight times the sample's k-factor."""
    return events.genWeight * metadata["k_factor"]


def channel(events):
    """0 hadronic, 1 one lepton, 2 two or more leptons."""
    return np.minimum(events.nLep, 2)


FILESET = {
    "TTJets DiLept": {
        "files": {
            f"{LOCAL}/TTJets_DiLept_TuneCP5_13TeV_2018UL/707E5366-52C7-C64C-BE69-2FDE6E109E71.root": "Events",
            f"{LOCAL}/TTJets_DiLept_TuneCP5_13TeV_2018UL/1CC920F3-1F58-3444-8551-618DCE2E12BE.root": "Events",
        },
        "metadata": {"kind": "mc", "xsec_pb": 87.3, "met_cut": 150.0, "weights": "genWeight"},
    },
    "WJets HT 400-600": {
        "files": {
            f"{LOCAL}/WJetsToLNu_TuneCP5_13TeV_2018UL/C17B82E7-01B7-9B4F-84F7-EA9703AC1CFB.root": "Events",
        },
        "metadata": {"kind": "mc", "xsec_pb": 48.9, "k_factor": 1.21, "met_cut": 200.0,
                     "weights": scaled},
    },
}


CART = Cart(
    fileset=FILESET,
    prepare=prepare,
    keep=keep,
    columns={
        "MET": "MET_pt",
        "MET_phi": "MET_phi",
        "Njet": "nJet",
        "Nlep": lambda events: events.nLep,
        "channel": channel,
        "Jet_pt": "Jet_pt",
    },
    labels={"channel": {0: "hadronic", 1: "one lepton", 2: "two or more leptons"}},
    title="Recipe snapshot acceptance",
    note="Two local UL18 NanoAOD samples, one branch weight and one Python weight.",
)
