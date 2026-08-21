"""REMOTE EXAMPLE — a study whose files are not on this machine.

Only fileset.json differs from any other study: it names URLs instead of paths. These
are public CMS Open Data on CERN's EOS, so no certificate is needed.

    root://host//path/file.root      needs `fsspec-xrootd`
    https://host/path/file.root      needs `aiohttp` and `requests`

`fsspec-xrootd` is what uproot reads root:// through; the `xrootd` package alone gets
you the xrdcp command but not the URL. Both samples use root://, because https reads of
these files fail under the many small byte ranges a skim asks for.

Streaming re-reads the network every skim. To iterate on KEEP, copy them down once with
xrdcp and point fileset.json at the local files instead — measured here, 22s to copy and
then 0s a skim, against 15s on every run. For grid files see studies/grid_example/.
"""
import awkward as ak
import numpy as np

HAS_WEIGHTS = False
WEIGHTS_BRANCH = ""

PT_MIN = 5.0


def PREPARE(events: ak.Array) -> ak.Array:
    """Keep events with two muons, then sort every muon field by descending pT."""
    events = events[events.nMuon >= 2]
    order = ak.argsort(events.Muon_pt, axis=1, ascending=False)
    for f in ("Muon_pt", "Muon_eta", "Muon_phi", "Muon_mass", "Muon_charge"):
        events = ak.with_field(events, events[f][order], f)
    return events


def KEEP(events: ak.Array) -> ak.Array:
    """Both muons above threshold."""
    return (events.Muon_pt[:, 0] > PT_MIN) & (events.Muon_pt[:, 1] > PT_MIN)


COLUMNS = {
    "nMuon": "nMuon",
    "PT_mu_lead": lambda events: events.Muon_pt[:, 0],
    "PT_mu_sub": lambda events: events.Muon_pt[:, 1],
    "AbsEta_mu_lead": lambda events: abs(events.Muon_eta[:, 0]),
    "q_pair": lambda events: ak.values_astype(
        events.Muon_charge[:, 0] * events.Muon_charge[:, 1], np.int32),
}
