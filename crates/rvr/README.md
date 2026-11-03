# rvr

The command for RowVer studies: start a study, say what it reads, see exactly what it found, and
look inside the `.rvr` files it makes.

```sh
rvr new DIR                 # start a study
rvr proxy init              # make a grid proxy from your certificate (for grid storage)
rvr samples resolve …       # find every file the study reads
rvr branches                # the branches a sample's file holds
rvr try "EXPRESSION"        # what an expression gives on the first events, before pressing
rvr press                   # read them through the recipe into one .rvr file
rvr status                  # is the .rvr current, and what would press read
rvr derive FILE …           # add, drop or rename derived columns of a pressed file
rvr validate / dump / diff  # check and inspect .rvr files
rvr reproduce FILE          # press a file's own study again and compare
```

New to `.rvr` files? [The examples](https://gitlab.cern.ch/dgrove/rowver/-/blob/main/docs/examples/README.md)
make a first one from a DAS dataset and take it, step by step, to a plot.

## Install

Download it from the [releases](https://gitlab.cern.ch/dgrove/rowver/-/releases): one file, no
Rust needed. The Linux one runs on any x86_64 Linux with glibc 2.28 or later (el8 and el9, so the
LPC and lxplus; Ubuntu 20.04 on); the macOS one on Apple silicon and Intel. For example on the LPC:

```sh
mkdir -p ~/bin && cd ~/bin
curl -sL https://gitlab.cern.ch/api/v4/projects/dgrove%2Frowver/packages/generic/rvr/15.0.0/rvr-15.0.0-x86_64-linux.tar.gz | tar xz
~/bin/rvr --version     # add ~/bin to PATH in ~/.bashrc to type just rvr
```

(Downloaded with `curl`, the macOS binary opens as it is; one saved from a browser needs
`xattr -d com.apple.quarantine rvr` first.) With Rust 1.91 or later (`rustup update`):

```sh
cargo install --locked rvr
```

or the newest commit with `cargo install --locked --git https://gitlab.cern.ch/dgrove/rowver rvr`, or,
from a checkout of rowver, `cargo install --path crates/rvr`. Each puts `rvr` in
`~/.cargo/bin`. `rvr --version` says which `.rvr` version it reads and writes, and the commit it
was built from.

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

Then press it, here the first 50,000 events of each sample:

```text
$ rvr press --events 50000
Pressing 2 samples, 2 files, 100,000 entries, 2 at a time:
  1/2       50,000 entries      1,300 kept   VBF_HToTauTau.root
  2/2       50,000 entries        948 kept   GluGluToHToTauTau.root
Read 100,000 entries; kept after each cut:
  trigger                                8,590
  a_good_muon                            6,650
  a_good_tau                             2,248
Wrote higgs_tautau.rvr (2,248 rows, 11 columns, 0.3 MB) in 2.6 s.

$ rvr validate higgs_tautau.rvr
higgs_tautau.rvr: valid, 2248 rows, 11 columns, read 100000 of 968616 entries, not signed
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
Wrote higgs_tautau.reproduced/higgs_tautau.rvr (2,248 rows, 11 columns, 0.3 MB) in 2.4 s.
Comparing the two with rvr diff:
The headers show no difference in rows, samples, columns, derived columns, by_sample or sample columns.
```

## A study folder

| file | what it is | written by | commit it? |
|---|---|---|---|
| `recipe.toml` | what to keep and store of every event | you | yes |
| `samples.toml` | what to read, in your words: datasets, folders, patterns, eras | you, or any tool that writes it | yes |
| `samples.lock` | every file found: where, bytes, adler32, ROOT UUID, entries, and what a press reads of it | `rvr samples resolve` | yes; review its changes like code |
| `STUDY.rvr` | the pressed events, with the three texts above inside it | `rvr press` | no (`.gitignore` leaves it out) |

## Commands

Every command prints what it did, or one sentence saying what is wrong and how to fix it. Exit
status: 0 when all went well, 1 when something failed or differs, 2 for a command it does not know;
`rvr press` exits 3 when the file it leaves has holes (see Holes, below).

### `rvr new DIR`

Writes `samples.toml`, `recipe.toml` and `.gitignore` into `DIR`, making the folder if needed.
It never writes over a file: if any of the three is already there it writes nothing and says so.
The recipe's `title` is the folder's name.

### `rvr samples expand samples.toml [--json]`

Lists the samples `samples.toml` describes, with each one's name, label, tree, event choice and
what it reads, after eras, mass points and placeholders are filled in. It asks no storage, so it is the
quick check while editing. `--json` prints them as JSON for other tools.

### `rvr samples resolve samples.toml`

Finds every file of every sample and writes `samples.lock` beside `samples.toml`. For each kind
of source it asks:

| source | written as | asked of |
|---|---|---|
| storage: LPC EOS, CERN EOS, any grid site | `files = "root://host//path/file.root"` (or `davs://`, `https://`) | the storage's WebDAV door: one listing per folder, one checksum request per file, and a few small reads of each file for its UUID and entries |
| this computer | `files = "~/data/…/file.root"` (or a path relative to `samples.toml`) | the file itself, read whole for its adler32 |

Of each sample's first file it also reads its tree's whole record, for the sample's branch list
(every branch's name, kind and shape, written once in the lock as a `[[branches]]` list): for a
skim that can be megabytes. A press checks each file against it, for the branches the recipe reads.

No event is read. If any sample fails,
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

### `rvr press [DIR] [--events N|all] [--random] [--seed S] [--out FILE] [--workers N] [--full] [--fill [KINDS]] [--restart]`

Presses the study in `DIR` (the current folder when not given): reads what `samples.lock` names,
keeps the events that pass every cut of `recipe.toml`, and writes `DIR/<DIR's name>.rvr`, or
`--out FILE`. `--events`, `--random` and `--seed` are `plan`'s: the first N of every sample, every
sample whole, or N chosen at random. `--workers` is how many files are read at once (one per core
by default). `samples.toml` must be beside the lock, since the file carries both.

**Adding a column you forgot.** Add it to `recipe.toml` and run `rvr press` again (`rvr status`
says beforehand whether the file can be built on, and what would be read). When the
file is already there and was pressed from the same `samples.toml` and `samples.lock`, with the
same cuts and the same choice of events, by the same build of rvr, only the columns that are new
or whose definition changed are read, and only the events the file already kept are stored; every
other column is copied from it, and the cuts are not run again. The file written is the one a full
press of the recipe writes, and `rvr diff` against one shows no difference:

```text
$ rvr press
local.rvr has the events this recipe keeps (22,325) and 10 of its 11 columns.
Reading only Jet_pt for those events, 2 files 2 at a time:
  1/2      476,963 entries      9,362 kept   GluGluToHToTauTau.root
  2/2      491,653 entries     12,963 kept   VBF_HToTauTau.root
Wrote local.rvr (22,325 rows, 11 columns, 2.4 MB) in 0.2 s.
```

Here that reads at most 26.3 MB of baskets (`Jet_pt` and `nJet`) where a full press reads
108.0 MB; on local copies of the two files it takes 0.2 s instead of 0.6 s (Apple M5, 2026-10-30).
A column counts as changed when its expression, its stored kind, or any name it uses from
`[define]` changed.

**Adding a sample.** Add it to `samples.toml`, resolve, and run `rvr press` again (`rvr status`
says what it would read). When the file was pressed with the same cuts and the same columns, by
the same build of rvr, and every sample it holds is in the new lock as it was (the same files,
tree, metadata and choice of events), only the new samples are read; the rows of the others are
the file's own. A sample taken out, or the samples put in another order, reads nothing. The rows
are laid out again in the lock's order, so every column is packed again: an array is compressed
whole, and a sample's part of it cannot be copied as it lies; as a fill does, it holds one column
of the file at a time in memory. The file written is the one a full press writes; a sample it
holds with holes keeps them (`--fill` reads them).

```text
$ rvr press
flat.rvr holds the rows of 2 of the 3 samples as samples.lock has them.
Reading only the new sample c, 1 file, 5 entries, 1 at a time:
  1/1            5 entries          4 kept   flat_c.root
Read 5 entries of the new sample c; kept after each cut:
  jets                                       4
Wrote flat.rvr (12 rows, 2 columns, 0.0 MB) in 0.0 s.
```

On 2.1 M rows (ten samples of 30 links to the TChiWZ skim), adding an eleventh of 30 files took
1.5 s where a full press takes 4.9 s; taking one out took 0.75 s against 4.5 s, all of it the
packing again (medians of 9, Apple M5, 2026-10-31).

When the file cannot be built on, `rvr press` says why and presses every column; `--full` always
does. Other cuts, other `--events`, a sample whose files, tree or metadata changed in the lock, a
column changed together with the samples, or another build of rvr all mean a full press, since
the file's rows or the code that computed them would no longer be the ones it records.

Before a file is read, it is checked against the lock: its size and adler32, then its ROOT UUID
and entries. A file that changed is not read: it is a hole (below), and the file says so. A
`samples.toml` edited since the lock was written stops the press: resolve again and review the new
lock. Nothing is written until every file has been read or found to be a hole: the file appears
whole, or not at all.

#### Derived columns

A column under `[derived]` in `recipe.toml` is kept in the file as its expression, not as an array:
a reader (`rvr dump`, the Python package) works it out from the file's columns when it is asked for.
The Python package then holds it as it holds every column it has read, until `f.forget(...)` lets it
go or the file object goes; asked for again, it is worked out again. Its expression uses the columns
the file stores, the derived columns above it, and `sample.KEY` (the keys of `samples.toml`, the
sample's totals, and `sample.file`, the path of the row's file; not the counts the press records,
`events_read`, `events_total` and `events_missing`). While it reads, the press works each one out
over the events one run of each sample kept, so that a mistake in one (a value it cannot convert,
a type that differs between samples) stops it then, not once every file is read; this costs nothing measurable on the bench. It works each one out once more, from the arrays
it has packed for the file (holding the columns they use in memory), to record its kind, its
checksums and its facts, so that a reader can check what it works out and `rvr diff` can compare it,
and then writes the file with those records. Should one fail there, the file is written without its
derived columns, and the press says so: fix `[derived]`, and the next press reads no input. Changing
only `[derived]` reads no ROOT file: every stored column is copied from the file, and the derived
ones are worked out from it. `rvr derive` changes them in a pressed file alone.

Which columns to store and which to derive is a choice of space against time. On 2.1 M rows (300
copies of the TChiWZ skim, Apple M5, 2026-10-31), seven typical computed columns (a sum, a count, the
leading lepton's pt, a ratio, a transverse mass, the largest jet-pair mass, jets cleaned of leptons) take 70 bytes per row
stored, uncompressed, 35% more than the ten columns they use; derived, they add 0.54 s to the press
(4.0 s against 3.5 s), and a Python run that works all seven out takes 0.98 s, where one that reads
them stored takes 0.10 s and one that only opens the file 0.05 s. A simple one, `sum(PT_jet)`, takes
0.11 s.

#### While it reads

On a terminal, a press shows one bar per sample (its entries read of those it chose), how many
events it has kept, how fast it reads and how long it has taken, redrawn five times a second; what
happens meanwhile (a wait, a hole) is printed above the bars. In a log or a batch job (when the
output is not a terminal), it prints one line per file instead, as each is read. With more samples
than fit, the ones under way have a bar and the rest a line. The bars are cut to the terminal's
width, so none wraps. The rate counts every byte read, the whole-file checksum read of a local
file included. Drawing costs under 1%: 0.397 s against 0.394 s without it on the TChiWZ skim (30
files, on a terminal; Apple M5, 2026-10-30).

```text
$ rvr press --workers 2
Pressing 3 samples, 90 files, 636,390 entries, 2 at a time:

  early                       ████████████████████ 100%  63,639 / 63,639
  middle                      ████████████████████ 100%  212,130 / 212,130
  late_and_long_named_sample  █░░░░░░░░░░░░░░░░░░░   5%  21,213 / 360,621

  147,966 kept so far · 8762 MB/s · 1.5 s
```

#### Holes

A press that cannot read part of what it chose still writes the file, without those events, and
records each **hole** in it: the entries of the file it could not read, a kind, and why. The kinds
are `tape` (every copy of a dataset's file is on tape only), `unreachable` (its storage kept
failing after every wait), `damaged` (a basket does not decompress), `gone` (the file is not
there any more), `changed` (it is not the file the lock names) and `refused` (its storage refuses
it with a valid proxy). A damaged basket makes a hole of the 50,000 entries read with it, and the
rest of the file is read; any other kind makes a hole of the rest of the file. What is not the
file's fault makes no hole and stops the press: a mistake in the recipe, a grid proxy missing or
refused, a certificate rvr does not trust, a local file that cannot be read for want of rights or
a mount gone. The press exits 3 when the file has holes:

```text
$ rvr press
Pressing 1 sample, 2 files, 10 entries, 2 at a time:
  1/2            0 entries          0 kept   flat_b.root
    hole (damaged): ./flat_b.root, tree analysis, entries 0 to 4: weight: decompression failed (zlib): corrupt deflate stream, in basket 0 of branch weight (entries 0 to 4, 111 bytes at byte 662). The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.
  2/2            5 entries          5 kept   flat_a.root
Read 5 of 10 entries; with no cuts, every one is kept.
Wrote flat.rvr (5 rows, 2 columns, 0.0 MB) in 0.0 s, with 1 hole:
  damaged     flat  ./flat_b.root, tree analysis, entries 0 to 4: weight: decompression failed (zlib): corrupt deflate stream, in basket 0 of branch weight (entries 0 to 4, 111 bytes at byte 662). The file is damaged there: if another copy of it exists, put it in its place, run rvr samples resolve samples.toml, then rvr press --fill.
rvr press --fill reads them again once they can be read.
```

`rvr status` shows what each sample is missing, by kind of hole. Each sample's `events_read` counts the entries read, and a file with holes adds `events_missing`
(the entries chosen that lie in holes) to every sample's metadata, so both are sample columns.
`rvr validate` says how many holes, of which kinds; `rvr validate --complete` fails a file with
holes, for scripts. Pressing again a file that holds the study as it is but for its holes says so
and writes nothing (exit 3: the file still has holes).

**`--fill`** reads only the holes, and writes the file again with their rows in place: within each
input file, its rows and the new ones merged by entry. Every column is packed again, but nothing
but the holes is read. `--fill tape,unreachable` reads only holes of those kinds. Holes that fail
again stay holes, with their new kind and reason; a fill that fills nothing, each hole failing as
before, leaves the file as it was. A fill holds one column of the file at a time in memory. The
study must be the one that pressed the file, except that the lock may have been resolved again for
the files whose holes it reads (a file put back where it was, or another copy of it in its place),
and for where a dataset's copies are (a Rucio rule): the file filled is then the one a full press
makes. A file that moved, or has other entries, changes the events chosen: press again. `rvr status`
says whether `--fill` would fill the file, or why not. On the TChiWZ skim (30 files, 212,130 entries) with one file's hole, a fill takes 0.16 s
where a full press takes 0.39 s (0.18 s and 1.84 s with `--workers 1`; Apple M5, 2026-10-29).

```text
$ rvr press --fill
Filling flat.rvr: reading 1 hole of 1 file, 5 entries, 1 at a time; every other row is the file's own:
  1/1            5 entries          5 kept   flat_b.root
Read 5 entries in the holes; with no cuts, every one is kept.
Wrote flat.rvr (10 rows, 2 columns, 0.0 MB) in 0.0 s.
```

#### Stopped part way

A press is staged in a folder beside its output, `FILE.partial/`, with a journal of the files it
has finished. A press stopped part way (Ctrl-C, a killed process, a laptop gone to sleep, a failure
that is not a file's own, such as a disk not mounted, a disk full or a certificate refused) leaves
the folder, and the next `rvr press` of the same study carries on from the files it had finished;
`rvr status` says how far it got. The journal is made safe on disk every 2 seconds while files
finish, so a press stopped loses at most the files of its last 2 seconds, which are read again; a
file stopped half read is read again from its start, and so is one that became a hole for a reason
that may pass (`unreachable`, `refused`, `tape`), while a `damaged`, `gone` or `changed` one stays
a hole. A press that fails once every file is read
(the disk filled while the file was written) keeps them all, and the next press only writes the
file. A press that is not the same (another recipe, samples, lock, choice of events, columns read
or build of rvr) starts over and says why. `--restart` always starts over. While a press runs it
holds its journal, and another press of the same output, or a `--restart`, is refused. The folder
goes once the file is in place, and when the press fails for a reason pressing again would not
mend (a mistake in the recipe).

A press that loses the network stops and keeps what it read, instead of making a hole of every
file it could not reach. While a read waits for a storage, once no storage has answered for 2
minutes, rvr tries for 5 seconds to connect to the hosts that answered earlier in the press and
are not failing now (another storage, or Rucio), or, when every one fails, to all of them, and
then, when they are all at one site, to CERN's Rucio server for CMS as well
(`cms-rucio.cern.ch`, asked only to take a connection). One that connects shows the network works: only the failing storage is
down, and its files become `unreachable` holes after their waits. If none connects, twice 15
seconds apart, the network is gone: the press stops and says so, and pressing again once it is
back carries on. A name that does not resolve is waited on the same way. (A press of one site at
CERN cannot tell CERN down from the network gone; it stops and keeps what it read.)

```text
$ rvr press --workers 1
Pressing 1 sample, 90 files, 636,390 entries, 1 at a time:
   1/90        7,071 entries      3,523 kept   f01.root
…
  57/90        7,071 entries      3,523 kept   f57.root
^C
$ rvr status
tchiwz.rvr.partial: a press stopped there with 33 of 90 files done (journal from 2026-10-30T10:24:33Z). rvr press carries on from there when the study and the press are the same, and --restart starts over.
tchiwz.rvr is not there: rvr press makes it.
$ rvr press --workers 1
Picking up tchiwz.rvr where it stopped: 33 of 90 files done (journal from 2026-10-30T10:24:33Z).
Pressing 1 sample, 90 files, 636,390 entries, 1 at a time:
  34/90        7,071 entries      3,523 kept   f34.root
…
  90/90        7,071 entries      3,523 kept   f90.root
Read 636,390 entries; kept after each cut:
  met                                  317,070
  leptons                              317,070
Wrote tchiwz.rvr (317,070 rows, 7 columns, 0.8 MB) in 3.7 s.
```

The file picked up is the one an uninterrupted press writes. Keeping the journal costs at most
1.5%: on the TChiWZ skim, 0.395 s against 0.397 s without it (30 files, default workers), 5.655 s
against 5.609 s (90 files, `--workers 1`, three checkpoints), and 3.827 s against 3.770 s for a
50.7 MB file of 22 columns, synced before it is put in place; Apple M5, 2026-10-30.

What a press holds in memory does not grow with the events it keeps (building on an earlier
file, it holds the entry number of each event that file kept, 8 bytes each). Each file's kept values go
to `FILE.partial/spool` as they are read, compressed where that pays, and each column is then
packed from there a piece at a time into `FILE.partial/pressed.rvr.arrays/`. While the columns are packed
both exist, so a press needs free disk of about twice the file it writes. Pressing 12 million rows of 31 columns
(a 1.5 GB file, 4 workers) peaks at 0.3 GB of memory, where holding every row took 4.9 GB
(2026-10-20).

The file carries the whole study: the text of `recipe.toml`, `samples.toml` and `samples.lock`,
every file read (with its UUID and entries), which events of each sample were read (all, the first
N with where each file stopped, or N at random with the seed, so `rvr validate` can choose them
again), each sample's label, and its metadata with two more keys, `events_read` and
`events_total`, which become sample columns. It also records what built `rvr` (its version, git
commit, compiler and platform), and is signed when you have a key (see `rvr key`). A random choice
reads only around its chosen entries: a stretch of 50,000 entries with none chosen is skipped
(in a local file, any 2,000), and a file with none chosen is not opened. For most choices that is
still every basket of every file it reads.

Remote files are read through their storage's HTTPS door, all the baskets the recipe can use of
each 50,000 entries asked for in one request (`multipart/byteranges`, which EOS answers).

A storage under load refuses requests for minutes at a time (EOS answers `403`; others `429` or
`503`), and the network drops some. So a read that fails that way is asked again after 5 s, then
15 s, 30 s, 1, 2, 4 and 5 minutes, about 13 minutes in all, and each wait is printed:

```text
  waiting 30 s to ask again for WZTo3LNu_UL18.root (3 of 8 tries made): its storage answered 403 Forbidden
```

The press goes on where it was; a read that fails all eight times leaves a hole (`unreachable`, or
`refused` for a `403` that stays), and the others stop waiting as soon as the press stops. A file
that is gone (`404`) is a hole at once; a `401`, or a certificate that is not trusted, stops the
press, as every file there would fail the same. Fetching ahead is never waited for: the read that
needs the bytes is.

A file of a central dataset (`dataset = …`) is read from one of its copies. As the press starts,
Rucio is asked once where every such file can be read over HTTPS. Only copies on disk, available,
and at a CMS storage door are kept, and Rucio must give the size and adler32 the lock has. The
sample's `sites` in the lock come first: when `rvr samples resolve` wrote the lock, it asked the
sample's first file with copies on disk once at every storage that held it, all at once, and
recorded the three that answered first, within 3 s, with its size (and adler32, when the storage
gives one), each with the address its files' logical names follow (about 310 bytes of the lock
per sample). The lock rides in every file pressed from
it, so a refill or a reproduction years later tries the same sites first. When Rucio is
unreachable but a recorded site is not, a press reads each file from the recorded sites the lock
says hold it (only at CMS storage doors, as with Rucio's answers), and a file none of them holds
is a hole of kind `unreachable`, for `rvr press --fill unreachable` once Rucio answers; one not
where its site's address says is the hole its storage's answer makes, saying where the address
came from. Any other answer of Rucio's, such as a proxy it refuses,
or no network at all, stops the press as before. Each file's first question (its size and adler32) goes at once to the storage that
answered fastest so far and to two not yet measured. The first to answer with the file the lock names (its size, and
its adler32 when the storage gives one) is read, so every storage of the dataset gets timed as its
files open. A storage that failed twice since it last served a whole file is tried last. A read
that fails is tried at the other copies, since every copy holds the same bytes: the copies it
failed at are left once another serves it, and a read that every copy fails is a hole. A file
whose every copy is on tape is a hole of kind `tape`, and the press goes on with the others. The
file records which storage served each dataset file (`served`: its Rucio storage element and
door), and the press says what it read from where:

```text
$ rvr press --events all
Rucio said where the 18 dataset files can be read in 4.3 s.
Pressing 1 sample, 18 files, 9,821,283 entries, 10 at a time:
  …
  read from T2_DE_DESY: 1 file, 24.0 MB
  read from T2_US_Florida: 14 files, 238.3 MB
  read from T2_US_Purdue: 3 files, 28.6 MB
  passed over T1_US_FNAL_Disk (cmsdcadisk.fnal.gov:2880) 3 times: could not be asked: invalid peer certificate: UnknownIssuer
  …
Read 9,821,283 entries; kept after each cut:
  three_leptons                      1,234,580
Wrote wz.rvr (1,234,580 rows, 9 columns, 27.8 MB) in 41.4 s.
```

That is the whole WZTo3LNu UL18 NanoAOD dataset (14.9 GB on storage, of which the recipe's
branches were 291 MB), read from a laptop in the United States on 2026-10-19. With `--workers 1`
the same press took 148.8 s. rvr then passed over eight of the dataset's storages, whose
certificates are signed by grid authorities it did not carry (given them through `X509_CERT_DIR`,
FNAL served 7 of the files and the press took 37.5 s). rvr now carries them (see Grid access): on
2026-10-29 the same dataset, through a recipe reading 419 MB of it, took 29.5 s, FNAL serving 5
files.

### `rvr status [DIR] [--out FILE] [--events N|all] [--random] [--seed S] [--holes]`

Says whether the study's `.rvr` file (the one `rvr press` with the same options would write)
holds the study as it is now, from the study's three texts and the file alone (no input is read): what changed in
`recipe.toml` (columns new, changed or dropped; the cuts), whether `samples.toml` was edited or
the lock resolved again since, and what `rvr press` would read; then how much of what it chose
each sample holds, and its holes by kind, each kind with what to do about it (`--holes` lists every
hole: sample, file, entries, kind and why). A press that stopped part way is shown, with how many
files it finished, and so is one under way. Exit status 0 when the file holds the study as it is,
was pressed with the same `--events`, `--random` and `--seed`, and has no holes, so a script or an
app can ask before pressing. It asks no storage: whether the input files changed since the lock is
`rvr samples check samples.lock`.

```text
$ rvr status
flat.rvr: pressed 2026-10-30T19:26:53Z by rvr 15.0.0
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
flat.rvr: pressed 2026-10-30T19:26:53Z by rvr 15.0.0
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

Lists every branch of the first file of the study's first sample (or of `--sample NAME`), in
alphabetical order whatever the case, one per line with its type and its shape: `flat` (one value
per event), `jagged` (a list per event), `3 per event` for a fixed array, or `jagged, 3 per element`
for a list of fixed arrays (`xy[n][3]/F`). These are the names a
recipe uses, in full: `Jet_pt`, `lep_pt` in an ATLAS ntuple, `H1_PX` in an LHCb one, and for a
member of a split object such as `recoMuons_muons__RECO.obj.m_state.p4Polar_.fCoordinates.fPt`,
`branch('…')` with its name. Branches `rvr` cannot read (whole objects, strings, nested vectors)
are listed last, under `Not readable by rvr:`, each with why. A dataset's file with no copy on disk
(only on tape) is passed over for the next one, up to the tenth. It reads the file's description
only, no events, so it takes as long as opening the file. `--find TEXT` shows only the branches
whose name holds TEXT, in any case. A name a recipe uses that the file does not have is answered
with the nearest branches, and a pointer here.

```text
$ rvr branches --find jet
GluGluToHToTauTau: GluGluToHToTauTau.root, tree Events, 9 of its 69 branches hold "jet"

  Jet_btag     float   jagged
  Jet_eta      float   jagged
  Jet_mass     float   jagged
  Jet_phi      float   jagged
  Jet_pt       float   jagged
  Jet_puId     bool    jagged
  Muon_jetIdx  int32   jagged
  nJet         uint32  flat
  Tau_jetIdx   int32   jagged
```

### `rvr try "EXPRESSION" [DIR] [--events N] [--sample NAME] [--kept]`

Shows what one expression gives, event by event, on the first `--events` events (10 unless told)
of the study's first sample, or of `--sample NAME`. It reads the study's `samples.lock`, so resolve
first, and its `recipe.toml`, so the names it defines can be used. It
opens one file and reads only the branches the expression names. `--kept` shows only the events
the `[keep]` cuts keep, which are the rows `rvr press` would write. A mistake in the expression, or
a branch the file does not have, is said as `rvr press` would say it.

```text
$ rvr try "first(pair_mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon], Muon_mass[good_muon]), -1)" --kept --events 5
Rucio said where the 10 dataset files can be read in 1.6 s.
DY: 13D0AD97-6B32-CB4C-BA87-5E37BA4CF20E.root, tree Events (f4, one value per event)
  entry       14   88.6032
  entry       25   93.59802
  entry       35   90.682175
  entry       43   90.26609
  entry       52   92.6495
```

### `rvr derive FILE 'NAME = EXPRESSION'...`, `--drop NAME...`, `--rename OLD NEW`

Changes the derived columns of a pressed file: columns kept as the expression that computes them
from the file's own columns, worked out when they are read (see Derived columns, under
`rvr press`). A derived column is added with its expression, which may use the file's stored
columns, the derived columns above it, and `sample.KEY`; dropped; or given another name, its
expression as it was. An expression is never changed in place: drop the column, then add it
again. A derived column another uses is dropped with it, or after it, and keeps its name while it
is used. It first works the file's derived columns out and checks them against their records, as
`rvr validate` does, so that a file is changed and signed again only when they hold; then only
what is new is worked out, for its checksums and facts.

The file is written again with its arrays as they were, and the recipe it carries gains, loses or
renames the line under `[derived]`, so that `rvr reproduce` still presses the same file. The
study's own `recipe.toml` is not changed: make the same change there, or the next `rvr press` of
the study writes the file with the derived columns `recipe.toml` lists. Its signature is made again
with your key; with no key, a file that was signed is no longer.

```text
$ rvr derive tiny.rvr 'lead = first(Jet_pt, -1)' 'lead_share = lead / ht'
Added lead, lead_share to tiny.rvr: it has 3 derived columns; signed with your key 7c10dc33f447eb3a…
Its study's recipe.toml is as it was: make the same change under [derived] there to keep it when the study is pressed again.
$ rvr derive tiny.rvr --drop lead
rvr derive: lead_share uses lead: drop lead_share first, or with it
$ rvr derive tiny.rvr --rename lead_share lead_fraction
Renamed lead_share to lead_fraction in tiny.rvr: it has 3 derived columns; signed with your key 7c10dc33f447eb3a…
Its study's recipe.toml is as it was: make the same change under [derived] there to keep it when the study is pressed again.
$ rvr derive tiny.rvr 'pt2 = Muon_pt * 2'
rvr derive: [derived] pt2 uses Muon_pt, but the file does not store Muon_pt: a derived column uses only the columns the file stores, under [columns], and the derived columns above it
$ rvr derive tiny.rvr 'mean_jet_pt = ht'
rvr derive: tiny.rvr already has a derived column named mean_jet_pt: an expression is not changed in place; drop it, then add it again
```

### `rvr validate FILE... [--complete]`

Checks each file completely, as docs/FORMAT.md §12 says: every array is one zstd frame that
decompresses to its declared length and checksum, every per-sample checksum, the column facts, the
study's three texts parsed and checked against each other and the file (the lock's samples, files,
UUIDs, entries and metadata), that the rows belong to the files recorded, and that every row's
entry was read (a random choice is made again from its seed). An empty file gets every check but
its arrays'. One line per file: `valid`, how many entries the press read of how many its samples hold
(`read all 968616 entries`, so you can see nothing was left out), its holes by kind
(`read 5 of 10 entries, 1 hole (damaged)`), and who signed it, or the first problem found. Every
derived column is worked out, and checked against the checksums and facts the file records of it
(`1 derived`), and the recipe the file carries lists the same derived columns.
`--complete` also fails (exit 1, on its line) a valid file that has holes.

Every reader (rvr, the Python package) refuses a file whose header fails these checks, or whose
signature no longer holds; `rvr dump` still shows it.

### `rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns]`

Prints the inside of a file: the header in full, the texts of recipe.toml, samples.toml and
samples.lock, and every column with its kind, length and first values; a derived column worked
out, with its expression:

```text
mean_jet_pt    f8[8]  derived = "ht / nJet"  [33.901041666666664, 38.51171875, 34.822916666666664, 120.5, 33.901041666666664, ...]
```

Each flag prints only that part: `--files` the files read and each sample's event choice and
metadata, `--recipe`, `--samples` and `--lock` one text each, `--columns` the columns.

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

### `rvr reproduce FILE [DIR] [--workers N]`

Writes the recipe.toml, samples.toml and samples.lock that FILE holds into DIR (FILE's name with
`.reproduced` when not given), presses them again with the same event choice, and runs
`rvr diff` on the two files: exit status 0 when they hold the same data. A relative path in the
lock is read from FILE's folder, where `rvr press` writes it.

### `rvr empty FILE... [--study FOLDER] [--anyway] [--force | -f]`, `rvr refill FILE [--study FOLDER] [--workers N]`

`rvr empty` gives back a file's space and keeps it in its place: it lets the columns go and keeps
the header byte for byte, with the study it was pressed from, every checksum and its signature. On
the 2.1 M-row bench, a 39.3 MB file keeps its 114 kB header. `rvr validate`, `rvr dump`, `rvr diff`
and the Python package read an empty file's header as ever and say it is empty; reading a column
of it says how to fill it. A refill reads the inputs again, so `rvr empty` first checks that each
one the lock names is as the lock recorded it (a local file's size and adler32, a storage file's
digest, what Rucio says of a dataset file), and refuses, saying which is not, unless with
`--anyway`. A file with holes is not emptied: a hole may read otherwise later, and a refill must
read what the press read. At a terminal it asks first, naming the file's rows, columns and size,
and empties it only on a yes; `--force` (or `-f`) empties without asking, and is needed where no
one is at a terminal to answer, as in a script.

`rvr refill` presses the study an empty file carries again, beside it, with the same event
choice, and puts the columns back once every array has the checksum the header records: with the
same rvr, the file is the one it was, byte for byte, its signature as it was. An rvr that
compresses otherwise (another zstd) gives the same values in other bytes; the header then takes
their new places and is signed again with your key. Values that differ (an input or the rvr that
reads it has changed) are refused, naming the columns, and the file stays empty. A relative path in
the lock is read from FILE's folder, or FOLDER.

```text
$ rvr empty tiny.rvr
Empty tiny.rvr (8 rows, 3 columns, 6.2 kB)? Its columns go and its header stays; rvr refill presses them again from the study it carries, reading its inputs again. [y/N] y
Emptied tiny.rvr: 332 bytes given back; its header (5.9 kB) keeps its study, every checksum and its signature. rvr refill tiny.rvr presses its columns again.
$ rvr validate tiny.rvr
tiny.rvr: valid, 8 rows, 3 columns, 1 derived (worked out once refilled), read all 10 entries, empty (rvr refill presses its columns again), signed by you
$ rvr refill tiny.rvr
Pressing the study tiny.rvr carries again, into tiny.rvr.refill:
…
Refilled tiny.rvr: every column is back, byte for byte as it was; signed by you.
```

`rvr press` does not build on an empty file: it presses every column. `rvr status` says a file is
empty, with exit status 1.

### `rvr key new [NAME]`, `rvr key show`, `rvr key trust KEY NAME`

`rvr key new` makes your ed25519 key in `~/.config/rvr/key` (keep it private); from then on every
file you press is signed with it, so a reader can tell who pressed it and that its header has not
changed since. `rvr key show` prints your public key for others, and `rvr key trust KEY NAME` adds
someone's key to `~/.config/rvr/trusted`, so `rvr validate` names them. `RVR_CONFIG` names another
folder for both files.

### `rvr proxy`, `rvr proxy init [--hours H] [--vo VO]`

`rvr proxy` shows the grid proxy rvr would use: its file, whose it is, its VO and how long its
attributes last, the time left, and what to do when it is missing or ending. The exit status is 0
when it can be used.

`rvr proxy init` makes one with no other grid tool installed. It reads your certificate from
`~/.globus`: `usercert.pem` and `userkey.pem`, or a `.p12` file as your certificate authority
gave it (`usercred.p12`, or the only `.p12` there; `X509_USER_CERT` and `X509_USER_KEY` name other
files). It refuses a key file that other accounts can read, and says the `chmod` that fixes it. It
asks your passphrase without showing it, then asks the VOMS server for your attributes (`cms`
unless `--vo` says otherwise). It writes a proxy valid for `--hours` (192 unless told otherwise,
never past your certificate's end) to `X509_USER_PROXY`, or else `/tmp/x509up_u<your uid>`, readable
by you alone.

```text
$ rvr proxy init
Passphrase for /Users/…/.globus/userkey.pem:
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
`voms-proxy-info` and OpenSSL read and verify it. A press or resolve that needs the grid warns
when less than six hours are left.

## samples.toml

```toml
format = "rvr-samples"
version = 1

[places]                      # short names for long locations, used as {name}
lpc = "root://cmseos.fnal.gov//store/user/username/ntuples"

[defaults]                    # every sample has these unless it says otherwise
tree = "KUAnalysis"

[eras]                        # values become metadata; {era} is the era's name
[eras.UL17]
era = 2
[eras.UL18]
era = 3

[[sample]]
name = "ST_s-channel"         # makes ST_s-channel_UL17 and ST_s-channel_UL18
label = "ST s-channel"        # shown as "ST s-channel (UL17)"
files = "{lpc}/Summer20{era}_106X/ST_s-channel_4f_leptonDecays_TuneCP5_13TeV-amcatnlo-pythia8_Summer20{era}_106X.root"
eras = ["UL17", "UL18"]
metadata = { kind = "background" }

[[sample]]
name = "Signal"               # one file per era holds a tree per mass point
files = "{lpc}/Summer20{era}_106X_SMS/SMS-Signal_TuneCP5_13TeV-madgraphMLM-pythia8_Summer20{era}_106X.root"
eras = ["UL17", "UL18"]
trees = { "300/250" = "SMS_30000_25000", "300/292.5" = "SMS_30000_29250" }
events = 20_000               # makes Signal_300_292p5_UL18, shown as "Signal 300/292.5 (UL18)"
choose = "random"
seed = 7
metadata = { kind = "signal" }
```

Every samples.toml starts with `format` and `version` and has `[places]`, `[defaults]` and `[eras]`,
each even when empty; a key that is not one of these is refused, with the key it is nearest to.
Off a terminal a file that leaves out the header or a table is refused; at one, `rvr samples
resolve` offers to write them in. Every rule is in [docs/SPEC.md](../../docs/SPEC.md) §3.

| key | meaning |
|---|---|
| `name` | required: what every tool calls the sample. Letters, digits, `_` and `-`; unique. |
| `label` | what plots show; any text; the name when left out. |
| `files` | required: paths or URLs, one or a list; `*` and `?` may appear in the file name |
| `tree` / `trees` | the tree to read, or a table of label → tree making one sample per tree |
| `eras` | the eras this entry exists in, one sample each: `_ERA` added to the name, ` (ERA)` to the label |
| `events` | how many entries to read before any selection: a number, or `"all"` (the default) |
| `choose`, `seed` | `"first"` (the default) or `"random"`, reproducible from `seed` (1 when left out) |
| `metadata` | a table: it travels in the `.rvr`, and one value per sample becomes a sample column |

A mass point's label becomes part of the name as CMS dataset names write numbers: `.` becomes
`p`, anything else a name cannot hold becomes `_` (`300/292.5` → `300_292p5`).

**Metadata is each sample's own.** A key one sample has, another need not have. A recipe reads it
as `sample.KEY`; for a sample without the key it is missing, and no comparison with it is true. A
number or text that some samples lack is still a sample column, `null` for those samples (a whole
number then as `f8`, a float, since only `f8` has a `null`):

```text
xsec           f8 by sample[2]  data: null, mc: 88.29
```

A true-or-false value has no `null`. Give a flag to every sample in `[defaults]`, and set it only
where it differs:

```toml
[defaults]
tree = "Events"
metadata = { isData = false }

[[sample]]
name = "data"
files = "{eos}/SingleMuon_Run2018D/*.root"
metadata = { isData = true }  # every other sample keeps the default, false
```

It is then a sample column, and `if(sample.isData, …)` works for every sample:

```text
isData         b1 by sample[2]  data: true, mc: false
```

Set on the data samples alone, the flag stays in the file's metadata but is not a sample column,
and `if(sample.isData, …)` stops the press at a sample without it (`sample.isData is used as the
condition of if, but this sample has no isData: …`); `if(sample.isData == true, …)` works, taking a sample without it as false. A text field (`metadata = { kind = "data" }` on the data samples,
read as `sample.kind == 'data'`) needs no default.

## recipe.toml

The recipe is TOML, with one expression per value, evaluated over whole columns of events. The
[recipe reference](https://crates.io/crates/rvr-recipe/15.0.0) lists every table, operator and function, each with an
example and its result.

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

Every branch is named in full. A defined name may use the names above it, and may not be a
branch's name. A derived column uses only what the file holds: the stored columns and the derived
columns above it. A cut can apply to some samples only: `hem = { if = "sample.era == 3", apply =
"hem_veto" }`, where `sample.NAME` is a sample's metadata. A total is the sum over every event a
sample read, before the cuts, which a file of the events kept cannot give afterwards: the sum of
`genWeight` that normalizes simulation. Each file's total is in its record, and each sample's in
its metadata, a sample column a derived column reads as `sample.NAME`. `{ value = "genEventSumw",
tree = "Runs" }` sums over every entry of another tree of each file instead, for a skim whose
events were filtered before, and is refused for a sample that reads only part of its files. A
change to `[totals]` presses every event again. Every recipe writes all five tables, even the
ones it leaves empty; one that leaves a table out is refused. At a terminal, `rvr press` and
`rvr try` ask first: answered y, they write the missing tables in, empty and in their places,
check that the recipe reads as before, and go on. (Not when `[columns]` is missing or stores nothing,
which only the recipe can fix, nor for `--fill`, which takes the recipe the file was pressed
from.)

## Grid access

Central datasets, LPC EOS and most CERN EOS areas need a grid proxy; CERN Open Data does not.

```sh
rvr proxy init
```

(`voms-proxy-init -voms cms -valid 192:00` makes the same proxy, where it is installed.) `rvr`
uses `X509_USER_PROXY` when it is set, otherwise `/tmp/x509up_u<your uid>`. It trusts, with no
setup, the public certificate authorities, CERN's, and the grid's: the 83 IGTF accredited
authorities (release 1.146, the set grid sites install as `ca-policy-lcg`), which sign the
storage of most CMS sites, FNAL's dCache among them. `X509_CERT_DIR` adds a folder of others (as
grid tools install them in `/etc/grid-security/certificates`), for an authority newer than rvr.
Without a valid proxy, local files and public storage still resolve, and every other sample says
what to run.

## When something goes wrong

| you see | what to do |
|---|---|
| `… matched no file (81 files in that folder). Did you mean: …?` | fix the file name or pattern; the suggestion is the nearest name in that folder |
| `… has no tree "Events". Its trees: …` | set `tree` (or `[defaults] tree`) to one of the trees listed |
| `{lcp} is not defined here. Defined: {lpc}.` | a misspelt place, or a place missing from `[places]` |
| `the dataset … has no valid files in DBS. Did you mean: …?` | the nearest datasets of that name follow; usually the version suffix |
| `… refused the listing (403). It needs a grid proxy: …` | run the command given |
| `No usable grid proxy: … Run rvr proxy init to make one.` | run `rvr proxy init` |
| `… holds your private key, and other accounts on this machine can read it` (from `proxy init`) | run the `chmod` given |
| `…: the passphrase is wrong` (from `proxy init`) | type it again; it is the one you chose when you saved the certificate |
| `… has no copy that can be read now (T1_…_Tape: on tape; …)` (a `tape` hole, from `press`) | the file is only on tape: ask for a Rucio rule that puts the dataset on disk, then `rvr press --fill` |
| `passed over … invalid peer certificate: UnknownIssuer` (from `press`) | an authority newer than rvr's: set `X509_CERT_DIR` to the grid authorities folder (`/etc/grid-security/certificates` where grid tools are installed) |
| `… no storage has answered, and none that answered earlier in this press takes a connection now (…): the network seems to be gone` (from `press`) | the network went: press again once it is back, which carries on from the files done |
| `waiting 30 s to ask again for … its storage answered 403 Forbidden` (from `press`) | nothing: the storage is busy and the read is asked again; if all eight tries fail it is a hole (`refused` for a `403` that stays, `unreachable` otherwise): `rvr press --fill` later |
| `Wrote … with 2 holes:` (exit status 3, from `press`) | each hole says why; `rvr press --fill` reads them again once they can be read |
| `… refused the listing (403, 4 tries over 50 s). Either the grid proxy does not give access there, or the storage is refusing requests while it is busy …` | `rvr proxy` shows whether the proxy is valid; if it is, try again later |
| `…, tree T, entries A to B: X: decompression failed (zlib): …, in basket N of branch X (…)` (a `damaged` hole, from `press`) | the file is damaged at that basket: put another copy in its place if there is one, resolve again, and `rvr press --fill` |
| `rvr press would press every column, as it was made by another build of rvr (…)` (from `status` or `press`) | a full press, by design: columns made by another build are not copied |
| `a name holds only letters, digits, _ and - …` | use the name it suggests, and put the text in `label` |
| `1 file differs from the lock: …` (from `check`) | the file changed since the lock: resolve again if that is intended, and review the new lock |
| `samples.toml has changed since the lock was written` | resolve again |
| `… has changed since samples.lock was written …` (a `changed` hole, from `press`) | as for `check`: resolve again if the change is wanted, review the new lock, and `rvr press --fill` |
| `… has a samples.lock but no samples.toml` (from `press`) | put samples.toml back: the file carries both |
| `This file is rowver 14; this reader reads rowver 15 only.` | press its study again with `rvr press` |
| `The file was made by rvr …, newer than this one (…), and may use what only a newer version reads` | update rvr and rowver to that version or later; its stored columns still read meanwhile |
| `The header changed after it was signed` | the file was edited after it was pressed: press it again |

MIT licensed.
