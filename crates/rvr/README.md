# rvr

The command for RowVer studies: start a study, say what it reads, see exactly what it found, and
look inside the `.rvr` files it makes.

```sh
rvr new DIR                 # start a study
rvr proxy init              # make a grid proxy from your certificate (for grid storage)
rvr samples resolve …       # find every file the study reads
rvr branches                # the branches and collections a sample's file holds
rvr try "EXPRESSION"        # what an expression gives on the first events, before pressing
rvr press                   # read them through the recipe into one .rvr file
rvr status                  # is the .rvr current, and what would press read
rvr validate / dump / diff  # check and inspect .rvr files
rvr reproduce FILE          # press a file's own study again and compare
```

New to `.rvr` files? [The examples](https://gitlab.cern.ch/dgrove/rowver/-/blob/main/examples/README.md)
make a first one from a DAS dataset and take it, step by step, to a plot.

## Install

Download it from the [releases](https://gitlab.cern.ch/dgrove/rowver/-/releases): one file, no
Rust needed. The Linux one runs on any x86_64 Linux with glibc 2.28 or later (el8 and el9, so the
LPC and lxplus; Ubuntu 20.04 on); the macOS one on Apple silicon and Intel. For example on the LPC:

```sh
mkdir -p ~/bin && cd ~/bin
curl -sL https://gitlab.cern.ch/api/v4/projects/dgrove%2Frowver/packages/generic/rvr/14.3.0/rvr-14.3.0-x86_64-linux.tar.gz | tar xz
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
sample         i4[2248]  [0, 0, 0, 0, 0, ...]
entry          i8[2248]  [29, 45, 140, 157, 162, ...]
run            i4[2248]  [1, 1, 1, 1, 1, ...]
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
The headers show no difference in rows, samples, columns, by_sample or sample columns.
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
108.0 MB; on local copies of the two files it takes 0.2 s instead of 0.6 s (2026-10-19).
A column counts as changed when its expression, its stored kind, or any object, helper or
collection it uses changed. When the file cannot be built on, `rvr press` says why and presses
every column; `--full` always does. A sample added or removed, a lock resolved again, other
`--events`, or another build of rvr all mean a full press, since the file's rows or the code that
computed them would no longer be the ones it records.

Before a file is read, it is checked against the lock: its size and adler32, then its ROOT UUID
and entries. A file that changed is not read: it is a hole (below), and the file says so. A
`samples.toml` edited since the lock was written stops the press: resolve again and review the new
lock. Nothing is written until every file has been read or found to be a hole: the file appears
whole, or not at all.

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
are not failing now (another storage, or Rucio), or, when every one fails, to all of them if they
are at two sites or more. One that connects shows the network works: only the failing storage is down, and
its files become `unreachable` holes after their waits. If none connects, twice 15 seconds apart,
the network is gone: the press stops and says so, and pressing again once it is back carries on.
A name that does not resolve is waited on the same way. A press that reads one site and no
dataset has nothing to tell the two apart by, and a file that keeps failing is a hole.

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
and at a CMS storage door are kept, and Rucio must give the size and adler32 the lock has. Each
file's first question (its size and adler32) goes at once to the storage that answered fastest so
far and to two not yet measured. The first to answer with the file the lock names (its size, and
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
the same press took 148.8 s. rvr 0.2.0 passed over eight of the dataset's storages, whose
certificates are signed by grid authorities it did not carry (given them through `X509_CERT_DIR`,
FNAL served 7 of the files and the press took 37.5 s). Since 14.3.0 rvr carries them (see Grid
access): on 2026-10-29 the same dataset, through a recipe reading 419 MB of it, took 29.5 s, FNAL
serving 5 files.

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
flat.rvr: pressed 2026-10-30T05:41:02Z by rvr 14.3.0
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
flat.rvr: pressed 2026-10-30T04:06:48Z by rvr 14.3.0
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

Lists every branch of the first file of the study's first sample (or of `--sample NAME`), one per
line with its type and its shape: `flat` (one value per event), `jagged` (a list per event), or `3
per event` for a fixed array. Branches are grouped by the prefix before their first `_`, and
a group of jagged branches is a **collection**: under `Jet:`, its counter `nJet` comes first, then
`Jet_pt` and the rest (write `from = "Jet"`, `count(Jet)`, `Jet where pt > 30`, and `pt` inside its
`where` for `Jet_pt`). A member of a split object is grouped under its object, the name before its
first `.`: in MiniAOD and AOD, one EDM product (`recoMuons_muons__RECO`, its count `.obj` first);
in ATLAS DAOD_PHYSLITE, one container (`AnalysisJetsAuxDyn`). A recipe names those branches through
a `[collection]` (`pt = "recoMuons_muons__RECO.obj.m_state.p4Polar_.fCoordinates.fPt"`) or
`branch('…')`. Branches in no group are listed last, under `Other:`, and branches `rvr` cannot read
(whole objects, strings, nested vectors) under `Not readable by rvr:`, each with why. A dataset's
file with no copy on disk (only on tape) is passed over for the next one, up to the tenth. It reads
the file's description only, no events, so it takes as long as opening the file. `--find TEXT`
shows only the branches whose name holds TEXT, in any case: `rvr branches --find muon`. A name a
recipe uses that the file does not have is answered with the nearest branches and collections, and
a pointer here. Groups longer than seven are cut with `…` below:

```text
$ rvr branches
GluGluToHToTauTau: GluGluToHToTauTau.root, tree Events, 69 branches

GenPart:
  nGenPart        uint32  flat
  GenPart_eta     float   jagged
  GenPart_mass    float   jagged
  GenPart_pdgId   int32   jagged
  GenPart_phi     float   jagged
  GenPart_pt      float   jagged
  GenPart_status  int32   jagged

Jet:
  nJet      uint32  flat
  Jet_btag  float   jagged
  Jet_eta   float   jagged
  Jet_mass  float   jagged
  Jet_phi   float   jagged
  Jet_pt    float   jagged
  Jet_puId  bool    jagged

Muon:
  nMuon                uint32  flat
  Muon_charge          int32   jagged
  Muon_dxy             float   jagged
  Muon_dxyErr          float   jagged
  Muon_dz              float   jagged
  …

Tau:
  nTau                 uint32  flat
  Tau_charge           int32   jagged
  Tau_decayMode        int32   jagged
  Tau_eta              float   jagged
  Tau_genPartIdx       int32   jagged
  …

HLT:
  HLT_IsoMu17_eta2p1_LooseIsoPFTau20  bool  flat
  HLT_IsoMu24                         bool  flat
  HLT_IsoMu24_eta2p1                  bool  flat

MET:
  MET_CovXX         float  flat
  MET_CovXY         float  flat
  MET_CovYY         float  flat
  MET_phi           float  flat
  MET_pt            float  flat
  MET_significance  float  flat
  MET_sumet         float  flat

PV:
  PV_npvs  int32  flat
  PV_x     float  flat
  PV_y     float  flat
  PV_z     float  flat

Other:
  event            uint64  flat
  luminosityBlock  uint32  flat
  run              int32   flat
```

### `rvr try "EXPRESSION" [DIR] [--events N] [--sample NAME] [--kept]`

Shows what one expression gives, event by event, on the first `--events` events (10 unless told)
of the study's first sample, or of `--sample NAME`. It reads the study's `samples.lock`, so resolve
first, and its `recipe.toml`, so the recipe's objects, helpers and collections can be used. It
opens one file and reads only the branches the expression names. `--kept` shows only the events
the `[keep]` cuts keep, which are the rows `rvr press` would write. A mistake in the expression, or
a branch the file does not have, is said as `rvr press` would say it.

```text
$ rvr try "first(pair_mass(good_muon.pt, good_muon.eta, good_muon.phi, good_muon.mass), -1)" --kept --events 5
Rucio said where the 1 dataset file can be read in 3.2 s.
DY: 13D0AD97-6B32-CB4C-BA87-5E37BA4CF20E.root, tree Events (f4, one value per event)
  entry       14   88.6032
  entry       25   93.59802
  entry       35   90.682175
  entry       43   90.26609
  entry       52   92.6495
```

### `rvr validate FILE... [--parent PARENT] [--complete]`

Checks each file completely: every array decompresses to its declared length and checksum, every
per-sample checksum, the column facts, the study's texts and their checksums, that the rows belong
to the files recorded, and that every row's entry was read (a random choice is made again from its
seed). One line per file: `valid`, how many entries the press read of how many its samples hold
(`read all 968616 entries`, so you can see nothing was left out), its holes by kind
(`read 5 of 10 entries, 1 hole (damaged)`), and who signed it, or the first problem found.
`--complete` also fails (exit 1, on its line) a valid file that has holes. A derived file
(columns computed from another file) is compared with its parent when you name it with `--parent`.

Every reader (rvr, the Python package) refuses a file whose header fails these checks, or whose
signature no longer holds; `rvr dump` still shows it.

### `rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns]`

Prints the inside of a file: the header in full, the texts of recipe.toml, samples.toml and
samples.lock, and every column with its kind, length and first values. Each flag prints only that
part: `--files` the files read and each sample's event choice and metadata, `--recipe`,
`--samples` and `--lock` one text each, `--columns` the columns.

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

The recipe is TOML, with one expression per value, evaluated over whole columns of events. The
[recipe reference](https://crates.io/crates/rvr-recipe/14.3.0) lists every table, operator and function, each with an
example and its result.

```toml
format = "rvr-recipe"
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

Also `[helper.NAME]` for named expressions you reuse, `[collection.NAME]` to group branches as one
collection's attributes, and `[column]` for computed columns, written after the copies:

```toml
[column]
ht = "sum(Jet_pt)"            # one number per event
```

A cut can apply to some samples only: `hem = { if = "sample.era == 3", apply = "hem_veto()" }`,
where `sample.NAME` is a sample's metadata.

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
| `This file is rowver 13; this reader reads rowver 14 only.` | press its study again with `rvr press` |
| `The header changed after it was signed` | the file was edited after it was pressed: press it again |

MIT licensed.
