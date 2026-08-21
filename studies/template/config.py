"""TEMPLATE —

    Copy this directory, rename it, and edit the three files in it.

    fileset.json    which ROOT files, and what each sample is called   (mandatory)
    config.py       this file                                          (mandatory)
    view.py     how the page opens                                 (optional)

    python run_rowver.py <your study>

    fileset.json holds local paths. For files that are not on this machine, two
    worked studies ship beside this one, each with a config that matches its own files:

        studies/remote_example/   root:// open data, no certificate needed
        studies/grid_example/     CMS DAS through the CERN and Fermilab redirectors
"""
import awkward as ak
import numpy as np


#: (optional) what an integer column's values mean, so a chip reads a word, not a number.
#: Every value present in the data needs an entry or the build refuses.
LABELS = {"nJet": {0: "0 jets", 1: "1 jet", 2: "2 jets", 3: "3 or more"}}

#: (optional, defaults to True) do these events carry weights?
HAS_WEIGHTS = True

#: (optional, defaults to "weight") which COLUMNS entry holds them
WEIGHTS_BRANCH = "weight"

NJET_MAX = 3


def leading(events: ak.Array, branch: str) -> ak.Array:
    """A helper, not a hook — name it whatever you like."""
    return events[branch][:, 0]


def PREPARE(events: ak.Array) -> ak.Array:
    """(optional) Runs first. Whatever it returns BECOMES the chunk KEEP and COLUMNS see."""
    return events[events.nJet >= 1]


def KEEP(events: ak.Array) -> ak.Array:
    """(mandatory) Returns a boolean mask: which events to keep."""
    return events.MET_pt > 100


#: (mandatory) one entry per column of the page, and one number per event.
COLUMNS = {
    "weight": "genWeight",                                  # a string: a branch, copied
    "MET": "MET_pt",                                        # a string: a branch, copied
    "nJet": lambda events: ak.values_astype(                # a lambda: a one-liner
        np.minimum(events.nJet, NJET_MAX), np.int32),
    "PT_jet_lead": lambda events: leading(events, "Jet_pt"),
    "AbsEta_jet_lead": lambda events: abs(leading(events, "Jet_eta")),
}
