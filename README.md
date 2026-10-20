# RowVer

RowVer turns ROOT ntuples into one `.rvr` file: the events your selection keeps, the columns you
choose to store, and the whole study that made it. You describe a study in two small TOML files;
`rvr` finds every input, presses the events into one file, and checks it. The file carries its own
recipe, sample list and input lock, so anyone holding it can see exactly how it was made and press
it again.

- **One file, columnar.** Each column is compressed on its own, so a reader opens only what it needs.
- **Checked.** Every array has a SHA-256, and one per sample, so `rvr diff` can say which column of
  which sample changed between two files.
- **Pinned inputs.** A lock records each input's size, adler32, ROOT UUID and entries, and
  `rvr press` refuses an input that has changed.
- **Signed**, once you make a key: a reader can tell who pressed a file, and that it was not edited
  since.
- **A small selection language.** Cuts and computed columns are short expressions in
  `recipe.toml`, such as `count(good_muon) >= 1` or `sum(Jet_pt)`, evaluated over whole columns at
  once. The [recipe reference](crates/rvr-recipe/README.md) lists every operator and function, each
  with an example and its result.
- **Readable in Python** as NumPy, Awkward, pandas, Arrow or Parquet.

## What a `.rvr` file looks like

A file is four parts, one after another:

```
magic (8 bytes) | header length (4 bytes) | JSON header | compressed arrays
```

The first bytes of a real file, `higgs_tautau.rvr` from the tutorial below:

```text
00000000: 8952 5652 0d0a 1a0a ce32 0000 7b22 726f  .RVR.....2..{"ro
00000010: 7776 6572 223a 3134 2c22 6461 7461 223a  wver":14,"data":
00000020: 7b22 726f 7773 223a 3232 3438 2c22 7361  {"rows":2248,"sa
```

The header is plain JSON (`rvr header FILE` prints it). Here is that file's header, trimmed: `…`
marks what was cut, and checksums are shortened.

```json
{
  "rowver": 14,
  "data": {
    "rows": 2248,
    "samples": [
      {"name": "GluGluToHToTauTau", "label": "ggH to tau tau (CMS Open Data 2012)", "rows": 948},
      {"name": "VBF_HToTauTau", "label": "VBF H to tau tau (CMS Open Data 2012)", "rows": 1300}
    ],
    "files": [
      {"sample": "GluGluToHToTauTau", "path": "root://eospublic.cern.ch//eos/opendata/…/GluGluToHToTauTau.root", "rows": 948},
      …
    ],
    "sample": {"at": 0, "size": 115, "sha256": "61410385…"},
    "entry": {"at": 115, "size": 9250, "sha256": "8478fb51…", "by_sample": ["33c42609…", "7071e586…"]},
    "cols": [
      …
      {"name": "MET_pt", "kind": "f4", "at": 23103, "size": 8007, "sha256": "8fc683ad…",
       "by_sample": ["9a22d1fb…", "0921334d…"],
       "lim": [0.33867204189300537, 373.67779541015625], "nice": [0.33, 380.0]},
      …
      {"name": "Muon_pt", "kind": "jagged",
       "offsets": {"at": 39818, "size": 1595, "sha256": "a6bf8bca…", "by_sample": […]},
       "content": {"kind": "f4", "count": 2713, "at": 41413, "size": 9713, "sha256": "264ab757…", "by_sample": […]}},
      …
    ],
    "sha256": "f4e283d4…",
    "sample_cols": [
      {"name": "kind", "kind": "text", "values": ["signal", "signal"], "sha256": "fa9b4c00…"},
      …
    ]
  },
  "made": {
    "study": "higgs_tautau",
    "built": "2026-10-19T01:43:56Z",
    "by": "rvr 0.1.0",
    "toolchain": {"rvr": "0.1.0", "rowver": "14", "commit": "cc6ba5d2…", "rustc": "rustc 1.91.0 (f8297e351 2025-10-28)", "target": "aarch64-apple-darwin"},
    "recipe":  {"sha256": "43cbd29c…", "text": "…the whole recipe.toml…", "format": "rvr-recipe",
                "columns": ["run", "luminosityBlock", "event", "PV_npvs", "MET_pt", "MET_phi", "Muon_pt", "Muon_eta", "Tau_pt", "Tau_eta", "Jet_pt"]},
    "samples": {"sha256": "…", "text": "…the whole samples.toml…"},
    "lock":    {"sha256": "…", "text": "…the whole samples.lock…"},
    "files": {
      "GluGluToHToTauTau": {
        "events": {"choose": "first", "n": 50000},
        "files": {"root://eospublic.cern.ch//eos/opendata/…/GluGluToHToTauTau.root":
                  {"tree": "Events", "uuid": "b7e90ca6-bdad-11e9-a1c9-97650d81beef", "entries": 476963, "stop": 50000}},
        "metadata": {"kind": "signal", "events_read": 50000, "events_total": 476963}
      },
      …
    },
    "sha256": "2fcd85c2…"
  }
}
```

