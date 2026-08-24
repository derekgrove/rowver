"""MIXED DEMO — real data and weighted MC on one cart, which is what rowver 5 is for.

Both samples read the same committed open-data file, so the two hold the SAME events and
any difference on the page is the weighting alone. One declares no weights and is counted
one event at a time; the other declares a function, and its metadata says what that
function scales by. At luminosity L the first reads N and the second reads 2 x N x L.

That is the whole rule: a sample with no weights of its own is never multiplied by a
luminosity, however its group is set.
"""
from cart import Cart

import awkward as ak
import numpy as np

SLICE = "studies/dimuon_demo/open_data_slice.root"


def PREPARE(events: ak.Array) -> ak.Array:
    """Keep events with two muons, then sort every muon field by descending pT."""
    events = events[ak.num(events.Muon_pt, axis=1) >= 2]
    order = ak.argsort(events.Muon_pt, axis=1, ascending=False)
    for f in ("Muon_pt", "Muon_eta", "Muon_phi", "Muon_mass", "Muon_charge"):
        events = ak.with_field(events, events[f][order], f)
    return events


def KEEP(events: ak.Array) -> ak.Array:
    """Both muons measured."""
    return (events.Muon_pt[:, 0] < 600.0) & (events.Muon_pt[:, 1] < 600.0)


def stand_in_weight(events: ak.Array, meta: dict):
    """A stand-in generator weight: this file is real data, so the sample invents one."""
    return ak.ones_like(events.Muon_pt[:, 0]) * meta["scale"]


COLUMNS = {
    "PT_mu_lead": lambda events: events.Muon_pt[:, 0],
    "AbsEta_mu_lead": lambda events: abs(events.Muon_eta[:, 0]),
    "nMuon": lambda events: ak.values_astype(events.nMuon, np.int32),
}

FILESET = {
    "Data 2012 DoubleMuParked": {
        "files": {SLICE: "Events"},
        "metadata": {"is_MC": False, "weights": None},   # real data: no weights, ever scaled
    },
    "MC stand-in x2": {
        "files": {SLICE: "Events"},
        "metadata": {"is_MC": True, "weights": stand_in_weight, "scale": 2.0},
    },
}


CART = Cart(
    name="mixed_demo",
    fileset=FILESET,
    columns=COLUMNS,
    keep=KEEP,
    prepare=PREPARE,
)
