"""Copy this directory, then edit only this file.

Build it from the repository root with ``python run_rowver.py <directory-name>``.
"""
import awkward as ak

from cart import Cart


def keep(events, metadata):
    """Keep the inexpensive baseline; tune tighter cuts in the browser."""
    threshold = 200 if metadata["kind"] == "data" else 180
    return (events.nJet > 0) & (events.MET_pt > threshold)


CART = Cart(
    fileset={
        "Signal": {
            "files": {"signal/*.root": "Events"},
            "metadata": {"kind": "mc", "weights": "genWeight"},
        },
        "Background": {
            "files": {"background/*.root": "Events"},
            "metadata": {"kind": "mc", "weights": "genWeight"},
        },
        "Data": {
            "files": {"data/*.root": "Events"},
            "metadata": {"kind": "data", "weights": None},
        },
    },
    keep=keep,
    columns={
        "MET": "MET_pt",
        "nJet": "nJet",
        "PT_jet_lead": lambda events: events.Jet_pt[:, 0],
        "AbsEta_jet_lead": lambda events: abs(events.Jet_eta[:, 0]),
        "HT": lambda events: ak.sum(events.Jet_pt, axis=1),
    },
    title="My analysis",
)
