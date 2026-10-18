# rvr

The command for RowVer studies: start a study, say what it reads, see exactly what it found, and
look inside the `.rvr` files it makes.

```sh
rvr new DIR                 # start a study
rvr samples resolve …       # find every file the study reads
rvr validate / dump / diff  # check and inspect .rvr files
```

## Install

You need Rust 1.91 or later (`rustup update`).

```sh
cargo install --git https://gitlab.cern.ch/dgrove/rowver rvr
```

or, from a checkout of rowver, `cargo install --path crates/rvr`. Either puts `rvr` in
`~/.cargo/bin`. `rvr --version` says which `.rvr` version it reads and writes.

## Five minutes, no grid proxy

`rvr new` writes a study that works as it is: two CMS Open Data samples, which anyone can read,
and a muon and tau selection for them.

```text
$ rvr new higgs_tautau
Made higgs_tautau/ with
  samples.toml   what to read: two CMS Open Data samples, which need no grid proxy
  recipe.toml    what to keep and store: a muon and tau selection for them
  .gitignore     keeps pressed .rvr files out of git

$ cd higgs_tautau
$ rvr samples resolve samples.toml
  ok    GluGluToHToTauTau                          1 file    196.5 MB   all 476,963 events
  ok    VBF_HToTauTau                              1 file    231.8 MB   all 491,653 events

2 samples, 2 distinct files, 428.4 MB in 1.5 s.
The folders read also hold 8 .root files no sample reads (not an error; check none was meant):
  …
Wrote samples.lock (0.0 MB).

$ rvr samples plan samples.lock --events 50000
What a press would read (--events 50000), planned from the lock in 0.0 ms:
  GluGluToHToTauTau                                  50,000 of 476,963 events   1 of 1 files; stops at entry 50,000 of GluGluToHToTauTau.root
  VBF_HToTauTau                                      50,000 of 491,653 events   1 of 1 files; stops at entry 50,000 of VBF_HToTauTau.root
100,000 of 968,616 events in all.
```

Then press it. `rvr press` is not written yet; until it is, rapidity's Rust presser does the
pressing from the lock (build it once in a rapidity checkout with
`cargo build --release -p renorm-engine --bin renorm-build`):

```text
$ rvr samples fileset samples.lock --events 50000 > fileset.json
$ renorm-build --fileset fileset.json --definition recipe.toml --out higgs_tautau.rvr
…
{"event":"completed","path":"…/higgs_tautau.rvr","rows":2248,"columns":11}

$ rvr validate higgs_tautau.rvr
higgs_tautau.rvr: valid, 2248 rows, 11 columns
$ rvr dump higgs_tautau.rvr --columns
sample         i4[2248]  [0, 0, 0, 0, 0, ...]
entry          i8[2248]  [29, 45, 140, 157, 162, ...]
MET_pt         f4[2248]  [31.82324, 85.44, 16.09842, 19.80629, 77.1447, ...]
Muon_pt        jagged f4[2248]  [[44.166725], [20.321072], [60.48154], ...]
…
```

## A study folder

| file | what it is | written by | commit it? |
|---|---|---|---|
| `recipe.toml` | what to keep and store of every event | you | yes |
| `samples.toml` | what to read, in your words: datasets, folders, patterns, eras | you (or the Fileset Builder) | yes |
| `samples.lock` | every file found: where, bytes, adler32, ROOT UUID, entries, and what a press reads of it | `rvr samples resolve` | yes; review its changes like code |
| `STUDY.rvr` | the pressed events, with the texts above inside it | the presser | no (`.gitignore` leaves it out) |

## Commands

Every command prints what it did, or one sentence saying what is wrong and how to fix it. Exit
status: 0 when all went well, 1 when something failed or differs, 2 for a command it does not know.

### `rvr new DIR`

Writes `samples.toml`, `recipe.toml` and `.gitignore` into `DIR`, making the folder if needed.
It never writes over a file: if any of the three is already there it writes nothing and says so.
The recipe's `title` is the folder's name.

### `rvr samples expand samples.toml [--json]`

Lists the samples `samples.toml` describes, with each one's name, label, tree and what it reads,
after eras, mass points and placeholders are filled in. It asks no catalog or storage, so it is the
quick check while editing. `--json` prints them as JSON for other tools.

### `rvr samples resolve samples.toml`

Finds every file of every sample and writes `samples.lock` beside `samples.toml`. For each kind
of source it asks:

