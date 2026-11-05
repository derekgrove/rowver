# rvr

The command for RowVer studies: start a study, say what it reads, see exactly what it found, press
it into one `.rvr` file, and look inside the files it makes. What every command does, exactly, is
[the specification](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#5-the-commands-and-their-options); this page shows each
one at work.

```sh
rvr new DIR                 # start a study
rvr samples resolve …       # find every file the study reads, into samples.lock
rvr branches                # the branches a sample's file holds
rvr try "EXPRESSION"        # what an expression gives on the first events, before pressing
rvr press                   # read them through the recipe into one .rvr file
rvr status                  # is the .rvr current, and what would press read
rvr derive FILE …           # add, drop or rename derived columns of a pressed file
rvr validate / dump / diff  # check and inspect .rvr files
rvr reproduce FILE          # press a file's own study again and compare
rvr proxy init              # make a grid proxy from your certificate (for grid storage)
```

New to `.rvr` files? [The examples](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/examples/README.md) make a first one from CMS Open
Data and take it, step by step, to a plot.

## Install

```sh
pip install rowver
```

installs the Python package `rowver` and, with it, the `rvr` command (Python 3.11 or later;
[SPEC §11](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#11-the-python-package)). Or download `rvr` alone from the
[releases](https://gitlab.cern.ch/dgrove/rowver/-/releases): one file, no Rust needed. The Linux
one runs on any x86_64 Linux with glibc 2.28 or later (el8 and el9, so the LPC and lxplus; Ubuntu
20.04 on); the macOS one on Apple silicon and Intel. For example on the LPC:

```sh
mkdir -p ~/bin && cd ~/bin
curl -sL https://gitlab.cern.ch/api/v4/projects/dgrove%2Frowver/packages/generic/rvr/15.0.0/rvr-15.0.0-x86_64-linux.tar.gz | tar xz
~/bin/rvr --version     # add ~/bin to PATH in ~/.bashrc to type just rvr
```

(Downloaded with `curl`, the macOS binary opens as it is; one saved from a browser needs
`xattr -d com.apple.quarantine rvr` first.) `rvr --version` says which `.rvr` version it reads and
writes, and the commit it was built from. Building rvr yourself, from a checkout, is for
developers: [docs/SETUP.md](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SETUP.md).

## Five minutes, no grid proxy

`rvr new` writes a study that presses as it is: two CMS Open Data samples, read over HTTPS, which
need no grid proxy, and a muon and tau selection for them.

```text
$ rvr new higgs_tautau
Made higgs_tautau/ with
  samples.toml   what to read: two CMS Open Data samples, which need no grid proxy
  recipe.toml    what to keep and store: a muon and tau selection for them
  .gitignore     keeps pressed .rvr files out of git

Next:
  cd higgs_tautau
  rvr samples resolve samples.toml       find every file; writes samples.lock
  rvr samples plan samples.lock          what a press would read
  rvr branches                           the branches the files hold, as a recipe names them
  rvr try "count(good_muon)"             what an expression gives on the first events
  rvr press --events 50000               a quick first press; rvr press for every event
  rvr validate higgs_tautau.rvr          check the file it wrote

$ cd higgs_tautau
$ rvr samples resolve samples.toml
  ok    GluGluToHToTauTau                          1 file    196.5 MB   all 476,963 events
  ok    VBF_HToTauTau                              1 file    231.8 MB   all 491,653 events

2 samples, 2 distinct files, 428.4 MB in 1.6 s.
The folders read also hold 8 .root files no sample reads (not an error; check none was meant):
  root://eospublic.cern.ch//eos/opendata/cms/derived-data/AOD2NanoAODOutreachTool/DYJetsToLL.root
  root://eospublic.cern.ch//eos/opendata/cms/derived-data/AOD2NanoAODOutreachTool/Run2012BC_DoubleMuParked_Muons.root
  root://eospublic.cern.ch//eos/opendata/cms/derived-data/AOD2NanoAODOutreachTool/Run2012B_TauPlusX.root
  … and 5 more
Wrote samples.lock (0.0 MB).

$ rvr samples plan samples.lock --events 50000
What a press would read (--events 50000), planned from the lock in 0.0 ms:
  GluGluToHToTauTau                                  50,000 of 476,963 events   1 of 1 files; stops at entry 50,000 of GluGluToHToTauTau.root
  VBF_HToTauTau                                      50,000 of 491,653 events   1 of 1 files; stops at entry 50,000 of VBF_HToTauTau.root
100,000 of 968,616 events in all.
```

Then press it, here the first 50,000 events of each sample:

```text
$ rvr press --events 50000
Pressing 2 samples, 2 files, 100,000 entries, 2 at a time:
  1/2       50,000 entries        948 kept   GluGluToHToTauTau.root
  2/2       50,000 entries      1,300 kept   VBF_HToTauTau.root
Read 100,000 entries; kept after each cut:
  trigger                                8,590
  a_good_muon                            6,650
  a_good_tau                             2,248
Wrote higgs_tautau.rvr (2,248 rows, 11 columns, 0.3 MB) in 3.7 s.

$ rvr validate higgs_tautau.rvr
higgs_tautau.rvr: valid, 2,248 rows, 11 columns, read 100,000 of 968,616 entries, not signed
$ rvr dump higgs_tautau.rvr --columns
sample          i4[2248]  [0, 0, 0, 0, 0, ...]
entry           i8[2248]  [29, 45, 140, 157, 162, ...]
run             i4[2248]  [1, 1, 1, 1, 1, ...]
…
```

The file carries the study: `rvr reproduce` writes its recipe.toml, samples.toml and
samples.lock back out, presses them again and compares the two files.

```text
$ rvr reproduce higgs_tautau.rvr
Wrote higgs_tautau.reproduced/ with the recipe.toml, samples.toml and samples.lock higgs_tautau.rvr holds.
Pressing 2 samples, 2 files, 100,000 entries, 2 at a time:
  1/2       50,000 entries      1,300 kept   VBF_HToTauTau.root
  2/2       50,000 entries        948 kept   GluGluToHToTauTau.root
Read 100,000 entries; kept after each cut:
  trigger                                8,590
  a_good_muon                            6,650
  a_good_tau                             2,248
Wrote higgs_tautau.reproduced/higgs_tautau.rvr (2,248 rows, 11 columns, 0.3 MB) in 4.7 s.
Comparing the two with rvr diff:
The headers show no difference in rows, samples, labels, columns, derived columns, by_sample or sample columns.
```

## A study folder

| file | what it is | written by | commit it? |
|---|---|---|---|
| `recipe.toml` | what to keep and store of every event | you | yes |
| `samples.toml` | what to read, in your words: files, folders, patterns, eras | you, or any tool that writes it | yes |
| `samples.lock` | every file found: where, bytes, adler32, ROOT UUID, entries, and each sample's branch list | `rvr samples resolve` | yes; review its changes like code |
| `STUDY.rvr` | the pressed events, with the three texts above inside it | `rvr press` | no (`.gitignore` leaves it out) |

## Commands

| command | does | SPEC |
|---|---|---|
| `rvr new DIR` | writes a study that presses as it is | [5.3](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#53-rvr-new-dir) |
| `rvr samples expand FILE [--json]` | lists the samples `samples.toml` makes, asking nothing of any storage | [6](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#6-resolving-rvr-samples-and-sampleslock) |
| `rvr samples resolve FILE` | finds every file and writes `samples.lock` | [6](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#6-resolving-rvr-samples-and-sampleslock) |
| `rvr samples check LOCK` | asks again whether every file is as the lock recorded it | [6](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#6-resolving-rvr-samples-and-sampleslock) |
| `rvr samples plan LOCK [--events N\|all] [--random] [--seed S]` | what a press would read, from the lock alone | [6](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#6-resolving-rvr-samples-and-sampleslock) |
| `rvr branches [DIR] [--sample NAME] [--find TEXT]` | the branches of a sample's first file | [5.4](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#54-rvr-branches) |
| `rvr try "EXPRESSION" [DIR] [--events N] [--sample NAME] [--kept]` | an expression's values on a sample's first events | [5.5](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#55-rvr-try) |
| `rvr press [DIR] [--events N\|all] [--random] [--seed S] [--out FILE] [--workers N] [--full] [--fill [KINDS]] [--restart]` | presses the study | [7](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#7-the-pressing-core), [8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file) |
| `rvr status [DIR] [--out FILE] [--events N\|all] [--random] [--seed S] [--holes]` | whether the file holds the study as it is, and what a press would read | [9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files) |
| `rvr derive FILE 'NAME = EXPRESSION'… \| --drop NAME… \| --rename OLD NEW` | adds, drops or renames derived columns of a file | [8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file) |
| `rvr validate FILE… [--complete]` | checks files in full | [9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files) |
| `rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns] [--all]` | the inside of a file as text | [9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files) |
| `rvr header FILE` | the header as the file holds it | [9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files) |
| `rvr diff A B [--rows] [--key COLUMN,…] [--sample NAME]` | where two files differ: from their headers, or (`--rows`) event by event | [9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files) |
| `rvr reproduce FILE [DIR] [--study FOLDER] [--workers N]` | presses a file's own study again and compares | [9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files) |
| `rvr empty FILE… [--study FOLDER] [--anyway] [--force \| -f]` | lets a file's arrays go, keeping its header | [8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file) |
| `rvr refill FILE [--study FOLDER] [--workers N]` | presses an empty file's arrays again | [8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file) |
| `rvr key new [NAME]`, `rvr key show`, `rvr key trust KEY NAME…` | the key that signs your files, and the keys you trust | [10](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#10-keys-and-proxies) |
| `rvr proxy`, `rvr proxy init [--hours H]` | the grid proxy, shown or made | [10](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#10-keys-and-proxies) |
| `rvr --version` | `rvr VERSION (reads and writes rowver 15; commit COMMIT)` | [12](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#12-builds-and-releases) |
| `rvr`, `rvr help`, `rvr --help` | the usage, every command and flag | |

A command line is `rvr COMMAND` with its words and flags in any order (`rvr --holes status` is
`rvr status --holes`). A flag's value is the argument after it (`--events 10`; `--events=10` is not
read), and a count is a whole number from 1, with `_` allowed between two digits
(`--events 50_000`). How a line is read, exactly, is
[SPEC §5.1](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#51-every-command). What a command reports goes to standard output;
the line that ends a failing command goes to standard error, as
`rvr COMMAND: what is wrong, and how to fix it`. Wrong use does nothing, says what is wrong with the
nearest command or flag, and prints the usage:

```text
$ rvr prss
rvr: prss is not a command. Did you mean: press?
usage:
…
$ rvr press --evnts 5
rvr press: --evnts is not one of its flags. Did you mean: --events?
usage:
…
```

| exit status | means |
|---|---|
| 0 | done, or, for `status`, `diff`, `samples check` and `proxy`, the answer is yes |
| 1 | failed, or the answer is no (a file that differs, a study not current, a file invalid) |
| 2 | wrong use, an unknown command, or a file `rvr diff` cannot read |
| 3 | `rvr press` wrote, or found, a file with holes |
| 141 | the output pipe closed before the command finished, as a C tool stopped by SIGPIPE |

rvr asks a question (`[y/N]`) only when standard input and standard error are both terminals.

### `rvr new DIR`

Writes `samples.toml`, `recipe.toml` and `.gitignore` into `DIR`, making the folder if needed
(above). The recipe's `title` is the folder's own name, the one a press names the file after (for
`rvr new .`, the current folder's name). It never writes over a file: when any of the three is
already there, it writes nothing and says which.

```text
$ rvr new higgs_tautau
rvr new: higgs_tautau already has samples.toml and recipe.toml and .gitignore; rvr new writes nothing over a file. Use another folder, or move those away.
```

### `rvr samples expand samples.toml [--json]`

Lists the samples `samples.toml` makes (each one's name, label, tree, event choice and the files it
asks for, after eras, mass points and placeholders are filled in), asking no storage: the quick
check while editing. `--json` prints them as JSON, each with its metadata too. Every problem in the
file is listed at once:

```text
$ rvr samples expand samples.toml
rvr samples expand: samples.toml: sample "my sample" has no key "treee"; it takes name, label, files, tree, trees, eras, events, choose, seed, metadata. Did you mean: tree, trees? Metadata goes in its own table: metadata = { treee = … }.
sample "my sample": a name holds only ASCII letters, digits, _ and -, since tools use it in commands and file names: use name = "my_sample", and put the text in label
```

### `rvr samples resolve samples.toml`

Reads `samples.toml` and the `recipe.toml` beside it, finds every file of every sample, checks the
recipe against what it found as a press would, and writes `samples.lock` beside `samples.toml`
only when it fits: for each file its
size, adler32 (when its storage gives one), ROOT UUID and entries, and for each sample the branch
list of its first file, which a press checks every file against. No event is read. A file on this
computer is read whole, for its adler32; a file on storage is asked of its HTTPS door (a listing per
folder, a `HEAD` per file, and the bytes that open it). What it asks, exactly, is
[SPEC §6.2](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#62-rvr-samples-resolve-file).

It is all or nothing: when a sample fails, or the recipe does not fit the samples (a branch no
sample has, a defined name that is a branch's, a column of two kinds), no lock is written and the
lock already there stays as it was. Each failure is a line marked `FAIL`, with the fix:

```text
$ rvr samples resolve samples.toml
  FAIL  flat: ./flat_a.root has no tree "Events". Its trees: analysis, other, ntuple/Events, deep/a/b/t.

0 samples, 0 distinct files, 0.0 MB in 0.0 s.
rvr samples resolve: 1 of 1 samples could not be resolved; samples.lock was not written. Fix the lines marked FAIL and run resolve again.
```

```text
$ rvr samples resolve samples.toml
  ok    flat                                       1 file      0.0 MB   all 5 events

1 sample, 1 distinct file, 0.0 MB in 0.0 s.
  FAIL  [columns] Jet_ptt: There is no column named Jet_ptt. Did you mean: Jet_pt, Jet_eta? (sample flat, over the branches samples.lock lists for it)
rvr samples resolve: the recipe does not fit the samples as the lines marked FAIL say, so samples.lock was not written (a press would refuse them): fix them, then run resolve again.
```

A study with no `recipe.toml` is refused (`rvr new` writes one); a recipe that leaves out a table is
offered it at a terminal, as `rvr press` offers it. A lock exists only when the study's recipe can be
pressed from it; a press checks again, as the recipe may change after resolve.

Under the totals it lists the `.root` files in the folders it read that no sample reads: not an
error, a check that none was forgotten. A `samples.toml` that leaves out `format`, `version`,
`[places]`, `[defaults]` or `[eras]` is refused; at a terminal, resolve offers to write them in
([SPEC §3.1](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#31-the-document)).

### `rvr samples check samples.lock`

Asks again: every file's size and adler32 (on storage, a `HEAD`), and whether the `samples.toml`
beside the lock is the one it was resolved from. Exit status 0 when everything is as the lock
recorded it, else 1. A file that differs is one a press would make a `changed` hole of; to take a
new version of it, resolve again and review the new lock.

```text
$ rvr samples check samples.lock
1 file differs from the lock:
  ./flat_b.root: it was 39262 bytes, adler32 5ece3801; it is now 39262 bytes, adler32 bbc43743
A press from this lock makes a changed (or gone) hole of each; if the change is wanted, run rvr samples resolve again and review the new lock.
```

### `rvr samples plan samples.lock [--events N|all] [--random] [--seed S]`

Says what a press would read of each sample (entries, files, and where the first N stop), from the
lock alone, asking nothing of any storage. Its options are the press's:

| options | every sample reads |
|---|---|
| none | its own choice, from `samples.toml` |
| `--events N` | its first N entries |
| `--events all` | every entry |
| `--events N --random` | N chosen at random, seed 1 |
| `--events N --random --seed S` | N chosen at random, seed S |
| `--random` | its own N, chosen at random: a sample already random with its own seed, any other with seed 1; refused when a sample reads every entry |
| `--seed S` | its own choice, with seed S for a random one; refused when no sample is random |

`--events all --random` is refused ([SPEC §7.1](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#71-the-choice-of-events)). The
first entries come from the first files; a random choice is spread over every file of a sample,
which avoids the bias of reading only the start (the earliest runs of data, one batch of MC) but
opens every file.

```text
$ rvr samples plan samples.lock --events 1000 --random --seed 8
What a press would read (--events 1000 --random --seed 8), planned from the lock in 0.8 ms:
  GluGluToHToTauTau                          1,000 of 476,963 events (random)   1 of 1 files
  VBF_HToTauTau                              1,000 of 491,653 events (random)   1 of 1 files
2,000 of 968,616 events in all.
```

### `rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N] [--full] [--fill [KINDS]] [--restart]`

Presses the study in `DIR` (the current folder when not given): reads what `samples.lock` names,
keeps the events that pass every cut of `recipe.toml`, and writes `DIR/<DIR's name>.rvr`, or
`--out FILE`. `--events`, `--random` and `--seed` are `plan`'s (above). `--workers` is how many
files are read at once: the number of cores when not given, and 1 when the system says none.
`samples.toml` must be beside the lock, since the file carries both.

Before any file is opened, each input goes through its validator (`samples.toml`, `recipe.toml`,
`samples.lock`, the options, and the `.rvr` already at the target), and then through one check of
how they fit together; a study that does not pass is refused with every problem at once, one a
line. What passes becomes one Plan, which the pressing core runs: from there a press fails only on
what the files hold. How a press runs, and the few ways the core can fail, is
[SPEC §2](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#2-how-a-press-runs).

```text
$ rvr press
rvr press: recipe.toml: A recipe has no [totals] table: every recipe has its five tables, [define], [keep], [columns], [totals] and [derived], each written even when it is empty. Add the line [totals].
```

(At a terminal, `rvr press` offers to write the missing tables in and goes on:
[SPEC §4.1](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#41-the-document).)

#### Building on the file there

When `rvr press` finds a file at its target made from the same study, it reads only what is new
([SPEC §8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file)); `rvr status` says beforehand what it would
read, and `--full` always presses everything.

**A current file.** When the file holds the study as it is, `rvr press` leaves it byte for byte and
exits 0; with holes, it says so and exits 3.

```text
$ rvr press --events 50000
higgs_tautau.rvr is current: nothing to press.
```

**A column you forgot.** Add it to `recipe.toml` and press again. When the file was made by this
build from the same `samples.toml` and lock, with the same cuts, totals and event choices, only the
columns new to the recipe, or whose definition changed (their expression, their stored kind, or a
defined name they use), are read, for the events the file kept; every other column is copied from
the file. The file written is the one a full press writes.

```text
$ rvr press --events 50000
higgs_tautau.rvr has the events this recipe keeps (2,248) and 11 of its 12 columns.
Reading only Jet_eta for those events, 2 files, 2 at a time:
  1/2       50,000 entries        948 kept   GluGluToHToTauTau.root
  2/2       50,000 entries      1,300 kept   VBF_HToTauTau.root
Wrote higgs_tautau.rvr (2,248 rows, 12 columns, 0.3 MB) in 2.4 s.
```

On local copies of the same two files, every event, a press that reads only `Jet_pt` read at most
26.3 MB of baskets (`Jet_pt` and `nJet`) where a full press reads 108.0 MB, in 0.12 s instead of
0.51 s (Apple M5, 2026-11-03). While building on columns, a file that cannot be read stops the
press, as a hole would change the rows the columns are built on; `rvr press --full` presses
everything and makes holes.

**A sample you added.** Add it to `samples.toml`, resolve, and press again. When the file was made
by this build with the same cuts, totals and columns, and every sample it holds is in the lock as
it was, only the new samples are read. Every column is packed again, in the lock's order of
samples; a sample it holds keeps its holes.

```text
$ rvr press
flat.rvr holds the rows of 1 of the 2 samples as samples.lock has them.
Reading only the new sample c, 1 file, 5 entries, 1 at a time:
  1/1            5 entries          5 kept   flat_c.root
Read 5 entries of the new sample c; with no cuts, every one is kept.
Wrote flat.rvr (15 rows, 4 columns and 2 derived, 0.0 MB) in 0.0 s.
```

On 2.1 M rows (ten samples of 30 links to the TChiWZ skim), adding an eleventh of 30 files took
2.4 s where a full press takes 3.9 s; taking one out took 2.1 s against 3.5 s, all of it the
packing again (medians of 9, Apple M5, 2026-11-03).

**Nothing to read.** When only `[derived]` changed, or columns were only dropped, no input is
opened: the stored columns are copied from the file, and the derived ones worked out from them.

```text
$ rvr press
flat.rvr has the events this recipe keeps (10) and 4 of its 4 columns.
Nothing to read: every column is copied from it.
Wrote flat.rvr (10 rows, 4 columns and 2 derived, 0.0 MB) in 0.0 s.
```

**Anything else** (other cuts, totals or event choices, a sample whose files changed, a column
changed together with the samples, another build of rvr) presses every column, and says why:

```text
$ rvr press
Pressing every column: flat.rvr cannot be built on, as the cuts under [keep], or what they use, have changed.
Pressing 2 samples, 3 files, 15 entries, 3 at a time:
…
```

An rvr whose commit is `unknown` or ends in `-dirty`
([SPEC §12](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#12-builds-and-releases)) never builds on a file and keeps no
journal: it presses whole files only.

#### A value the recipe cannot take

A conversion out of the target kind's range, a mask or two lists whose lengths differ in one
event, an event past the pair limit, a whole number to a negative power, or a total that is not
finite stops the press, naming the file, its entry, the column and the value; the press keeps what
it read. It counts only in an event the choice chose, and for a stored column only in a kept one
([SPEC §4.2](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#42-what-the-core-evaluates)). Floats may be NaN or infinite
anywhere.

```text
$ rvr press
Pressing 1 sample, 1 file, 5 entries, 1 at a time:
rvr press: ./uproot_flat.root, tree analysis, entry 3: [columns] x: inf does not fit in u1, which holds 0 to 255. The recipe cannot take this value: mend the recipe (or the file), then press again. err.rvr was not written.
```

A file that lacks the tree a `[totals]` total over another tree names, or a branch that total reads
there, stops the press the same way, naming the file, the tree and the branch.

#### Derived columns

A column under `[derived]` is kept in the file as its expression, not as an array, and a reader
(`rvr dump`, the Python package) works it out from the file's columns when it is asked for. Its
expression uses the stored columns, the derived columns above it, and `sample.KEY`: the sample's
metadata with its totals and `file`, without `events_read`, `events_total` and `events_missing`.
The press works each one out on the first run of each sample that keeps an event, so that a value
it cannot take stops the press there, and once more over the whole file, to record its kind,
checksums and facts; one that fails then leaves the file written without its derived columns, and
the press says so and exits 1. The rules are
[SPEC §4.6](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#46-derived-columns) and
[FORMAT §10](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md#10-derived-columns); `rvr derive` changes them in a pressed file
alone.

Which columns to store and which to derive is a choice of space against time. On 2.1 M rows (300
copies of the TChiWZ skim, Apple M5, 2026-11-03), seven typical computed columns (a sum, a count,
the leading lepton's pt, a ratio, a transverse mass, the largest jet-pair mass, jets cleaned of
leptons) take 70 bytes per row stored, uncompressed, 35% more than the ten columns they use;
derived, they add 0.6 s to the press (4.0 s against 3.4 s), and a Python run that works all seven
out takes 1.0 s, where one that reads them stored takes 0.10 s and one that only opens the file
0.05 s. A simple one, `sum(PT_jet)`, takes 0.11 s.

#### While it reads

On a terminal, a press shows one bar per sample under way, with the entries read of those it chose,
the events kept so far, the read rate and the time, redrawn every 200 ms and cut to the terminal's
width; what happens meanwhile (a wait, a hole) is printed above the bars. Elsewhere (a log, a batch
job) it prints one line per file as it finishes, with its holes, and all the holes again once the
file is written, in the lock's order
([SPEC §7.7](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#77-what-a-press-shows)).

```text
$ rvr press
Pressing 2 samples, 2 files, 968,616 entries, 2 at a time:

  GluGluToHToTauTau  ██████░░░░░░░░░░░░░░  31%  150,000 / 476,963
  VBF_HToTauTau      ████░░░░░░░░░░░░░░░░  20%  100,000 / 491,653

  5,547 kept so far · 5 MB/s · 8.1 s
```

Drawing costs no more than the run-to-run spread: 0.199 s against 0.195 s without it on the TChiWZ
skim (30 files, on a terminal), 2.064 s against 2.068 s for 90 files with `--workers 1` (Apple M5,
2026-11-03).

#### Holes

A file the press cannot read is not fatal: the file is written without those entries, and records
a **hole** of them, with its kind and why
([SPEC §7.3](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#73-holes), [FORMAT §6.2](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md#62-holes)):

| kind | the press found |
|---|---|
| `unreachable` | its storage kept failing after every wait, or answered in a way no wait mends |
| `damaged` | a basket that does not decompress, or another error in the file's bytes |
| `gone` | the file is not there any more |
| `changed` | the file is not the one the lock pinned |
| `refused` | its storage refuses it, with a valid grid proxy |

A damaged basket makes a hole of the entries of the run read with it, and the rest of the file is
read; every other kind makes a hole of every entry the file's choice has left from where it
happened. What is not one file's own failure (the network gone, a proxy or certificate refused, a
disk not mounted or full, a permission refused) makes no hole: it stops the press, which keeps what
it read. rvr writes a run of entries as `entries 0 to 4`, both ends among them (`entry 7` for
one); the file, and the Python package's `holes`, record that run as `[0, 5]`. The press exits 3
when the file has holes:

```text
$ rvr press
Pressing 1 sample, 2 files, 10 entries, 2 at a time:
  1/2            0 entries          0 kept   flat_b.root
    hole (damaged): ./flat_b.root, tree analysis, entries 0 to 4: weight: decompression failed (zlib): corrupt deflate stream, in basket 0 of branch weight (entries 0 to 4, 111 bytes at byte 662). The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.
  2/2            5 entries          5 kept   flat_a.root
Read 5 of 10 entries; with no cuts, every one is kept.
Wrote holes.rvr (5 rows, 2 columns, 0.0 MB) in 0.0 s, with 1 hole:
  damaged     flat  ./flat_b.root, tree analysis, entries 0 to 4: weight: decompression failed (zlib): corrupt deflate stream, in basket 0 of branch weight (entries 0 to 4, 111 bytes at byte 662). The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.
rvr press --fill reads them again once they can be read.
$ rvr press
holes.rvr holds this study as it is, with 1 hole: rvr press --fill reads them again (rvr press --full presses everything again).
```

A file with holes adds `events_missing` (the entries chosen that lie in holes) to every sample's
metadata, beside `events_read`. `rvr status` shows what each sample is missing, by kind;
`rvr validate` says how many holes, of which kinds, and `rvr validate --complete` fails a file with
holes, for scripts.

**`rvr press --fill [KINDS]`** reads only the holes of the file at the target, and writes the file
again with their rows in place, merged by entry within each file; `--fill damaged,unreachable`
reads only holes of those kinds (how the word after `--fill` is read:
[SPEC §5.1](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#51-every-command)). It presses the recipe the file carries, with its
event choices; the lock may have been resolved again for the files it fills (one put back, or
another copy in its place). A hole that fails again stays a hole, with its new kind and why; when
none can be read, the file is left as it was. On the TChiWZ skim (30 files, 212,130 entries) with
one file's hole, a fill takes 0.09 s where a full press takes 0.18 s (0.24 s and 0.69 s with
`--workers 1`; Apple M5, 2026-11-03).

```text
$ rvr press --fill
Filling holes.rvr: reading 1 hole of 1 file, 5 entries, 1 at a time; every other row is the file's own:
  1/1            5 entries          5 kept   flat_b.root
Read 5 entries in the holes; with no cuts, every one is kept.
Wrote holes.rvr (10 rows, 2 columns, 0.0 MB) in 0.0 s.
```

#### Stopped part way

A press is staged in `FILE.partial/` beside its target, with a journal of the files it has
finished; nothing is put at the target until every file is read or made a hole, so the file
appears whole, or not at all. A press stopped by anything (Ctrl-C, a killed process, a laptop
asleep, a failure that is not a file's own, a value the recipe cannot take) loses at most the files
finished since its last checkpoint (at most every 2 seconds while files finish), and the next press
with the same Plan carries on, and says so. A file stopped half read is read again from its start,
and so is one that became an `unreachable` or `refused` hole; a `damaged`, `gone` or `changed` hole
stays. A press with another Plan starts over and says what differs; `--restart` always starts over.
While a press runs it holds a lock on its journal, and another press of the same target, and
`rvr empty` or `rvr refill` of it, is refused
([SPEC §7.5](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#75-stopping-and-carrying-on)).

```text
$ rvr press --workers 1
Pressing 2 samples, 2 files, 968,616 entries, 1 at a time:
  1/2      476,963 entries      9,362 kept   GluGluToHToTauTau.root
^C
$ rvr status
higgs_tautau.rvr.partial: a press stopped there with 1 of 2 files done (journal from 2026-11-03T20:48:22Z); rvr press carries it on, reading the 1 file left, 491,653 entries.
higgs_tautau.rvr is not there yet: rvr press makes it.
$ rvr press --workers 1
Picking up higgs_tautau.rvr where it stopped: 1 of 2 files done (journal from 2026-11-03T20:48:22Z).
Pressing 2 samples, 2 files, 968,616 entries, 1 at a time:
  2/2      491,653 entries     12,963 kept   VBF_HToTauTau.root
Read 968,616 entries; kept after each cut:
  trigger                               82,629
  a_good_muon                           64,771
  a_good_tau                            22,325
Signed with your key 013f1fa24edd009f…
Wrote higgs_tautau.rvr (22,325 rows, 11 columns, 2.4 MB) in 19.4 s.
```

The file picked up is the one an uninterrupted press writes (`rvr diff` finds no difference).
Keeping the journal costs at most 1%: on the TChiWZ skim, 0.185 s against 0.188 s without it (30
files, default workers), 2.057 s with and without it (90 files, `--workers 1`), and 3.554 s against
3.524 s for the 39.3 MB file of 2.1 M rows, synced before it is put in place; Apple M5, 2026-11-03.

What a press holds in memory does not grow with the events it keeps: each file's kept values go to
a spool as they are read, and each column is packed from it a piece at a time, so a press needs free
disk for the spool beside the file it writes: 461 MB beside the 39.3 MB file of 2.1 M rows, whose
300 copies of one file compress far better whole than a piece at a time ([SPEC §7.6](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#76-what-a-press-holds)).
Pressing 12 million rows of 31 columns from 1,698 files (4 workers) peaks at 0.14 GB of memory, and
3 million rows from 425 files at 0.10 GB (Apple M5, 2026-11-03).

The file carries the whole study (the three texts, every file read with its identity and how much
of it was read, each sample's metadata with `events_read` and `events_total`, the totals, and what
built rvr), and is signed when you have a key
([SPEC §7.8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#78-what-a-press-writes)).

#### Files on storage

A remote file is read over HTTPS from its door: a `root://` name through that host's HTTPS door,
from the WLCG door list built into rvr; a `davs://` or `https://` name as written
([SPEC §3.8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#38-where-files-are)). Only the branches the recipe uses are read,
the baskets of each run asked for together in `multipart/byteranges` requests
([SPEC §7.2](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#72-reading)). A random choice reads only the runs that hold a chosen
entry, and opens no file with none chosen.

A storage under load refuses requests for minutes at a time (EOS answers `403`; others `429` or
`503`), and the network drops some. So a read that fails that way is asked again after 5, 15, 30,
60, 120, 240 and 300 seconds, eight tries over 770 s, and each wait is printed as
`  waiting W s to ask again for FILE (K of 8 tries made): WHY`. A read whose eight tries fail makes
an `unreachable` hole, or a `refused` one when the last answer was `403`. A `404` is a `gone` hole at
once; a `401`, a certificate rvr does not trust, or a `403` with no proxy stops the press, as every
file there would fail the same ([SPEC §7.4](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#74-when-a-storage-fails)).

A press that loses the network stops and keeps what it read, instead of making a hole of every
file it could not reach. While a read waits, once no storage has answered anything for 120 s, rvr
tries for 5 s to connect to each door that answered earlier in the press and is not failing now
(or, when every one is failing, to every door that answered). One connection shows the network
works: only the failing storage is down, and its files become holes after their waits. No
connection, twice 15 s or more apart, means the network is gone: the press stops, keeps what it
read, and says so; pressing again once it is back carries on. When every door that answered is at
one site, rvr cannot tell the site down from the network gone, and stops and keeps what it read,
saying why.

### `rvr status [DIR] [--out FILE] [--events N|all] [--random] [--seed S] [--holes]`

Says, from the study's three texts and the file's header alone (no input is read), whether the
`.rvr` file `rvr press` with the same options would write holds the study as it is: what changed in
each text, what a press would do, how much each sample holds of what it chose, and its holes by
kind; `--holes` lists every hole. It shows a press stopped part way (above), one stopped before it
finished a file (which `rvr press` starts over), or one under way. Exit
status 0 only when `rvr press` with the same options would have nothing to do: the file current and
without holes, so a script can ask before pressing. Whether the input files changed since the lock
is `rvr samples check`.

```text
$ rvr status
flat.rvr: pressed 2026-11-03T19:42:13Z by rvr 15.0.0
  recipe.toml   changed since: new: ht
  samples.toml  as the file holds it
  samples.lock  as the file holds it
  events        as the file holds it
rvr press would read only ht, for the events the file kept, and copy every other column from it.

  flat  ████████████████████████ 100%   2 files
```

A file with a hole (the one under Holes, above):

```text
$ rvr status
holes.rvr: pressed 2026-11-03T19:43:27Z by rvr 15.0.0
  recipe.toml   as the file holds it
  samples.toml  as the file holds it
  samples.lock  as the file holds it
  events        as the file holds it
It holds the study as it is, but for its holes: rvr press --fill reads them.

  flat  ████████████░░░░░░░░░░░░  50%   2 files, 1 hole

Holes, by why:
  damaged      1 file, 5 entries   flat
      ./flat_b.root, tree analysis, entries 0 to 4: weight: decompression failed (zlib): corrupt deflate stream, in basket 0 of branch weight (entries 0 to 4, 111 bytes at byte 662). The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.
(rvr status --holes lists every hole.)
rvr press --fill would read 5 entries of 1 file.
```

### `rvr branches [DIR] [--sample NAME] [--find TEXT]`

Lists every branch of the first file of the study's first sample (or of `--sample NAME`), from the
sample's branch list in the lock: no file is opened. Before the study has a lock (and before it has
a recipe), it finds that one file through `samples.toml`, as resolve finds files, opens it and reads
its branches from the file itself, asking no checksum and writing nothing: list the branches first, then write the recipe and resolve.
One line per branch, ordered by name ignoring
case: its name as a recipe writes it, its kind, and its shape (`flat`, `jagged`, `N per event`, or
`jagged, N per element`). Branches rvr cannot read (whole objects, strings, nested vectors) follow
under `Not readable by rvr:`, each with why. `--find TEXT` shows only the names holding TEXT,
ignoring case. A name a recipe uses that the file does not have is answered with the nearest
branches, and a pointer here.

```text
$ rvr branches --find jet
GluGluToHToTauTau: GluGluToHToTauTau.root, tree Events, 9 of its 69 branches hold "jet"

  Jet_btag     f4  jagged
  Jet_eta      f4  jagged
  Jet_mass     f4  jagged
  Jet_phi      f4  jagged
  Jet_pt       f4  jagged
  Jet_puId     b1  jagged
  Muon_jetIdx  i4  jagged
  nJet         u4  flat
  Tau_jetIdx   i4  jagged
```

### `rvr try "EXPRESSION" [DIR] [--events N] [--sample NAME] [--kept]`

Shows what one expression gives, one line per event, on the first `--events` events (10 when not
given) of the first file of the study's first sample, or of `--sample NAME`, with the names the
recipe defines; it reads only the branches the expression names. `--kept` shows only the events the
cuts keep, which are the rows `rvr press` would write. It reads the study's three texts through
their validators and the agreement check, as a press does, so resolve first; a total stands in as
1, and says so.

```text
$ rvr try "first(Muon_pt[good_muon], -1)" --kept --events 5
GluGluToHToTauTau: GluGluToHToTauTau.root, tree Events (f4, one value per event)
  entry       29   44.166725
  entry       45   20.321072
  entry      140   60.48154
  entry      157   80.38942
  entry      162   60.832092
```

### `rvr derive FILE 'NAME = EXPRESSION'...`, `--drop NAME...`, `--rename OLD NEW`

Changes the derived columns of a pressed file: adds one with its expression (a plain name, used by
nothing else in the file), drops them (a derived column another uses only with it, or after it), or
renames one no other uses. An expression is never changed in place: drop the column, then add it
again. It first checks the file's derived columns against their records, works out only what is
new, changes the recipe the file carries to match (so `rvr reproduce` still presses the same file),
copies the arrays byte for byte, and signs the file again with your key, saying whose signature it
replaces (or that the file is no longer signed). The study's own `recipe.toml` is not changed, and
it says so ([SPEC §8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file)).

```text
$ rvr derive flat.rvr 'lead = first(Jet_pt, -1)' 'lead_share = lead / ht'
Added lead, lead_share to flat.rvr: it has 4 derived columns; signed with your key 692b3edafef73283…, which replaces the signature of your own key 692b3edafef73283…
Its study's recipe.toml is as it was: make the same change under [derived] there to keep it when the study is pressed again.
$ rvr derive flat.rvr --drop lead
rvr derive: lead_share uses lead: drop lead_share first, or with it
$ rvr derive flat.rvr --rename lead_share lead_fraction
Renamed lead_share to lead_fraction in flat.rvr: it has 4 derived columns; signed with your key 692b3edafef73283…, which replaces the signature of your own key 692b3edafef73283…
Its study's recipe.toml is as it was: make the same change under [derived] there to keep it when the study is pressed again.
$ rvr derive flat.rvr 'pt2 = Muon_pt * 2'
rvr derive: [derived] pt2 uses Muon_pt, but the file does not store Muon_pt: a derived column uses only the columns the file stores, under [columns], and the derived columns above it
$ rvr derive flat.rvr 'mean_jet_pt = ht'
rvr derive: flat.rvr already has a derived column named mean_jet_pt: an expression is not changed in place; drop it, then add it again
```

### `rvr validate FILE... [--complete]`

Checks each file in full, as [FORMAT §12](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md#12-what-a-reader-checks) says:
every array decompressed and checked, every derived column worked out and checked against its
record, and the study's three texts parsed and checked against each other and the file. One line
per file: `FILE: valid, …` (rows, columns, derived columns, entries read of those the samples hold,
holes by kind, emptiness, the signer) or `FILE: INVALID: WHY`. `--complete` also fails a valid file
that has holes or is empty. Exit status 1 when any file fails.

```text
$ rvr validate flat/flat.rvr nothere.rvr holes/holes.rvr
flat/flat.rvr: valid, 15 rows, 4 columns, 4 derived, read all 15 entries, signed by you
nothere.rvr: INVALID: it is not there
holes/holes.rvr: valid, 10 rows, 2 columns, read all 10 entries, not signed
```

The file with a hole, under Holes above, before it was filled:

```text
$ rvr validate holes.rvr
holes.rvr: valid, 5 rows, 2 columns, read 5 of 10 entries, 1 hole (damaged), not signed
$ rvr validate --complete holes.rvr
holes.rvr: INVALID: it has holes, and --complete asks for none (read 5 of 10 entries, 1 hole (damaged), not signed): rvr status lists them, rvr press --fill reads them again
```

Every reader (rvr, the Python package) refuses a file whose header fails its checks, or whose
signature no longer holds; `rvr dump` still shows it.

### `rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns] [--all]`

Prints the inside of a file: the header in full, the three texts, and every column with its kind,
length and first values, a derived column worked out with its expression:

```text
per_jet        f8[10]  derived = "weight / nJet"  [0.3333333333333333, 0.125, 0.75, inf, 0.75, ...]
```

`--files`, `--recipe`, `--samples`, `--lock` and `--columns` print only those parts, in the order
given. `--all` prints every value of every column: a line naming them, then one line a row, the
values apart by tabs (its sample's name, its entry, then each stored column and each derived column
worked out, a list as `[a, b]`), then the sample columns. It holds every column decoded while the
rows are written, so it needs memory for the file's columns. It goes alone or with `--columns`; with
another part it is wrong use. A file whose header fails its checks is still shown, after a line
saying why ([SPEC §9](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#9-inspecting-files)).

```text
$ rvr dump flat.rvr --columns --all
sample	entry	weight	nJet	Jet_pt	per_jet
flat	0	1	3	[65.3125, 19.96875, 16.421875]	0.3333333333333333
flat	1	0.5	4	[51.75, 49.3125, 31.90625, 21.078125]	0.125
flat	2	2.25	3	[37.03125, 34.03125, 33.40625]	0.75
flat	3	1	0	[]	inf
flat	4	0.75	1	[120.5]	0.75
…
events_read    i8 by sample[1]  flat: 10
events_total   i8 by sample[1]  flat: 10
```

### `rvr header FILE`

Prints the header's bytes exactly as the file holds them (the JSON text between its length and its
arrays), without reading any array. A header that fails its checks is printed all the same, and
why is said on standard error, with exit status 1.

### `rvr diff A B [--rows] [--key COLUMN,…] [--sample NAME]`

Says where two files differ, from their headers alone, without decompressing anything: the rows,
in all and of each sample; samples, columns, derived columns and sample columns only one has; each
sample's label; kinds and expressions that changed; the entries, and each column and derived
column, of each sample whose contents changed (every array has one checksum per sample); and each
sample column's value that changed. It says when a header fails its checks, and when one file is
empty. Exit status 0 when the headers show no difference, 1 when they do, 2 when one cannot be read.

```text
$ rvr diff before.rvr after.rvr
sample column xsec: differs in sample c: 2.5 in before.rvr, 3.5 in after.rvr
```

With `--rows` it compares the events themselves, reading the arrays, so that two files holding the
same events in another order or layout can be compared: samples reordered, another program's
press, two skims of the same data. Events are matched within each sample (by name) by their file's
ROOT UUID, which of the sample's files with that UUID it is (copies of one file share one), and
their entry in it, or with `--key COLUMN,…` by the values of those stored flat
columns, which both files hold with one kind (`--key run,luminosityBlock,event` finds an event's
copy in another skim; two events of one key in one file are refused). Per sample it says how many
events only A holds, how many only B holds, and, of those both hold, the first whose values differ
in a stored or derived column both hold (floats bit for bit), with how many differ; values are
written as `rvr dump --all` writes them, with their bits in hex when two that differ read alike (as
NaNs). `--sample NAME` compares one sample. A column held with two kinds, or as a list in one file
only, is named and not compared, as is a column only one file holds. It holds the key columns of
both files, two row numbers per event both hold, and one column of each file at a time (a derived
one worked out alone, a chunk of the columns it uses at a time). Exit status 0 when the two hold the same events with the
same values, 1 when they do not, 2 when one cannot be read.

```text
$ rvr diff loose.rvr tight.rvr --rows --key run,event
Comparing the events of loose.rvr (A) and tight.rvr (B) by run, event:
sample one: 998 events in both, 93 only in A, 0 only in B
sample two: 998 events in both, 93 only in A, 0 only in B
```

### `rvr reproduce FILE [DIR] [--study FOLDER] [--workers N]`

Writes the recipe.toml, samples.toml and samples.lock FILE carries into DIR (FILE's name with
`.reproduced` when not given), presses them with the same event choices into
`DIR/<the file's study>.rvr`, and runs `rvr diff` on the two, exiting with its status (see the five
minutes above). Relative paths in the lock are read from FILE's folder, or `--study FOLDER`. A DIR
that holds a `.rvr` file, or a recipe.toml, samples.toml or samples.lock other than FILE's, is
refused, naming them.

### `rvr empty FILE... [--study FOLDER] [--anyway] [--force | -f]`, `rvr refill FILE [--study FOLDER] [--workers N]`

`rvr empty` gives back a file's space and keeps it in its place: it lets the arrays go and keeps the
header byte for byte, with the study it was pressed from, every checksum and its signature. On the
2.1 M-row bench, a 39.3 MB file keeps its 122.5 kB header. `rvr validate`, `rvr dump`, `rvr diff` and
the Python package read an empty file's header as ever and say it is empty. A refill reads the
inputs again, so `rvr empty` first checks that each one the lock names is as the lock recorded it,
and refuses a file made by another rvr version; `--anyway` empties it all the same. A file with
holes is not emptied. At a terminal it asks first, naming the file's rows, columns and size;
`--force` (`-f`) empties without asking, and is needed where nobody is at a terminal. Each file is
its own; the exit status is the worst.

`rvr refill` presses the study an empty file carries again, beside it, with the same event
choices, and puts the arrays back once each has the checksum its record states: with this build,
the file is the one it was, byte for byte, its signature as it was; with a build that compresses
otherwise, the header takes the arrays' new places and is signed again with your key, saying whose
signature it replaces. Other values are refused, naming the columns, and the file stays empty
([SPEC §8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#8-building-on-a-file)). Relative paths in the lock are read from
FILE's folder, or `--study FOLDER`.

```text
$ rvr empty flat.rvr
Empty flat.rvr (15 rows, 4 columns, 8.6 kB)? Its columns go and its header stays; rvr refill presses them again from the study it carries, reading its inputs again. [y/N] y
Emptied flat.rvr: 405 bytes given back; its header (8.2 kB) keeps its study, every checksum and its signature. rvr refill flat.rvr presses its columns again.
$ rvr validate flat.rvr
flat.rvr: valid, 15 rows, 4 columns, 4 derived (worked out once refilled), read all 15 entries, empty (rvr refill presses its columns again), signed by you
$ rvr refill flat.rvr
Pressing the study flat.rvr carries again, into flat.rvr.refill:
Pressing 2 samples, 3 files, 15 entries, 3 at a time:
…
Wrote flat.rvr.refill (15 rows, 4 columns and 4 derived, 0.0 MB) in 0.0 s.
Refilled flat.rvr: every column is back, byte for byte as it was, and its signature, by your own key 692b3edafef73283…, is as it was.
```

`rvr press` does not build on an empty file: it presses every column. `rvr status` says a file is
empty, with exit status 1.

`rvr derive`, `rvr empty` and `rvr refill` keep every tool key a file has (a top-level key of
another tool's, [FORMAT §11](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md#11-tool-keys)) as it was, and so does a press
that replaces the file at its target (a fill, a build on its columns or samples, a full press): the
new file carries them. A file at the target that does not open has none to keep, and a press adds
none of its own.

### `rvr key new [NAME]`, `rvr key show`, `rvr key trust KEY NAME...`

rvr's folder is `$RVR_CONFIG` when it is set and not empty, else `$XDG_CONFIG_HOME/rvr` when that
is set and not empty, else `~/.config/rvr`. `rvr key new` makes your Ed25519 key, `key` in that
folder (keep it private): 64 lowercase hexadecimal characters and a newline, readable by you alone,
never written over. Every file you press is signed with it from then on, so a reader can tell who
pressed it and that its header has not changed since. It prints your public key, and the
`rvr key trust` line others use; `rvr key show` prints your public key.

```text
$ rvr key new Alice
Made your key ~/.config/rvr/key (keep it private). Every file you press is signed with it from now on.
Your public key, for whoever reads your files:
  692b3edafef73283c92eecbc3950ca497369f823725dc9e12add7180146cfc99
They trust your files with:
  rvr key trust 692b3edafef73283c92eecbc3950ca497369f823725dc9e12add7180146cfc99 Alice
```

`rvr key trust KEY NAME...` adds `KEY NAME` (the words of NAME joined by spaces) as a line of
`trusted` in that folder; `#` lines and blank lines are passed over. A key trusted already stays
under its name: trusting it by that name says so (exit 0), and by another is refused (exit 1),
saying which line to take out. `rvr validate` names a signer from it: `signed by you`,
`signed by NAME (trusted)`, or `signed by key K…, which you do not trust yet (rvr key trust KEY NAME)`
([SPEC §10](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#10-keys-and-proxies)).

### `rvr proxy`, `rvr proxy init [--hours H]`

`rvr proxy` shows the grid proxy rvr uses (`$X509_USER_PROXY`, else `/tmp/x509up_u<your uid>`): its
file, whose it is, its VO and how long its attributes last, the time left, and what to do when it
is missing or ending. Exit status 0 when it can be used.

```text
$ rvr proxy
proxy     /tmp/x509up_u501
The grid proxy at /tmp/x509up_u501 does not exist. Run rvr proxy init to make one.
```

`rvr proxy init` makes a proxy for the VO cms with no other grid tool installed. It reads your
certificate and key from `$X509_USER_CERT` and `$X509_USER_KEY`, else `~/.globus/usercert.pem` and
`userkey.pem`, else `~/.globus/usercred.p12`, else the only `.p12` there (the exact order is
[SPEC §10](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#10-keys-and-proxies)). It refuses a key file with any permission for
its group or other accounts, giving the `chmod 600` that fixes it; asks the passphrase without
showing it; asks CMS's VOMS server for your attributes; and writes a proxy valid for `--hours`
(1 to 720, 192 when not given, never past the certificate's end), readable by you alone.

```text
$ rvr proxy init
Passphrase for ~/.globus/userkey.pem:
Asking voms-cms-auth.cern.ch for your cms attributes...
Wrote /tmp/x509up_u501:
proxy     /tmp/x509up_u501
whose     /DC=ch/DC=cern/OU=Organic Units/OU=Users/CN=…
VO        cms (attributes until 2026-10-27T17:14:08Z)
left      8 days 0 hours, until 2026-10-27T17:14:08Z
The grid proxy at /tmp/x509up_u501 is valid for 8 days.
```

(A real run of 2026-10-19, the user's name and folder left out.) The proxy is what
`voms-proxy-init -voms cms` makes (RFC 3820, inheriting all rights, with the VOMS attributes):
`voms-proxy-info` and OpenSSL read and verify it. A press or resolve that reads storage warns when
the proxy, or its attributes, have less than 6 hours left.

## samples.toml

`samples.toml` says what a study reads. Every key, placeholder and file form is
[SPEC §3](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#3-samplestoml); this is its example:

```toml
format = "rvr-samples"
version = 1

[places]
lpc = "root://cmseos.fnal.gov//store/user/username/ntuples"

[defaults]
tree = "KUAnalysis"
metadata = { lumi = 59.8 }

[eras.UL17]
era = 2
[eras.UL18]
era = 3

[[sample]]
name = "ST_s-channel"
label = "ST s-channel"
files = "{lpc}/Summer20{era}_106X/ST_s-channel_*.root"
eras = ["UL17", "UL18"]
metadata = { kind = "background", xsec = 3.36 }
```

It starts with `format` and `version` and has `[places]` (short names for long locations, used as
`{NAME}`), `[defaults]` (what every sample has unless it says otherwise) and `[eras]` (each era's
metadata), each even when empty. Each `[[sample]]` takes exactly `name`, `label`, `files`, `tree`
or `trees`, `eras`, `events`, `choose`, `seed` and `metadata`; any other key is refused with the
nearest. `files` are paths on this computer, or `root://`, `davs://` or `https://` names on storage,
with `*` and `?` in the file name ([SPEC §3.8](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#38-where-files-are)). The entry
above makes two samples, `ST_s-channel_UL17` and `ST_s-channel_UL18`, labelled
`ST s-channel (UL17)` and `ST s-channel (UL18)`.

A sample's metadata travels in the `.rvr`, and a recipe reads it as `sample.KEY`. A key that holds
one number, text or truth value per sample is also a sample column, one value per sample; a sample
that lacks a number or a text has `null` there (which keys, exactly:
[FORMAT §5](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md#5-sample-columns)):

```text
xsec           f8 by sample[2]  flat: null, c: 3.5
```

A true-or-false value has no `null`: give a flag to every sample in `[defaults]`, and set it only
where it differs.

## recipe.toml

The recipe is TOML, with one expression per value, evaluated over whole columns of events. The
[recipe reference](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/crates/rvr-recipe/README.md) is the language: every table, operator and function,
each with an example and its result; how a press uses a recipe is
[SPEC §4](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#4-recipetoml).

```toml
format = "rvr-recipe"
version = 1

[define]                      # names for expressions, used by the lines below them
good_muon = "Muon_tightId and Muon_pfRelIso04_all < 0.15 and Muon_pt > 20"

[keep]                        # an event is kept when it passes every line
trigger = "HLT_IsoMu24"
one_muon = "count(good_muon) >= 1"

[columns]                     # what the file stores
branches = ["MET_pt", "Muon_pt", "Jet_pt", "genWeight"]    # branches stored as they are
ht = "sum(Jet_pt)"            # computed while pressing, one number per event

[totals]                      # summed over every event read, before the cuts, per sample
sumw = "genWeight"

[derived]                     # kept as expressions, worked out when read
met_over_ht = "MET_pt / ht"
scaled = "genWeight / sample.sumw"
```

Every recipe writes all five tables, even the ones it leaves empty. A total is the sum over every
event a sample read, before the cuts, which a file of the events kept cannot give afterwards: the
sum of `genWeight` that normalizes simulation. A change to `[totals]` presses every event again.

## Grid access

LPC EOS and most CERN EOS areas need a grid proxy; CMS Open Data does not.

```sh
rvr proxy init
```

(`voms-proxy-init -voms cms -valid 192:00` makes the same proxy, where it is installed.) rvr trusts,
with no setup, the public web authorities, CERN's, and the 83 IGTF authorities (release 1.146, the
set grid sites install as `ca-policy-lcg`), which sign the storage of most CMS sites, FNAL's dCache
among them. `$X509_CERT_DIR`, else `/etc/grid-security/certificates` when it exists, adds others,
for an authority newer than rvr ([SPEC §10](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/SPEC.md#10-keys-and-proxies)). Without a
valid proxy, local files and public storage still resolve.

## When something goes wrong

| you see | what to do |
|---|---|
| `rvr press: --evnts is not one of its flags. Did you mean: --events?` (exit status 2) | wrong use: the command did nothing; use the flag it names, as the usage below the line shows |
| `… has no key "treee"; it takes name, label, … Did you mean: tree, trees?` | fix the key; metadata goes in its own table, `metadata = { … }` |
| `… matched no file … Did you mean: …?` | fix the file name or pattern; the suggestion is the nearest name in that folder |
| `… has no tree "Events". Its trees: …` | set `tree` (or `[defaults] tree`) to one of the trees listed |
| `{lcp} is not defined here; defined: {lpc}` | a misspelt place, or a place missing from `[places]` |
| `a name holds only ASCII letters, digits, _ and - …` | use the name it suggests, and put the text in `label` |
| `recipe.toml: A recipe has no [totals] table: …` | add the table it names, empty if need be; at a terminal, `rvr press` offers to |
| `… refused the listing (403). It needs a grid proxy: run rvr proxy init.` | run `rvr proxy init` |
| `No usable grid proxy: …` | run `rvr proxy init` |
| `… holds your private key, and other accounts on this machine can read it` (from `proxy init`) | run the `chmod 600` given |
| `…: the passphrase is wrong` (from `proxy init`) | type it again; it is the one you chose when you saved the certificate |
| `… invalid peer certificate: UnknownIssuer` | an authority newer than rvr's: set `X509_CERT_DIR` to the grid authorities folder (`/etc/grid-security/certificates` where grid tools are installed) |
| `… no storage has answered, and none that answered earlier in this press takes a connection now (…): the network seems to be gone` (from `press`) | the network went: press again once it is back, which carries on from the files done |
| `… rvr cannot tell that site down from the network gone, so it stops and keeps what it read` (from `press`) | press again once the site answers, which carries on from the files done |
| `  waiting 30 s to ask again for … (3 of 8 tries made): …` (from `press`) | nothing: the storage is busy and the read is asked again; if all eight tries fail it is a hole (`refused` for a `403` that stays, `unreachable` otherwise): `rvr press --fill` later |
| `… refused the listing (403, …). Either the grid proxy does not give access there, or the storage is refusing requests while it is busy …` | `rvr proxy` shows whether the proxy is valid; if it is, try again later |
| `Wrote … with 2 holes:` (exit status 3, from `press`) | each hole says why; `rvr press --fill` reads them again once they can be read |
| `…, tree T, entries A to B: X: decompression failed (zlib): …, in basket N of branch X (…)` (a `damaged` hole, from `press`) | the file is damaged at that basket: put another copy in its place if there is one, resolve again, and `rvr press --fill` |
| `… no longer exists: put it back and run rvr press --fill …` (a `gone` hole, from `press`) | put it back and fill; if it moved, resolve again, then press |
| `1 file differs from the lock: …` (from `check`) | the file changed since the lock: resolve again if that is intended, and review the new lock |
| `samples.toml has changed since samples.lock was written` | resolve again, and review the new lock |
| `… It has changed since samples.lock was written: …` (a `changed` hole, from `press`) | as for `check`: resolve again if the change is wanted, review the new lock, and `rvr press --fill` |
| `… has a samples.lock but no samples.toml` (from `press`) | put samples.toml back: the file carries both |
| `rvr press would press every column, as it was made by another build of rvr (…)` (from `status`) | a full press, by design: columns made by another build are not copied |
| `This file is rowver 14; this reader reads rowver 15 only.` | press its study again with `rvr press` |
| `The file was made by rvr …, newer than this one (…), and may use what only a newer version reads` | update rvr and rowver to that version or later |
| `the header changed after it was signed` | the file was edited after it was pressed: press it again |

MIT licensed.
