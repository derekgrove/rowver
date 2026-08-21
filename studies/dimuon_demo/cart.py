"""
This is a demo of formatting a .rvr file from CMS Open Data, found here: https://opendata.cern.ch/record/12341

In this config file we will see how we set up this file from start to finish.

A cart is one file. It holds the FILESET — which ROOT files, which trees, and whatever
metadata you want to hang on each sample — and the selection that turns them into columns.
Only KEEP and COLUMNS are required; the Cart carries on without the rest:

LABELS: dict (optional)
    - changes the in-page labelling of a particular column, ex. renaming the charge of the
      dimuon pair from -1 and 1 to "opposite sign" and "same sign"
    - if you leave it out it'll just show the -1 and 1 as toggles but it may not be clear to a user what they are selecting

FILESET: dict (mandatory)
    - coffea's shape: {sample: {"files": {path: tree}, "metadata": {...}}}
    - metadata["weights"] says what fills THAT sample's weights column — a branch name or
      a function, and every sample may name a different one
    - a sample declaring no weights is real data: its rows are 1.0 and the page never
      scales them by a luminosity, so data and MC belong on the same cart
    - this study is CMS Open Data, so it declares none

PREPARE: function (optional)
    - modifies your events, ex. reordering every muon field by descending pT
    - whatever it returns BECOMES the chunk, so KEEP and COLUMNS both see its work

KEEP: function (mandatory)
    - returns a boolean mask for your events, after PREPARE has prepared them

COLUMNS: dict (mandatory)
    - the names and columns for your .rvr file, one number per event
    - these become everything you can cut on and plot
"""

from cart import Cart

import awkward as ak
import numpy as np
import vector

# teaches awkward the Momentum4D behaviours for inv_mass
vector.register_awkward()

# LABELS is read by the next file, run_rowver.py, and writes it into the .rvr
# it is purely for aesthetics of the page, doesn't influence functionality in any way
LABELS = {
    "q_pair": {-1: "opposite sign", 1: "same sign"}
    }


# Since this will be run on real data there is no generator weight to apply and no
# cross-section to scale to. This will disable the luminosity scaling in the page.


# for simplicity I'm going to write some max values that I want to filter the events on
# will use these in KEEP to filter my events
PT_MAX = 600.0
ABSETA_MAX = 3.0
M_MAX = 2 * PT_MAX
NMUON_MAX = 5


# Helper function, get inv. mass of each event's two pT-leading muons
def inv_mass(events: ak.Array) -> ak.Array:

    #inner-helper: the i-th hardest muon, as a 4-vector `vector` knows how to add. float64
    # because the branches are float32, and E^2 - p^2 in single precision loses whole GeV
    def mu(i: int) -> ak.Array:
        f64 = lambda f: ak.values_astype(events[f][:, i], np.float64)
        return ak.zip({"pt": f64("Muon_pt"), "eta": f64("Muon_eta"),
                       "phi": f64("Muon_phi"), "mass": f64("Muon_mass")},
                      with_name="Momentum4D")
    return (mu(0) + mu(1)).mass


def PREPARE(events: ak.Array) -> ak.Array:
    """
    Drop events without two muons, then sort every muon field by descending pT.

    Dropping them HERE is what lets everything below index [:, 0] and [:, 1] directly.
    KEEP cannot make that guarantee for itself: `&` evaluates every term on every event,
    so an nMuon cut sitting in KEEP would not protect the terms beside it.
    """
    events = events[events.nMuon >= 2]
    order = ak.argsort(events.Muon_pt, axis=1, ascending=False)
    for f in ("Muon_pt", "Muon_eta", "Muon_phi", "Muon_mass", "Muon_charge"):
        events = ak.with_field(events, events[f][order], f)
    return events


def KEEP(events: ak.Array) -> ak.Array:
    """Both muons measured, and no more than NMUON_MAX of them. Charge pairing is a page cut."""
    return ((events.nMuon <= NMUON_MAX)
            & (events.Muon_pt[:, 0] < PT_MAX)
            & (events.Muon_pt[:, 1] < PT_MAX)
            & (abs(events.Muon_eta[:, 0]) < ABSETA_MAX)
            & (inv_mass(events) < M_MAX))


COLUMNS = {
    "M_mumu": inv_mass,
    "PT_mu_lead": lambda events: events.Muon_pt[:, 0],
    "PT_mu_sub": lambda events: events.Muon_pt[:, 1],
    "AbsEta_mu_lead": lambda events: abs(events.Muon_eta[:, 0]),
    "q_pair": lambda events: ak.values_astype(
        events.Muon_charge[:, 0] * events.Muon_charge[:, 1], np.int32),
    "nMuon": lambda events: ak.values_astype(events.nMuon, np.int32),
}

FILESET = {
    'Data · CMS Open DoubleMuParked 2012': {
        "files": {
            'studies/dimuon_demo/open_data_slice.root': 'Events',
        },
        "metadata": {'isRun3': False},
    },
}


CART = Cart(
    name='dimuon_demo',
    fileset=FILESET,
    columns=COLUMNS,
    keep=KEEP,
    prepare=PREPARE,
    labels=LABELS,
)
