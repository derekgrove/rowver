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
from cart import Cart

import awkward as ak



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

FILESET = {
    'JetMET Run2022C': {
        "files": {
            'root://cms-xrd-global.cern.ch//store/data/Run2022C/JetMET/NANOAOD/22Sep2023-v1/2530000/0b6815d2-5fd6-4b55-9374-a32b052c129b.root': 'Events',
        },
        "metadata": {},
    },
    'JetMET Run2022D': {
        "files": {
            'root://cmsxrootd.fnal.gov//store/data/Run2022D/JetMET/NANOAOD/22Sep2023-v1/2550000/03b788ce-72e2-4602-982a-7d042081ab89.root': 'Events',
        },
        "metadata": {},
    },
}


CART = Cart(
    name='grid_example',
    fileset=FILESET,
    columns=COLUMNS,
    keep=KEEP,
)
