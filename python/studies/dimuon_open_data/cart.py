"""The CMS Open Data dimuon spectrum: 2012 DoubleMuParked collisions read over https.
The first 800,000 events, every one with two or more muons."""

import awkward as ak
import numpy as np

from rowver import Cart


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


def muon1_pt(events):
    """The leading muon's pT."""
    return events.Muon_pt[:, 0]


def muon1_eta(events):
    """The leading muon's eta."""
    return events.Muon_eta[:, 0]


def muon2_pt(events):
    """The second muon's pT."""
    return events.Muon_pt[:, 1]


# ---- recipe ----
FILESET = {
    "DoubleMuParked 2012B+C": {
        "files": {
            "https://opendata.cern.ch/eos/opendata/cms/derived-data/AOD2NanoAODOutreachTool/Run2012BC_DoubleMuParked_Muons.root": {"tree": "Events", "stop": 800000},
        },
        "metadata": {},
    },
}

COLUMNS = {
    "mass": mass,
    "charge": charge,
    "Nmuon": "nMuon",
    "Muon1_pt": muon1_pt,
    "Muon1_eta": muon1_eta,
    "Muon2_pt": muon2_pt,
}

LABELS = {
    "charge": {-2: "mu- mu-", 0: "mu+ mu-", 2: "mu+ mu+"},
}

CART = Cart(
    fileset=FILESET,
    columns=COLUMNS,
    prepare=prepare,
    keep=keep,
    labels=LABELS,
    title="CMS Open Data dimuons",
    note="2012 DoubleMuParked (Runs B and C) from CMS Open Data, the first 800,000 events, those with two or more muons.",
)
