"""TEMPLATE —

    Copy this directory, rename it, and edit this file.

    cart.py     the whole study: which files, what to keep, what columns   (mandatory)
    view.py     how the page opens                                         (optional)

    python run_rowver.py <your study>

A cart is ordinary python, so the fileset is an ordinary dict — no JSON riding alongside.
`python cart.py <your study> --json` prints it as JSON whenever you want to read it that
way, and `python cart.py <your file>.rvr` prints what a built file actually holds.

The paths below are placeholders. For files that are not on this machine, two worked
studies ship beside this one, each with a cart that matches its own files:

    studies/remote_example/   root:// open data, no certificate needed
    studies/grid_example/     CMS DAS through the CERN and Fermilab redirectors
    studies/mixed_demo/       real data and weighted MC on ONE cart
"""
from cart import Cart

import awkward as ak
import numpy as np


#: (optional) what an integer column's values mean, so a chip reads a word, not a number.
#: Every value present in the data needs an entry or the build refuses.
LABELS = {"nJet": {0: "0 jets", 1: "1 jet", 2: "2 jets", 3: "3 or more"}}

NJET_MAX = 3


def leading(events: ak.Array, branch: str) -> ak.Array:
    """A helper, not a hook — name it whatever you like."""
    return events[branch][:, 0]


def PREPARE(events: ak.Array) -> ak.Array:
    """(optional) Runs first. Whatever it returns BECOMES the chunk KEEP and COLUMNS see."""
    return events[events.nJet >= 1]


def KEEP(events: ak.Array, meta: dict) -> ak.Array:
    """(mandatory) Returns a boolean mask: which events to keep."""
    return events.MET_pt > (100 if meta["is_MC"] else 120)


#: (mandatory) one entry per column of the page, and one number per event.
#: Every hook here may take a second argument, that sample's metadata — see KEEP above.
COLUMNS = {
    "MET": "MET_pt",                                        # a string: a branch, copied
    "nJet": lambda events: ak.values_astype(                # a lambda: a one-liner
        np.minimum(events.nJet, NJET_MAX), np.int32),
    "PT_jet_lead": lambda events: leading(events, "Jet_pt"),
    "AbsEta_jet_lead": lambda events: abs(leading(events, "Jet_eta")),
}

#: Coffea's shape. `metadata` is yours to fill and reaches every hook, with the file being
#: read folded in under `sample`, `file` and `tree`.
#:
#: `metadata["weights"]` is the one key the cart reads itself: it says what fills THIS
#: sample's weights column, in the same grammar as COLUMNS — a branch name, or a function.
#: Every sample may name a different branch. `None` means real data: its rows are 1.0 and
#: the page never scales them by a luminosity, which is how data and MC live on one cart.
#: EVERY sample must say one or the other — silence is refused, because silence and "no
#: weights" look identical in a dict and that is how MC quietly ships at weight 1.0.
FILESET = {
    "some signal sample": {
        "files": {"/path/to/your/signal/*.root": "Events"},
        "metadata": {"is_MC": True, "weights": "genWeight"},
    },
    "some background sample": {
        "files": {"/path/to/your/background/*.root": "Events"},
        "metadata": {"is_MC": True, "weights": "LHEWeight_originalXWGTUP"},
    },
    "some data sample": {
        "files": {"/path/to/your/data/*.root": "Events"},
        "metadata": {"is_MC": False, "weights": None},   # real data — say so OUT LOUD
    },
}


CART = Cart(
    name="template",
    fileset=FILESET,
    columns=COLUMNS,
    keep=KEEP,
    prepare=PREPARE,
    labels=LABELS,
)