| source | written as | asked of |
|---|---|---|
| a central CMS dataset | `dataset = "/Primary/Processed/TIER"` | DBS (files, bytes, adler32, events) and Rucio (which sites hold them on disk) |
| storage: LPC EOS, CERN EOS, any grid site | `files = "root://host//path/file.root"` (or `davs://`, `https://`) | the storage's WebDAV door: one listing per folder, one checksum request per file, and three small reads of each file for its UUID and entries |
| this computer | `files = "~/data/…/file.root"` (or a path relative to `samples.toml`) | the file itself |

No event is read. The 307 files of a 353-sample study resolve in about 8 s. If any sample fails,
no lock is written and each failure is a line marked `FAIL` saying why, usually with the fix
("Did you mean …?", the trees the file does have). Under the totals it lists `.root` files in the
folders read that no sample reads: not an error, a check that none was forgotten.

### `rvr samples check samples.lock`

Asks again: every storage and local file's size and adler32, and the SHA-256 of the
`samples.toml` the lock came from. It says which files changed, or that `samples.toml` was edited
since, and exits 1 if anything did. A changed file is either a mistake or a new version; to
accept a new version, resolve again and review the new lock.

### `rvr samples plan samples.lock [--events N|all] [--random] [--seed S]`

Says what a press would read of each sample: how many entries, from how many files, and where it
stops. It plans from the lock alone, in under a millisecond, so it asks nothing of the network.
The options are the ones a press will take:

| | reads |
|---|---|
| (none) | what `samples.toml` asks of each sample |
| `--events 10000` | at most 10,000 of every sample, the first ones: a quick test |
| `--events all` | every sample whole, whatever `samples.toml` says |
| `--events 10000 --random` | 10,000 of every sample chosen at random, seed 1 |
| `--random` | each sample's own number, chosen at random |
| `--seed 8` | with `--random`: another reproducible choice |

The first entries come from one file; a random choice is spread over every file of a sample,
which avoids the bias of reading only the start (the earliest runs of data, one batch of MC) but
reads every file.

### `rvr samples fileset samples.lock [--events N|all]`

Prints the lock as a fileset JSON, `{sample: {"files": {path: {"tree", "stop"}}, "metadata"}}`,
for rapidity's `renorm-build` to press until `rvr press` exists. Each sample appears by its name,
with its label in its metadata, and each file read in part gets its `stop`. A random choice is
refused, since that presser reads the first entries only.

### `rvr validate FILE... [--parent PARENT]`

Checks each file completely: every array decompresses to its declared length and checksum, every
per-sample checksum, the column facts, the recipe snapshot and its checksums, and that the rows
belong to the files recorded. One line per file: `valid`, or the first problem found. A derived
file (columns computed from another file) is compared with its parent when you name it with
`--parent`.

### `rvr dump FILE [--fileset] [--cart] [--columns]`

Prints the inside of a file: the header in full, the recipe text, and every column with its kind,
length and first values. `--fileset` prints only the files that were read, `--cart` only the
recipe, `--columns` only the columns.

### `rvr header FILE`

Prints the JSON header as the file holds it, without reading any array.

### `rvr diff A B`

Says where two files differ from their headers alone, without decompressing anything: rows,
samples and columns only one has, columns whose kind changed, and each column of each sample
whose contents changed (every column carries one checksum per sample). Exit status 0 when the
headers show no difference, 1 when they do.

```text
$ rvr diff before.rvr after.rvr
column MET: differs in sample B
sample column xsec: differs in sample B: 2.5 in before.rvr, 3.5 in after.rvr
```

## samples.toml

```toml
format = "rvr-samples"
version = 1

[places]                      # short names for long locations, used as {name}
lpc = "root://cmseos.fnal.gov//store/user/username/ntuples"

[defaults]                    # every sample has these unless it says otherwise
tree = "KUAnalysis"

[eras.UL17]                   # values become metadata; {era} is the era's name
era = 2
[eras.UL18]
era = 3

[[sample]]
name = "ST_s-channel"         # makes ST_s-channel_UL17 and ST_s-channel_UL18
label = "ST s-channel"        # shown as "ST s-channel (UL17)"
files = "{lpc}/Summer20{era}_106X/ST_s-channel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20{era}_106X.root"
eras = ["UL17", "UL18"]
kind = "background"

[[sample]]
name = "Signal"               # one file per era holds a tree per mass point
files = "{lpc}/Summer20{era}_106X_SMS/SMS-Signal_TuneCP5_13TeV-madgraphMLM-pythia8_Summer20{era}_106X.root"
eras = ["UL17", "UL18"]
trees = { "300/250" = "SMS_30000_25000", "300/292.5" = "SMS_30000_29250" }
events = 20_000               # makes Signal_300_292p5_UL18, shown as "Signal 300/292.5 (UL18)"
choose = "random"
seed = 7
kind = "signal"

[[sample]]
name = "WZTo3LNu_UL18"
dataset = "/WZTo3LNu_TuneCP5_13TeV-amcatnloFXFX-pythia8/RunIISummer20UL18NanoAODv9-106X_upgrade2018_realistic_v16_L1v1-v2/NANOAODSIM"
tree = "Events"
events = 1_000_000
```

