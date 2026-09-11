# Acceptance: the recipe snapshot beside its recipe

**2026-09-11 · plan step 9.4 · Derek judges; findings become a further step or close step 9.**

`studies/snapshot_acceptance/cart.py` is the recipe: two local UL18 NanoAOD
samples, a branch weight and a Python weight, `prepare` and `keep` hooks that
read metadata, a labelled integer column, and a jagged copy. It was pressed
with

```sh
ROWVER_SAMPLES=<directory holding the UL18 sample directories> \
    python -m rowver.cli run studies/snapshot_acceptance --no-live
# wrote studies/snapshot_acceptance/snapshot_acceptance.rvr — 45,147 rows, 1.1 MiB
```

and `studies/snapshot_acceptance/reconstruct.py` laid out the cartridge's
`made` drawer, and nothing else in it, in recipe form. `rvr.read` verified all
four digests on the way in. The layout is for reading beside the recipe, not
for running: `columns` and `labels` are not in `made`, and files are keyed by
ROOT UUID, not by path. The two texts follow, then the item-by-item pairing.

## The recipe as written

```python
"""Recipe-snapshot acceptance study, plan step 9.4.

Two local UL18 NanoAOD samples: TTJets DiLept weighted by the branch genWeight,
and WJetsToLNu weighted by a Python hook that scales genWeight by a k-factor
from the sample's metadata. PREPARE adds a lepton count, KEEP reads its MET
threshold from metadata, one integer column carries labels, and Jet_pt is a
jagged copy. ROWVER_SAMPLES names the directory holding the two UL18 sample
directories (default: ``samples`` beside this file). Press it, then run
reconstruct.py on the cartridge to lay the snapshot in ``made`` beside this file.
"""
import os

import numpy as np

from rowver import Cart

LOCAL = os.environ.get("ROWVER_SAMPLES", "samples")


def prepare(events):
    """Count the leptons once so KEEP and the columns share it."""
    events["nLep"] = events.nMuon + events.nElectron
    return events


def keep(events, metadata):
    """At least one jet and MET above the sample's threshold."""
    return (events.nJet >= 1) & (events.MET_pt > metadata["met_cut"])


def scaled(events, metadata):
    """The generator weight times the sample's k-factor."""
    return events.genWeight * metadata["k_factor"]


def channel(events):
    """0 hadronic, 1 one lepton, 2 two or more leptons."""
    return np.minimum(events.nLep, 2)


FILESET = {
    "TTJets DiLept": {
        "files": {
            f"{LOCAL}/TTJets_DiLept_TuneCP5_13TeV_2018UL/707E5366-52C7-C64C-BE69-2FDE6E109E71.root": "Events",
            f"{LOCAL}/TTJets_DiLept_TuneCP5_13TeV_2018UL/1CC920F3-1F58-3444-8551-618DCE2E12BE.root": "Events",
        },
        "metadata": {"kind": "mc", "xsec_pb": 87.3, "met_cut": 150.0, "weights": "genWeight"},
    },
    "WJets HT 400-600": {
        "files": {
            f"{LOCAL}/WJetsToLNu_TuneCP5_13TeV_2018UL/C17B82E7-01B7-9B4F-84F7-EA9703AC1CFB.root": "Events",
        },
        "metadata": {"kind": "mc", "xsec_pb": 48.9, "k_factor": 1.21, "met_cut": 200.0,
                     "weights": scaled},
    },
}


CART = Cart(
    fileset=FILESET,
    prepare=prepare,
    keep=keep,
    columns={
        "MET": "MET_pt",
        "MET_phi": "MET_phi",
        "Njet": "nJet",
        "Nlep": lambda events: events.nLep,
        "channel": channel,
        "Jet_pt": "Jet_pt",
    },
    labels={"channel": {0: "hadronic", 1: "one lepton", 2: "two or more leptons"}},
    title="Recipe snapshot acceptance",
    note="Two local UL18 NanoAOD samples, one branch weight and one Python weight.",
)
```

## The snapshot in recipe form