- **`data`** describes the rows. One row is one kept event, and each sample's rows are contiguous.
  Every row has a `sample` (its index into `samples`) and an `entry` (its entry number in its
  file's tree), so any event can be found again in the original ntuple. Each column record gives
  where its array lies (`at`, `size`), its type (`kind`: `f4`, `i8`, `b1`, …, or `jagged` for a
  list per event), its checksums, and its range.
- **`sample_cols`** hold one value per sample, such as a cross-section, taken from the samples'
  metadata.
- **`made`** is how the file was made: the three study files word for word, which events of each
  sample were read (all, the first N, or N at random from a seed), and every input file.
- A signed file also has a **`signature`**.

[FORMAT.md](FORMAT.md) specifies every field.

## Tutorial: from nothing to a file

This uses two Higgs-to-tau-tau samples from CMS Open Data, which anyone can read without a grid
proxy. Every output below is from a real run.

**1. Install `rvr`.** You need Rust 1.91 or later (`rustup update`).

```sh
cargo install --locked rvr
rvr --version
```

**2. Start a study.** This writes a `samples.toml` (what to read), a `recipe.toml` (what to keep
and store) and a `.gitignore`, ready to run as they are.

```text
$ rvr new higgs_tautau
Made higgs_tautau/ with
  samples.toml   what to read: two CMS Open Data samples, which need no grid proxy
  recipe.toml    what to keep and store: a muon and tau selection for them
  .gitignore     keeps pressed .rvr files out of git
…
$ cd higgs_tautau
```

`samples.toml` names each sample, its files and tree, and any metadata to carry along (here
`kind`):

```toml
[[sample]]
name = "GluGluToHToTauTau"
label = "ggH to tau tau (CMS Open Data 2012)"
files = "{opendata}/GluGluToHToTauTau.root"
kind = "signal"
```

`recipe.toml` says which events to keep and what to store of them:

```toml
[object.good_muon]
from = "Muon"
where = "tightId and pfRelIso04_all < 0.15 and pt > 20 and abs(eta) < 2.1"

[object.good_tau]
from = "Tau"
where = "idDecayMode and idIsoTight and idAntiMuTight and pt > 20 and abs(eta) < 2.3"

[keep]
trigger = "HLT_IsoMu17_eta2p1_LooseIsoPFTau20"
a_good_muon = "count(good_muon) >= 1"
a_good_tau = "count(good_tau) >= 1"

[copy]
branches = ["run", "luminosityBlock", "event", "PV_npvs", "MET_pt", "MET_phi", "Muon_pt", "Muon_eta", "Tau_pt", "Tau_eta", "Jet_pt"]
```

Each value in quotes is an expression. `[object.good_muon]` names the muons that pass its `where`
condition, and the `[keep]` cuts count them. What is stored is only what `[copy]` (and `[column]`)
name, so `Muon_pt` holds every muon of each kept event. The
[recipe reference](crates/rvr-recipe/README.md) explains every part, with examples.

**3. Find the inputs.** No event is read yet. This writes `samples.lock`, which pins every file.

```text
$ rvr samples resolve samples.toml
  ok    GluGluToHToTauTau                          1 file    196.5 MB   all 476,963 events
  ok    VBF_HToTauTau                              1 file    231.8 MB   all 491,653 events

2 samples, 2 distinct files, 428.4 MB in 1.7 s.
…
Wrote samples.lock (0.0 MB).
```

**4. Press.** `--events 50000` reads the first 50,000 events of each sample, for a quick first
look. Leave it out to read every event, or add `--random --seed 7` to choose them at random.

```text
$ rvr press --events 50000
Pressing 2 samples, 2 files, 100,000 entries, 2 at a time:
  1/2       50,000 entries        948 kept   GluGluToHToTauTau.root
  2/2       50,000 entries      1,300 kept   VBF_HToTauTau.root
Read 100,000 entries; kept after each cut:
  trigger                                8,590
  a_good_muon                            6,650
  a_good_tau                             2,248
Wrote higgs_tautau.rvr (2,248 rows, 11 columns, 0.3 MB) in 2.5 s.
```

**5. Check it, and look inside.**

```text
$ rvr validate higgs_tautau.rvr
higgs_tautau.rvr: valid, 2248 rows, 11 columns, not signed
$ rvr dump higgs_tautau.rvr --columns
sample         i4[2248]  [0, 0, 0, 0, 0, ...]
entry          i8[2248]  [29, 45, 140, 157, 162, ...]
run            i4[2248]  [1, 1, 1, 1, 1, ...]
luminosityBlock u4[2248]  [1987, 1987, 1988, 1988, 1988, ...]
event          u8[2248]  [198630, 198646, 198741, 198758, 198763, ...]
PV_npvs        i4[2248]  [10, 19, 13, 12, 21, ...]
MET_pt         f4[2248]  [31.82324, 85.44, 16.09842, 19.80629, 77.1447, ...]
MET_phi        f4[2248]  [0.73490125, 0.9452061, -2.9086378, 0.52976143, 1.8412126, ...]
Muon_pt        jagged f4[2248]  [[44.166725], [20.321072], [60.48154], ...]
…
```

**6. Read it in Python.** Build the package from a checkout of this repository with
`pip install ./crates/rowver-py`, then:

```text
>>> import rowver
>>> f = rowver.open("higgs_tautau.rvr")
>>> f
<rowver.File 'higgs_tautau.rvr': 2248 rows, 11 columns, 2 samples>
>>> f["MET_pt"][:5]
array([31.82324, 85.44   , 16.09842, 19.80629, 77.1447 ], dtype=float32)
>>> df = f.to_pandas(["MET_pt", "PV_npvs"])
>>> df.head(3)
              sample  entry     MET_pt  PV_npvs
0  GluGluToHToTauTau     29  31.823240       10
1  GluGluToHToTauTau     45  85.440002       19
2  GluGluToHToTauTau    140  16.098419       13
```

**7. Press it again from the file alone.**

```text
$ rvr reproduce higgs_tautau.rvr
Wrote higgs_tautau.reproduced/ with the recipe.toml, samples.toml and samples.lock higgs_tautau.rvr holds.
…
Comparing the two with rvr diff:
The headers show no difference in rows, samples, columns, by_sample or sample columns.
```

To use your own data, edit `samples.toml` and `recipe.toml`, then repeat steps 3 to 5. LPC EOS,
most CERN EOS areas and central datasets need a grid proxy first: `rvr proxy init` makes one
from your certificate in `~/.globus`. Run `rvr key new` once to sign every file you press.
The [recipe reference](crates/rvr-recipe/README.md) lists everything a recipe can say, each with an
example and its result, and the [rvr README](crates/rvr/README.md) describes every command and
every key of `samples.toml`.

## What is in this repository

| folder | what it is |
|---|---|
| [`crates/rvr`](crates/rvr/README.md) | the `rvr` command: start a study, resolve its samples, press them, and validate, dump, diff and reproduce `.rvr` files |
| [`crates/rvr-recipe`](crates/rvr-recipe/README.md) | the recipe language of `rvr press`: every table, operator and function, each with an example |
| [`crates/rowver-py`](crates/rowver-py/README.md) | the Python package `rowver`: `.rvr` files as NumPy, Awkward, Arrow, pandas and Parquet, and the `rvr` command |
| [`crates/rowver`](crates/rowver/README.md) | the Rust library that reads, validates and writes `.rvr` files; the reference builder of the format |
| [`FORMAT.md`](FORMAT.md) | the `.rvr` specification |

It reads ROOT with [hep-root](https://crates.io/crates/hep-root) and reaches the grid with
[voms-proxy](https://crates.io/crates/voms-proxy), [rucio-client](https://crates.io/crates/rucio-client)
and [cms-dbs](https://crates.io/crates/cms-dbs). MIT licensed.
