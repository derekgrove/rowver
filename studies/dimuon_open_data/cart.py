"""The CMS Open Data dimuon spectrum: 2012 DoubleMuParked collisions read over https.
The first 800,000 events, every one with two or more muons."""

import awkward as ak
import numpy as np

from rowver import Cart

MUONS = ("https://opendata.cern.ch/eos/opendata/cms/derived-data/AOD2NanoAODOutreachTool/"
         "Run2012BC_DoubleMuParked_Muons.root")


def prepare(events):
    """Order each event's muons by pT, highest first; this file does not."""
    order = ak.argsort(events.Muon_pt, ascending=False)
    for name in ("Muon_pt", "Muon_eta", "Muon_phi", "Muon_charge"):
        events[name] = events[name][order]
    return events


def keep(events):
    """Two or more muons."""
    return events.nMuon >= 2


def mass(events):
    """Invariant mass of the two leading muons, treating them as massless."""
    pt, eta, phi = events.Muon_pt, events.Muon_eta, events.Muon_phi
    return np.sqrt(2 * pt[:, 0] * pt[:, 1] * (np.cosh(eta[:, 0] - eta[:, 1]) - np.cos(phi[:, 0] - phi[:, 1])))


def charge(events):
    """Sum of the two leading muons' charges: -2, 0, or 2."""
    return events.Muon_charge[:, 0] + events.Muon_charge[:, 1]


FILESET = {
    "DoubleMuParked 2012B+C": {"files": {MUONS: {"tree": "Events", "stop": 800_000}}, "metadata": {}},
}

CART = Cart(
    fileset=FILESET,
    prepare=prepare,
    keep=keep,
    columns={
        "mass": mass,
        "charge": charge,
        "Nmuon": "nMuon",
        "Muon1_pt": lambda events: events.Muon_pt[:, 0],
        "Muon1_eta": lambda events: events.Muon_eta[:, 0],
        "Muon2_pt": lambda events: events.Muon_pt[:, 1],
    },
    labels={"charge": {-2: "mu- mu-", 0: "mu+ mu-", 2: "mu+ mu+"}},
    title="CMS Open Data dimuons",
    note="2012 DoubleMuParked (Runs B and C) from CMS Open Data, the first 800,000 events, those with two or more muons.",
)