```python
"""The made drawer of snapshot_acceptance.rvr in recipe form; rvr.read verified every digest.

study_name: snapshot_acceptance
by: RowVer 9.0.0
built: 2026-09-11T22:11:19.834610+00:00
made.sha256: f6de2e0513e5a364e4e202bb0bc82c283ea31c2983972e48d9c0d8854ad8ddd3
made.cart.sha256: 0d5f977b2d314abeefc8b197c23363761d4e788a5c78fbda76e54790e02c6833
sha256(studies/snapshot_acceptance/cart.py): 0d5f977b2d314abeefc8b197c23363761d4e788a5c78fbda76e54790e02c6833
made.cart.source is studies/snapshot_acceptance/cart.py byte for byte: True
"""


def prepare(events):
    """Count the leptons once so KEEP and the columns share it."""
    events["nLep"] = events.nMuon + events.nElectron
    return events


def keep(events, metadata):
    """At least one jet and MET above the sample's threshold."""
    return (events.nJet >= 1) & (events.MET_pt > metadata["met_cut"])


def scaled(events, metadata):
    """The generator weight times the sample's k-factor."""
    return events.genWeight * metadata["k_factor"]


SAMPLES = {'TTJets DiLept': {'files': {'e81f4cfc-db44-11ec-a57b-48090694beef': {'file': '707E5366-52C7-C64C-BE69-2FDE6E109E71.root',
                                                                      'tree': 'Events',
                                                                      'entries': 219310},
                             'b1b95c3e-db3f-11ec-99f2-dba8935abeef': {'file': '1CC920F3-1F58-3444-8551-618DCE2E12BE.root',
                                                                      'tree': 'Events',
                                                                      'entries': 296691}},
                   'metadata': {'kind': 'mc',
                                'xsec_pb': 87.3,
                                'met_cut': 150.0,
                                'weights': 'genWeight'}},
 'WJets HT 400-600': {'files': {'e0be8d36-ec09-11eb-944c-00051cacbeef': {'file': 'C17B82E7-01B7-9B4F-84F7-EA9703AC1CFB.root',
                                                                         'tree': 'Events',
                                                                         'entries': 186773}},
                      'metadata': {'kind': 'mc',
                                   'xsec_pb': 48.9,
                                   'k_factor': 1.21,
                                   'met_cut': 200.0,
                                   'weights': scaled}}}

PREPARE = prepare
KEEP = keep
TITLE = 'Recipe snapshot acceptance'
NOTE = 'Two local UL18 NanoAOD samples, one branch weight and one Python weight.'
# columns and labels are not in made: data.cols[].source and data.cols[].names
# hold them, and made.cart.source has the declarations verbatim
```

## Item by item

| Declared in `cart.py` | Found in `made` | Same? |
|---|---|---|
| study directory `snapshot_acceptance` | `study_name` | yes |
| `title`, `note` | `title`, `note` | yes |
| TTJets: two paths under `LOCAL`, tree `Events` | two records keyed by ROOT UUID: bare file name, `tree`, `entries` 219310 and 296691 | yes; the directory is dropped (D19) |
| TTJets `weights: "genWeight"` | `{"kind": "branch", "value": "genWeight"}` | yes |
| TTJets metadata `kind`, `xsec_pb` 87.3, `met_cut` 150.0 | same keys, float lexemes kept (`150.0`, not `150`) | yes |
| WJets: one path under `LOCAL` | one UUID record, `entries` 186773 | yes |
| WJets `weights: scaled` | `{"kind": "python", "name": "scaled", "doc": …, "source": …}` | yes |
| WJets metadata with `k_factor` 1.21 | same, `weights` removed | yes |
| `prepare`, `keep` | `{"kind": "python", …}` with name, docstring, and source | yes |
| `columns` (six declarations) | absent by D17; `data.cols[].source` records what ran, and `made.cart.source` keeps the declarations verbatim | not in `made` |
| `labels` | absent by D9/D15; `data.cols[].names` carries them | not in `made` |
| `import os`, `import numpy`, `LOCAL` | only inside `made.cart.source` | text only |
| the file itself | `made.cart.source` byte for byte, `made.cart.sha256` equal to the file's SHA-256 | yes |

## What the press also showed

- **Finding, proposed as plan step 9.5.** `data.cols[].source` for `Nlep`
  is `{"class": "derived", "from": ["nElectron", "nMuon"], "reproducible":
  "source", "python": "\"Nlep\": lambda events: events.nLep,"}`, and
  `channel` has the same shape. The inputs follow `prepare` through the field
  it made, as designed, but the `python` text is only the column's own source
  and omits `prepare`, which is where `nLep` comes from, so the text cannot
  reproduce the column. A string column naming the same prepare-made field
  gets the writers' text instead (`tests/test_cart.py`, "a string column
  naming a field PREPARE made"): `Cart._sources` in `rowver/press.py` sets
  `origins = [spec] if callable(spec) else [writers…]`. FORMAT.md §3.4 says
  `python` is "the text of the hook or hooks that produced it". The one-line
  fix is to prepend the writers, in `_writer_order`, to a callable spec's own
  source whenever the column read a field a writer made. This is a
  `data.cols` matter, outside the `made` snapshot the step judges.
- `built` carries microseconds and `+00:00`, as
  `datetime.now(timezone.utc).isoformat()` writes it; FORMAT.md asks only
  for ISO 8601 UTC by convention.
- No column needed a `vals` warning: `Njet` has 23 distinct values, `Nlep`
  15, `channel` 3, all within the presser's 25.
- Gate: `python gate.py` 98 tests green, `sh wheel_gate.sh` green.

## Reproduce

```sh
export ROWVER_SAMPLES=<directory holding the UL18 sample directories>
python -m rowver.cli run studies/snapshot_acceptance --no-live
python studies/snapshot_acceptance/reconstruct.py \
    studies/snapshot_acceptance/snapshot_acceptance.rvr studies/snapshot_acceptance/cart.py
```

The press needs the three files, which are Derek's local copies; the
reconstruction runs on any cartridge.