| key | meaning |
|---|---|
| `name` | required: what every tool calls the sample. Letters, digits, `_` and `-`; unique. |
| `label` | what plots show; any text; the name when left out. |
| `files` | paths or URLs, one or a list; `*` and `?` may appear in the file name |
| `dataset` | a CMS dataset, found through DBS and Rucio |
| `tree` / `trees` | the tree to read, or a table of label → tree making one sample per tree |
| `eras` | the eras this entry exists in, one sample each: `_ERA` added to the name, ` (ERA)` to the label |
| `events` | how many entries to read before any selection: a number, or `"all"` (the default) |
| `choose`, `seed` | `"first"` (the default) or `"random"`, reproducible from `seed` |
| anything else | metadata: it travels in the `.rvr`, and one value per sample becomes a sample column |

A mass point's label becomes part of the name as CMS dataset names write numbers: `.` becomes
`p`, anything else a name cannot hold becomes `_` (`300/292.5` → `300_292p5`).

## recipe.toml

The recipe is the definition language rapidity's Build node edits: TOML, with one expression per
value, evaluated over whole columns.

```toml
format = "renorm-definition"
version = 1

[object.good_muon]            # the muons that pass, as a collection of their own
from = "Muon"
where = "tightId and pfRelIso04_all < 0.15 and pt > 20"

[keep]                        # an event is kept when it passes every line
trigger = "HLT_IsoMu24"
one_muon = "count(good_muon) >= 1"

[copy]                        # branches stored as they are
branches = ["MET_pt", "Muon_pt", "Jet_pt"]
```

Also `[helper.NAME]` for named expressions you reuse, and `[column]` for computed columns
(allowed by rapidity's Derive; its Build presser stores `[copy]` only for now). Every operation
is listed in rapidity's `knowledge/build-definition.md`.

## Grid access

Central datasets, LPC EOS and most CERN EOS areas need a grid proxy; CERN Open Data does not.

```sh
voms-proxy-init -voms cms -valid 192:00
```

`rvr` uses `X509_USER_PROXY` when it is set, otherwise `/tmp/x509up_u<your uid>`. It trusts the
public certificate authorities and CERN's grid authorities, which FNAL and CERN EOS are signed by,
with no setup; `X509_CERT_DIR` adds a folder of others for sites signed by someone else. Without a
valid proxy, local files and public storage still resolve, and every other sample says what to run.

## When something goes wrong

| you see | what to do |
|---|---|
| `… matched no file (81 files in that folder). Did you mean: …?` | fix the file name or pattern; the suggestion is the nearest name in that folder |
| `… has no tree "Events". Its trees: …` | set `tree` (or `[defaults] tree`) to one of the trees listed |
| `{lcp} is not defined here. Defined: {lpc}.` | a misspelt place, or a place missing from `[places]` |
| `the dataset … has no valid files in DBS. Did you mean: …?` | the nearest datasets of that name follow; usually the version suffix |
| `… refused the listing (403). It needs a grid proxy: …` | run the command given |
| `a name holds only letters, digits, _ and - …` | use the name it suggests, and put the text in `label` |
| `1 file differs from the lock: …` (from `check`) | the file changed since the lock: resolve again if that is intended, and review the new lock |
| `samples.toml has changed since the lock was written` | resolve again |
| `A Build copies branches; computed columns belong to a Derive` (from `renorm-build`) | that presser stores `[copy]` only; move `[column]` entries to a derive of the pressed file, or wait for `rvr press` |

## Not built yet

- `rvr press`: until then, `rvr samples fileset` and rapidity's `renorm-build`, as above.
- `rvr reproduce FILE`: write a study back out of a `.rvr` and press it again.
- Writing `samples.toml`, the lock and each sample's label into the `.rvr` (`made.samples`,
  `made.lock`): a change to the file format, proposed for rowver 14.
- Lists of samples and folders from the Fileset Builder, which will write `samples.toml`.

MIT licensed.
