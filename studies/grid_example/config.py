"""GRID EXAMPLE — public CMS NanoAOD behind a redirector, which needs a certificate.

A redirector holds no data; it finds a site that does, so one path works from anywhere.
One file here goes through each:

    root://cms-xrd-global.cern.ch//store/data/...    CERN
    root://cmsxrootd.fnal.gov//store/data/...        Fermilab
    root://cmseos.fnal.gov//store/user/<you>/...     your own space, no redirector

Set both of these or every read fails:

    export X509_USER_PROXY=/tmp/x509up_u$(id -u)
    export X509_CERT_DIR=/path/to/certificates

The second is the one people forget, and the symptom misleads: `xrdfs stat` works while
uproot refuses the same path, because xrdfs finds the CA bundle itself and uproot has to
be told. Grid reads are slow — copy files down with xrdcp if you will iterate on KEEP.
"""
import awkward as ak

HAS_WEIGHTS = False
WEIGHTS_BRANCH = ""


def KEEP(events: ak.Array) -> ak.Array:
    """A jet and some missing energy — cheap, so this stays quick over the network."""
    return (events.nJet >= 1) & (events.PuppiMET_pt > 50.0)


COLUMNS = {
    "MET": "PuppiMET_pt",
    "nJet": "nJet",
    "nMuon": "nMuon",
    "PT_jet_lead": lambda events: events.Jet_pt[:, 0],
    "AbsEta_jet_lead": lambda events: abs(events.Jet_eta[:, 0]),
}
