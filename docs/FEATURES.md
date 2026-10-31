# RowVer: a self-describing columnar format for reproducible event selection

*The features of RowVer 15.0 (`.rvr` format version 15).*

## Abstract

A high-energy physics analysis typically reads its ROOT ntuples again on every iteration: each
new binning, cut or plot opens, decompresses and closes the same files, often split into chunks
across a batch system. RowVer does the reverse. A *press* reads the inputs once and keeps only
the events a declared selection passes and the columns a declared recipe names. It writes them to
one compressed, columnar `.rvr` file. The file carries the complete text of the study that made
it, a lock pinning every input, a reproducible record of which entries were read, SHA-256
checksums of every array (globally and per sample), and optionally an Ed25519 signature. When an
input cannot be read, the press records it as a typed *hole* and goes on. An interrupted press
resumes from a journal, and before giving up on unreadable data the press checks whether one
storage is busy or the whole network is gone. On a 30-file benchmark, reading the stored columns
from the pressed file is about 350 times faster than reading them again from the ROOT files. This
document describes the format, the tools around it, and the measurements behind its design.

## 1. Introduction

Opening and reading ROOT files takes most of the wall time of many analyses. The usual remedy is
to split datasets into chunks and process each chunk on a batch system. That spreads the cost out
but does not remove it: every chunk is opened, read and closed on every iteration, and finer
splitting adds overhead. RowVer takes the opposite approach. The idea is not new, but it is rarely
used: *merge instead of split*. A study's inputs are read once into one file that holds exactly
the events and branches the analysis uses. Every later iteration opens that one file, on the
analyst's own machine.

This raises two requirements. First, a file that stands in for the original ntuples must say
exactly how it was made from them; otherwise results built on it can be neither trusted nor
repeated. Second, the single read of the inputs must be robust. It often runs over the wide-area
network against grid storage that may be busy, partly down, or holding some files only on tape.

RowVer contributes:

1. a container format (§3) whose plain-JSON header states every fact about the data without
   decompressing any of it, with each array compressed on its own;
2. a hierarchy of checksums and a canonical JSON encoding (§4), which make any two files
   comparable column by column and sample by sample from their headers alone, together with a
   reproducible, checkable record of the event choice and optional signatures;
3. a declarative description of a study (§5) and a small, statically checked columnar selection
   language (§6);
4. a press (§7) with bounded memory, atomic output, incremental re-pressing, typed holes,
   resumption, and a rule that tells a storage outage from a network outage;
5. tools for inspection, comparison and reproduction (§8), derived columns (§9), and readers for
   Python and Rust (§10).

## 2. Design principles

- **One study, one file.** Every sample of a study lives in one file, each sample in a contiguous
  block of rows.
- **The file states how it was made.** The recipe, the sample description and the input lock are
  inside the file, word for word. Nobody edits a file by hand: you change the recipe and press
  again.
- **Store what is used.** A file holds the columns of an analysis, not a copy of a dataset.
  Another branch comes in through the recipe, and only what is new is read (§7.3).
- **No consumer is privileged.** The format and the tools know nothing of the programs that read
  the files. A consumer that needs to keep state in a file uses its own top-level key (§3.5), and
  every writer preserves it.
- **Leaving things out on purpose.** The format leaves out stored event weights, per-column
  provenance beyond the recipe text, percentile tables, per-row sample metadata, and reading of
  older versions. A weight is an ordinary column, and a quantile is measured by whoever holds the
  column.
- **Every failure is explained.** Each error says in plain words what went wrong and how to fix
  it, often naming the nearest valid alternative (a branch, a dataset version, a file name).

## 3. The `.rvr` container

[The format](FORMAT.md) specifies every byte, shown on a real file; this section and the
next summarise it.

### 3.1 Layout

```
magic (8 bytes) | header length (u32, little-endian) | JSON header (UTF-8) | arrays
```

The magic `89 52 56 52 0D 0A 1A 0A` follows the PNG convention. Its high byte and its CR LF,
Ctrl-Z, LF sequence mean that a transfer which strips the eighth bit or rewrites line endings
breaks the file visibly. The header is not compressed, so one parse gives a reader every fact
about the file. The arrays follow with no gap between them and nothing after the last. Each array
is compressed on its own, so a reader decompresses only what it needs. Array offsets (`at`) count
from the first byte after the header. A program that changes only the header can therefore write
the new header and copy the arrays byte for byte.

The top level of the header holds exactly `rowver` (the version, 14), `data`, `made`, an optional
`derived` and `signature`, and any number of tool keys.

### 3.2 The row model

One row is one kept event. Each sample's rows are contiguous and follow the sample order. Within
a sample, each input file's rows are contiguous and follow the lock's file order, however many
files were read at once. The same study on the same inputs therefore always gives the same
layout. Two arrays are always present: `sample` (`i4`, each row's sample index) and `entry`
(`i8`, each row's entry number in its file's tree). Together with the list of files, they tie
every row to one event of the original ntuple. A later program can thus read more branches for the
same rows without running the selection again.

### 3.3 Columns

A column is either flat (one value per row) or jagged (a list per row). The flat kinds are `f4`,
`f8`, signed and unsigned integers of 1, 2, 4 and 8 bytes, and `b1` (one byte per boolean). A
jagged column is two arrays: `offsets` (`i8[rows + 1]`) and `content` of any flat kind, where row
`i` owns `content[offsets[i] : offsets[i+1]]`. A copied branch keeps its type, and a computed
column has the type its expression gives. There is no promotion rule.

The press computes *column facts* for each flat column: `lim` (the minimum and maximum) and
`nice` (that range rounded outward to two significant digits, ready for an axis). Integer and
boolean columns with at most 64 distinct values also get `vals` (the distinct values, sorted) and
`names` (the recipe's labels for them). With these, a consumer can build axes and category
legends without reading any data.

### 3.4 Sample columns

Some quantities are the same for every event of a sample, such as a cross-section, an era code or
a data/simulation flag. These are stored once per sample instead of once per row. A metadata key
becomes a *sample column* when its values across the samples are all booleans, all integers, all
numbers or all text; its kind is then `b1`, `i8`, `f8` or `text`. A sample that lacks the key
gets `null`, which only `f8` and `text` allow. [The format](FORMAT.md#5-sample-columns)
states the typing rule exactly.
Each sample column carries a checksum of its values as bytes, so the checksum does not depend on
how a program writes a number in JSON. The press adds `events_read` and `events_total` to every
sample, and `events_missing` when the file has holes (§7.4), so normalisation can be done from the
file alone.

### 3.5 The `made` record and extension

`made` records the study:

- its name and title, and when it was built;
- a `toolchain` naming the `rvr` version, the format version, the git commit (with a hash of the
  uncommitted changes when the build was dirty), the compiler and the target platform;
- the complete text of `recipe.toml`, `samples.toml` and `samples.lock`, each with its SHA-256;
- for every sample, its event choice (§4.2) and its metadata;
- for every file read, its tree, the ROOT UUID stored inside it, its number of entries, how much
  of it the choice read, the storage element that served it (for catalog datasets), and its
  holes.

Tool keys are how the format is extended. Each tool gets one top-level object, outside every
checksum and the signature, and every program that saves a file writes them back. New consumer
state therefore needs no change to the specification.

### 3.6 Array encoding

Each array is stored in three steps. Its values are written little-endian. When the kind is wider
than one byte, the bytes are shuffled: every value's first byte, then every value's second byte,
and so on, which puts the slowly changing bytes of neighbouring values together. The result is
then compressed as one Zstandard frame [RFC 8878]. Both steps run in pure Rust, and reading a
column takes one read and one decode.

## 4. Integrity, provenance and reproducibility

### 4.1 A hierarchy of checksums

- Every array carries the SHA-256 of its uncompressed bytes.
- `data.sha256` hashes the concatenated array checksums in file order. It fingerprints all the
  data without decompressing anything.
- Every per-row array also carries `by_sample`: one SHA-256 per sample, over that sample's slice.
  For jagged offsets the slice is shifted to start at zero, so a change in one sample never
  changes another sample's checksum.
- `made.sha256` hashes a canonical JSON encoding defined in
  [the format](FORMAT.md#canonical-json). Keys are sorted by code point, string escapes are
  fixed, and there is no whitespace. Integers are written as given and never turned into
  doubles, so a 64-bit seed is checksummed exactly. Every other number is written as the shortest decimal that reads back as
  the same double.

Because the per-sample checksums are in the header, `rvr diff` can say from two headers alone
which column of which sample differs between two presses. It also reports differences in rows,
samples, kinds and sample-column values.

### 4.2 A checkable event choice

Each sample records which of its entries were read, in one of three ways. *All* means every
entry. *First N* means the first N entries in file order, and each file records where it stopped.
*Random* means N entries chosen at random from a seed. The random choice is fully specified: the
files count as one run of entries, and a SplitMix64 generator [Steele et al. 2014] seeded with S
drives Knuth's selection sampling, Algorithm S [Knuth 1997, §3.4.2], to choose min(N, total)
entries. Each file records how many of the chosen entries fall in it. A reader makes the choice
again from the seed and checks every row's entry against it. A random choice is spread over every
file of a sample. This avoids the bias of reading only the earliest runs of data, or only the
first production batch of a simulation.

### 4.3 Signatures

Once the person pressing has made a key (`rvr key new`), every press signs the file. The press
takes the SHA-256 of the canonical JSON of `{rowver, data, made, derived}`, signs it with Ed25519
[RFC 8032], and stores the signature and the public key. Readers refuse a file whose header
changed after it was signed. Whether to trust a key is a local decision: `rvr validate` names the
signer from the reader's own list of trusted keys. Tool keys are outside the signature, so
consumer state can change without breaking it.

### 4.4 Two levels of checking

Opening a file checks everything its header says without reading any array:

- the magic and the version;
- every field of `made`, each study text against its checksum, and `made.sha256`;
- that `data.samples` and `data.files` agree with `made.files`, including each sample's event
  choice;
- the structure of the holes;
- that the columns agree with the recipe;
- that the sample columns and the metadata agree, in both directions;
- `data.sha256`, and the signature.

Every reader refuses a file that fails any of these checks. Each column is also checked against
its checksum when it is first read.

`rvr validate` goes further. It decompresses every array and checks its length and checksum, and
every `by_sample`. It checks that the rows add up and belong to the recorded files, that every
entry is one the choice read and none lies in a hole, that jagged offsets never decrease, that
floats are finite, and that the column facts are right. It prints one line per file and names the
signer. `rvr dump` still shows a file that fails these checks, after saying what is wrong, so a
damaged file can be looked into.

### 4.5 Reproduction

A file contains its study, so `rvr reproduce FILE` can write the three texts back out, press them
with the same event choice, and compare the result with `rvr diff`. Exit status 0 means the
reproduced file holds the same data.

The same study gives back a file's space without losing the file. `rvr empty FILE` keeps the header
byte for byte, with the study, every checksum and the signature, and lets the arrays go: on the
2.1 M-row bench, a 39.3 MB file keeps a 114 kB header (0.29%). It first checks that every input the
lock names is as the lock recorded it, since only the inputs hold the events, and asks before it
empties (`--force`, or `-f`, does not ask). `rvr refill FILE`
presses the study again and puts the arrays back once each has the checksum its record states.
Pressed twice, the bench gives byte-identical arrays, whatever the number of workers, so a refill
by the same rvr gives back the file it was, byte for byte, and its signature still holds; another
rvr that compresses otherwise gives the same values in other bytes, placed anew and signed again.
An empty file reads as any other but for its columns, and says it is empty.

## 5. Describing a study

A study is a folder with two files the analyst writes and one the tool writes:

| file | contents | written by |
|---|---|---|
| `samples.toml` | what to read: datasets, storage folders, local files, patterns, eras | the analyst |
| `recipe.toml` | which events to keep, and what to store of each | the analyst |
| `samples.lock` | every input found: where it is, its size, adler32, ROOT UUID and entries | `rvr samples resolve` |

`rvr new DIR` writes a study that works as it is, as a starting point: two CMS Open Data samples
that need no credentials. It never writes over an existing file.

### 5.1 Samples

A sample's source is one of three things: a central CMS dataset (`dataset = "/…/NANOAODSIM"`),
files on grid storage (`root://`, `davs://` or `https://`, with `*` and `?` allowed in the file
name), or local paths. `[places]` gives short names to long locations, and `[defaults]` gives keys
that every sample shares. Two keys make families of samples. `eras` makes one sample per era; the
era's own keys become metadata, and `{era}` can be used in paths. `trees` makes one sample per
tree, as for the mass points of a signal scan, and names each sample from its label by a fixed
rule (`300/292.5` becomes `300_292p5`). Each sample can set how many entries to read, and whether
to take the first ones or choose them at random from a seed. Any other key is metadata: it travels
into the file, and the recipe reads it as `sample.KEY`. `rvr samples expand` prints the expanded
samples without asking any catalog or storage.

### 5.2 Resolution and pinning

`rvr samples resolve` finds every input without reading a single event:

- for a dataset, it asks DBS and Rucio;
- for grid storage, it asks the storage's WebDAV door: one listing per folder, one checksum
  request per file, and three small reads per file for its UUID and entry count;
- for a local file, it reads the file directly.

The 307 files of a 353-sample study resolve in about 8 s. If any sample fails, no lock is written,
and each failure is reported with a suggestion where there is one. Resolve also lists any `.root`
files in the folders it read that no sample uses, as a check that none was left out. The lock is
meant to be committed and reviewed like code.

`rvr samples check` asks again for every file's size and adler32, and for the checksum of the
`samples.toml` the lock came from. `rvr samples plan` says how many entries a press would read
from each sample, and where it would stop, from the lock alone in under a millisecond. Before it
reads a file, the press checks it against the lock: first its size and adler32, then its UUID and
entries. The press refuses to run at all if `samples.toml` was edited after the lock was written.

## 6. The selection language

### 6.1 Structure

`recipe.toml` is TOML, and its values are expressions. It names the language and its version
(`format = "rvr-recipe"`, `version = 1`) and has five tables:

| table | role |
|---|---|
| `[define]` | names for expressions, which the lines below use as if they were branches; none is stored |
| `[keep]` | the cuts, in order; an event is kept when every one is true, and the press reports the count after each (a cutflow) |
| `[columns]` | what the file stores: `branches = [...]` copied as they are, and `NAME = "expression"` computed while pressing, optionally with a declared storage kind |
| `[totals]` | `NAME = "expression"` summed over every event read, before the cuts, one number per sample (or over another tree of each file, `{ value = "...", tree = "Runs" }`) |
| `[derived]` | `NAME = "expression"` over the stored columns: kept in the file as its expression and computed when it is read (§9) |

Every branch is named in full, as the file spells it, so the language makes no guess about how an
experiment names things: `Jet_pt` in CMS NanoAOD, `PT_jet` in an analysis ntuple, `lep_pt` in
ATLAS Open Data and `H1_PX` in LHCb's all read the same way. A **mask** picks members of a list:
`Muon_pt[good_muon]`, with `good_muon` defined as a true-or-false list beside the muons, keeps the
good muons' pt, and the same mask picks from every other muon branch. A defined name may use the
names above it, and may not be the name of a branch, so each name means one thing. A branch whose
name is not a plain word, such as a member of a split object in MiniAOD or ATLAS DAOD_PHYSLITE, is
named through `branch('…')`. A cut can apply to some samples only: with `{ if = "sample.era ==
3", apply = "hem_veto" }`, every other sample passes the cut. Every recipe writes all five tables,
even empty: one that leaves a table out is refused, so that each says what it does not use. At a
terminal, `rvr press` and `rvr try` offer to write the missing tables in, empty, and go on, once
`[columns]` stores something.

A file keeps only the events that pass the cuts, yet normalizing simulation needs a sum over every
event: each event's weight is `xsec * lumi * genWeight / sum(genWeight)`, the sum over all the
events the sample read. A **total** is that sum, taken by the press as it reads, before the cuts:
`sumw = "genWeight"` under `[totals]`. Each file's total is kept in its record and each sample's
in its metadata, so it is a sample column, and a derived column can divide by it:
`weight = "genWeight * sample.xsec * sample.lumi / sample.sumw"`. A skim whose events were filtered
before cannot give the sum itself; a total can then read another tree of each file, as NanoAOD's
`Runs` with `genEventSumw`, for a sample that reads its files whole. A file's total is one running
sum over its events in entry order, and a sample's adds its files' in the lock's order, so neither
depends on how many files were read at once, or how; building on a file keeps its totals, and a
`--fill` adds the sum of the entries it read to the file's, which can differ from a full press in
the last bits.

### 6.2 Semantics

Expressions are evaluated over whole columns, 50,000 events at a time, with no loop over events.
Arithmetic follows NumPy's rules, so a cut keeps exactly the events that the same formula keeps in
NumPy or Awkward. When a flat value meets a jagged one, the flat value is used for every member.
The type rules are explicit: a comparison gives `b1`, a count gives `i8`, a sum of integers gives
`i8`, and floating-point functions keep the float kind they were given. Missing metadata has a
defined meaning. A comparison with a field the sample lacks is false (even `!=`), and arithmetic
with it is an error. A conditional cut lets such a sample through.

### 6.3 Static checking and minimal reads

The whole recipe is parsed and checked before any input is opened. A syntax error, a wrong number
of arguments, a defined name used above the line that defines it, or an unknown top-level key is
reported at once, with its position, and not an hour into a press. A branch name the input does
not have is answered with the nearest branches. Only the branches an expression names are read,
and a defined name is worked out once per run of events, however often it is used.

### 6.4 The function library

| family | functions |
|---|---|
| reductions over a list | `count`, `sum`, `any`, `all`, `least`, `greatest`, `least_index`, `greatest_index`, `first`, `nth` |
| positions and order | `index`, `sorted`, `order`, `take`, `listed` |
| joining lists | `join`, as electrons and muons into one list of leptons |
| value-by-value mathematics | `abs`, `sign`, `sqrt`, `exp`, `log`, `log10`, `cos`, `sin`, `tan`, `acos`, `asin`, `atan`, `atan2`, `cosh`, `sinh`, `tanh`, `hypot`, `floor`, `ceil`, `round`, `min`, `max`, `clip`, `isfinite`, `if` |
| bits | `bit_and`, `shift_right` |
| kinematics | `delta_phi`, `delta_r`, value by value |
| a list's sum of four-momenta | `mass`, `pt`, `eta` (the pseudorapidity), `phi`, `rapidity` |
| matching between two lists | `dr_nearest`, `dr_second_nearest`, `dr_farthest`, `dr_second_farthest`, `nearest_index`, `second_nearest_index`, `farthest_index`, `second_farthest_index` |
| pairs within a list | `pair_first`, `pair_second`, `pair_dr`, `pair_mass`, `pair_pt`, `pair_eta` (pseudorapidity), `pair_phi`, `pair_rapidity` |
| text, branches, types | `contains`, `branch`, `stored` |

A reduction that may meet an empty list takes an explicit `otherwise` value, so it never gives a
null. A selection that takes several lines of Awkward is often one line here. Cleaning jets
against muons within ΔR 0.4, for example, is
`Jet_pt[dr_nearest(Jet_eta, Jet_phi, Muon_eta, Muon_phi) > 0.4]`.

### 6.5 Tested documentation

A test evaluates every example in the language reference on three fixed events. Another runs
every Python/Awkward equivalent shown beside an rvr expression and compares the two. The reference
therefore cannot drift from the implementation.

### 6.6 Exploring before pressing

`rvr branches` lists the branches of a sample's first file in alphabetical order, with each
branch's type and shape (flat, jagged, or a fixed number per event). Branches rvr cannot read are
listed apart, each with the reason. `--find` filters the list by name. `rvr try "EXPRESSION"`
evaluates one expression, with the recipe's defined names, on the first events of a sample. With `--kept` it shows only the events that
pass the cuts, which are exactly the rows a press would write.

## 7. The press

`rvr press` reads what the lock names, applies the cuts, and writes the file.

### 7.1 Execution

Files are read in parallel, one per core by default (`--workers`), and only the branches the
recipe uses are fetched. Remote files are read through the storage's HTTPS door. For each block of
50,000 entries, every basket the recipe can use is fetched in one `multipart/byteranges` request.
A random choice reads only around its chosen entries: a block of 50,000 entries (2,000 for a local
file) with none chosen is skipped, and a file with none chosen is not opened. The row layout does
not depend on the number of workers (§3.2).

### 7.2 Bounded memory and atomic output

As each file is read, its kept values go to a spool on disk, compressed where that pays. Each
column is then packed from the spool a piece at a time. Memory therefore does not grow with the
number of kept events: pressing 12 million rows of 31 columns into a 1.5 GB file peaks at 0.3 GB,
where holding every row took 4.9 GB. The cost is free disk space of about twice the output.
Nothing is put in place until every file has been read or recorded as a hole, so the output
appears whole or not at all.

### 7.3 Incremental pressing

Suppose the output already exists, and was pressed from the same `samples.toml` and lock, with
the same cuts, the same event choice and the same build of `rvr`. A new press then reads only the
columns that are new or whose definition changed (their expression, their stored kind, or any
defined name they use), and only for the events the file already kept. Every
other column is copied over, and the cuts are not run again. The result is the same file a full
press writes, as `rvr diff` confirms. Adding one jagged column to a two-sample study read 26.3 MB
of baskets instead of 108.0 MB, in 0.2 s instead of 0.6 s.

Adding a sample works the same way. When the columns and cuts are the same, and every sample the
file holds is in the new lock as it was (its files, tree, metadata and event choice), only the new
samples are read, and the others' rows are taken from the file. A sample taken out, or the samples
reordered, reads nothing. The rows are laid out again, so every column is packed again: an array
is compressed whole, so a sample's part of it cannot be copied as it lies. On 2.1 M rows, adding a
sample of 30 files to ten of 30 took 1.5 s, against 4.9 s for a full press; taking one out took
0.75 s against 4.5 s. Any other change to the rows, or to the code that computed them, means a
full press, and the press says why. Such changes include a sample whose files changed, other
cuts, other events, or another build.

### 7.4 Holes

An input the press cannot read does not stop the press. The file is written without those events,
and records each *hole*: a range of entries, a kind from a closed list, and a sentence giving the
cause and the fix.

| kind | cause |
|---|---|
| `tape` | every copy of a dataset's file is on tape only |
| `unreachable` | its storage kept failing after every wait |
| `damaged` | a basket failed to decompress, or the file's bytes could not be read |
| `gone` | the file no longer exists |
| `changed` | the file differs from the lock |
| `refused` | its storage refuses the file, even with a valid proxy |

A damaged basket costs only the 50,000-entry block read with it, and the rest of the file is still
read. Every other kind makes a hole of the rest of the file. Some failures are not the file's own:
an error in the recipe, a missing or refused proxy, an untrusted certificate, a problem with
permissions or a mount. These stop the press, since every file would fail the same way. A press
that leaves holes exits with status 3, and every sample gains `events_missing`. Because the list
of kinds is closed (a new kind means a new format version), tools can act on holes by kind.

`rvr press --fill [KINDS]` reads only the holes, or only those of the kinds given, and merges
their rows into each file's rows by entry. A hole that fails again stays a hole, with its new kind
and reason. The lock may have been resolved again, but only for the files being filled (a file put
back where it was, or another copy of it) and for where a dataset's copies are. The filled file is
then the one a full press would write. Filling one file's hole in a 30-file study took 0.16 s,
against 0.39 s for a full press.

### 7.5 Resumption

A press is staged beside its output in `FILE.partial/`, which holds the spool, the packed arrays
and a journal of the files finished. A press can stop on Ctrl-C, a killed process, a laptop going
to sleep, or a failure that is not a file's own (a disk full or not mounted, a certificate
refused). It then leaves this folder behind, and the next press of the same study carries on from
the files it finished. The journal is made safe on disk at most every 2 s, so a stop loses at most
the last 2 s of work. That interval is a measured compromise. Syncing the journal after every file
slowed the benchmark by 7%, since a full flush on macOS takes about 4.5 ms; checkpointing every
2 s costs at most 1.5%.

A file stopped half read is read again from its start, and so is a file that became a hole of a
kind that may pass (`unreachable`, `refused`, `tape`). Holes of kind `damaged`, `gone` and
`changed` stay. A press that differs in any way (recipe, samples, lock, event choice, columns read
or build) starts over and says why. The journal is locked while a press runs, so two presses of
the same output cannot run at once. The resumed file is the same one an uninterrupted press writes.

### 7.6 Reading from grid storage

Grid storage under load refuses requests for minutes at a time, and networks drop connections. A
read that fails in such a way (`403` under load, `408`, `429`, `5xx`, a dropped connection) is
tried again after 5 s, 15 s, 30 s, 1, 2, 4 and 5 minutes, about 13 minutes in all, and every wait
is printed. If all eight tries fail, the file becomes an `unreachable` hole (`refused` for a `403`
that persists). Permanent failures are not retried: a `404` is a `gone` hole at once, and a `401`
or an untrusted certificate stops the press. A prefetch is never waited for; only the read that
needs the bytes is.

For catalog datasets, the press asks Rucio once where every file can be read over HTTPS. It keeps
only copies that are on disk, available, behind a CMS storage door, and whose size and adler32
match the lock. The three storages that answered fastest for the sample when its lock was written
(its `sites`, which ride in every file pressed from it) are tried first, and are read from, at
CMS storage doors only, when Rucio is unreachable. Each file's first request goes at once to the fastest storage so far and to
two storages not yet measured. The first to answer with the right file is used, so every storage of
the dataset gets timed as files open. A storage that has failed twice since it last served a whole
file is tried last, and a failed read moves to another copy. The file records which storage
element served each file. The press reports how many bytes it read from each site, and every
storage it passed over, with the reason.

Retries alone cannot tell one storage being down from the local network being lost. In the second
case, retrying would end up making every file an `unreachable` hole. The press therefore applies
an *outage rule*. Once no storage has answered for 2 minutes, it tries for 5 s to connect to the
hosts that answered earlier in the press and are not failing now. If every one of them is failing,
it tries all of them, and then, when they are all at one site, CERN's Rucio server for CMS as
well (only to see that it takes a connection; nothing is sent). One connection shows the network works
and only the storage is down. No connection, twice 15 s apart, means the network is gone: the
press stops, keeps what it read, and carries on when pressed again.

### 7.7 Feedback

On a terminal, the press draws one progress bar per sample, with the events kept, the read rate
and the time taken, five times a second and cut to the terminal's width. Events such as waits and
holes are printed above the bars. When the output is not a terminal, the press prints one line per
file instead. Drawing costs under 1% (0.397 s against 0.394 s on the 30-file benchmark).

## 8. Inspection and comparison

Every command exits 0 on success, 1 on failure or difference, and 2 for an unknown command. A
press that leaves holes exits 3.

| command | question it answers |
|---|---|
| `rvr status` | Does the study's file hold the study as it is now? It reports what changed in the recipe (columns new, changed or dropped; the cuts), whether `samples.toml` was edited or the lock resolved again, what a press would read, how complete each sample is, and the holes. It exits 0 only when the file is current, was pressed with the same choice, and is complete, so a script can ask before pressing. It reads no input. |
| `rvr validate` | Does the file check out in full (§4.4)? One line per file: rows, columns, derived columns (each worked out and checked), entries read of the entries the samples hold, holes by kind, and the signer. `--complete` fails a file with holes, or an empty one. |
| `rvr dump` | What is inside? The full header, the three study texts, and every column with its kind, length and first values (a derived column worked out, with its expression), or any one of these parts. |
| `rvr header` | The JSON header exactly as the file holds it, without reading any array. |
| `rvr diff A B` | Where do two files differ? From the headers alone: rows, samples, columns and derived columns, kinds, expressions, and each (column, sample) pair whose contents changed. |
| `rvr reproduce` | Does the file's own study, pressed again, give the same data (§4.5)? |
| `rvr empty`, `rvr refill` | Give back a file's space and keep its study in its place; press its columns again, checked against its checksums (§4.5). |

## 9. Derived columns

A column can be stored, an array in the file, or **derived**: kept as the expression that computes
it from the file's own columns, `ht_over_met = "ht / MET_pt"` under the recipe's `[derived]`. A
reader works a derived column out when it is first asked for; the Python package then holds it, as
it holds every column read, until `f.forget(...)` lets it go or the file object goes. The file holds
no array of it, only its expression and what the press measured of its values when it worked it out,
once: its kind, its checksums (whole and per sample) and its facts (`lim`, `nice`, `vals`). A reader
that works one out checks it against that record, so a derived column is as trustworthy as a stored
one; `rvr diff` compares it from the headers alone, and a plot gets its axis from `nice` without
computing anything. The reference reader does the work (`Reader::column` in `rowver`), and every
function of the recipe language gives the same bits on every platform: its exponentials,
logarithms, trigonometry, roundings, `hypot` and `**` come from the `libm` crate, as the platforms'
own math libraries do not agree. Measured, Apple's and musl's disagree in the last bit of 2 to 26% of double results over
the ranges physics columns take, and on the bench every one of twelve such columns came out
different; the `libm` crate costs 1.6% of a press of all twelve (6.93 s against 6.82 s, within the
run-to-run spread).

A derived expression uses only what the file holds: the stored columns, the derived columns above
it, and the sample's metadata. `Recipe::read` checks that before anything is pressed. Derived
columns are added to a pressed file, dropped from it or renamed with `rvr derive FILE`, which
first checks the derived columns it has, records only what is new, and keeps the recipe the file
carries in step, so that `rvr reproduce` still makes the same file; the study's `recipe.toml` is
left to the analyst. An expression is never changed in place: it is dropped and added again.
Changing only `[derived]` in a study reads no ROOT file: the press copies every stored column from
the earlier file and works the derived columns out from it.

The trade is space for time, the analyst's to choose column by column. On the 2.1 M-row bench (300
copies of the TChiWZ skim, Apple M5, 2026-10-31), seven typical computed columns take 70 bytes per
row stored (uncompressed; the bench's copies compress far better than real data would), 35% more
than the ten columns they are computed from. Kept as derived columns they cost nothing in the
file, 0.54 s more at press time to measure them (4.0 s against 3.5 s); a whole Python run that
works all seven out takes 0.98 s, where one that reads them stored takes 0.10 s and one that only
opens the file 0.05 s, and a simple one such as `sum(PT_jet)` takes 0.11 s.

## 10. Reading files from programs

The Python package `rowver` reads files through the same Rust reader as the `rvr` command, which it
also installs. Opening a file checks its header, and each column is read when first asked for and
then kept. A flat column comes back as a read-only NumPy array without a copy, and a jagged column
as an Awkward array. Several columns asked for together are read in parallel. A whole file converts
to an Awkward record array, a pyarrow table, a pandas data frame (with the sample as a categorical)
or Parquet. The package also exposes what the header says: the samples and their labels, the sample
columns, the recipe and study texts, the signer, and the holes with a `complete` flag. A derived
column is read as any other, `f["name"]`, worked out when first asked for and held from then on;
`f.derived` names them, and `f.forget(...)` lets columns go from memory. Only NumPy is required;
Awkward, pandas and pyarrow are used when installed.

The Rust crate `rowver` is the reference implementation of the format. `rowver::rvr` reads and
validates files, `rowver::write` builds them in memory or through a spool on disk (and measures a
derived column's record), `rowver::sign`
signs and verifies, and `rowver::choice` implements the event choice. The crate's major version is
the format version.

## 11. Grid access and deployment

`rvr proxy init` makes a VOMS proxy with no grid software installed:

- it reads the user's certificate from `~/.globus`, either as a PEM pair or as the `.p12` file a
  certification authority issues;
- it refuses a key file that other accounts can read, and gives the `chmod` that fixes it;
- it gets the user's VOMS attributes, and writes a proxy that standard tools (`voms-proxy-info`,
  OpenSSL) read and verify [RFC 3820].

`rvr proxy` shows the proxy's owner, VO and time left, and a press warns when fewer than six hours
are left. With no setup, `rvr` trusts the public web authorities, CERN's, and the 83
IGTF-accredited grid authorities (release 1.146), which sign the storage of most CMS sites.
`X509_CERT_DIR` adds others.

Every crate and the Python package share one version, whose major number is the format version,
and one tag makes a release. Each release builds the command as a single binary for macOS and for
x86_64 Linux with glibc 2.28 or later. The Linux binary is built on AlmaLinux 8, so it runs on el8
and el9 login nodes; a static musl build was measured 3.3 times slower. The command also installs
with `cargo install`, and the Python package with `pip`.

## 12. Measured performance

All times are wall times on an Apple M5 laptop in October 2026, unless noted. The *30-file study*
is 30 copies of one TChiWZ skim (212,130 events, 4.1 GB), with a recipe that keeps events with MET
above 150 GeV and two leptons, and stores 7 branches.

| measurement | result |
|---|---|
| reading the 7 branches from the 30 ROOT files with uproot, and applying the cut | 2.1 s, on every iteration |
| pressing the same study with `rvr press` | 0.4 s, once |
| opening the `.rvr` and reading all 7 columns | 6 ms, about 350 times faster than the ROOT loop |
| one 136 MB input file (285 branches in its main tree), pressed | 157 kB |
| incremental press adding one column (two Open Data samples, local copies) | 26.3 MB of baskets in 0.2 s, against 108.0 MB in 0.6 s |
| `--fill` of one file's hole in the 30-file study | 0.16 s, against 0.39 s for a full press |
| journal and checkpoints | at most 1.5% (0.395 s with, 0.397 s without; 90 files on one worker: 5.655 s with, 5.609 s without) |
| progress display | under 1% (0.397 s with, 0.394 s without) |
| seven computed columns on the 2.1 M-row bench: stored, or kept as derived columns | stored: 70 bytes per row (uncompressed), a Python run that reads them 0.10 s; derived: no bytes, 0.54 s more to press, a Python run that works them out 0.98 s |
| 12 M rows of 31 columns, 1.5 GB output, 4 workers | 0.3 GB peak memory, against 4.9 GB when holding every row |
| resolving 307 files of a 353-sample study | about 8 s |
| the whole WZTo3LNu UL18 NanoAOD dataset (9.8 M entries, 18 files, 14.9 GB on storage), read over the wide-area network | 41.4 s with 10 workers and 148.8 s with one (2026-10-19, 291 MB of branches); 29.5 s with 10 workers (2026-10-29, 419 MB of branches) |

The last row depends on the network and the storage sites, and is given as an illustration only.
On 2026-10-19 rvr did not yet carry the grid authorities of eight of that dataset's storages, and
passed over them; it carries them now, and on 2026-10-29 Fermilab served 5 of the files.

## 13. Scope and limitations

- A version 15 reader reads version 15 only. A file of any other version is pressed again from
  its study.
- The ROOT reader handles numeric branches, flat or jagged (counted arrays, split members,
  `std::vector`), in every codec ROOT writes (zlib, LZ4, zstd, LZMA). `rvr` reads neither
  RNTuple, nor whole objects, strings or nested vectors; `rvr branches` lists such branches as
  unreadable.
- Incremental pressing covers stored columns added or changed, and samples added, taken out or
  reordered, one kind of change at a time: a column added with a sample means a full press.
- Catalog datasets are pressed from their `Events` tree; another tree of each file (`Runs`) is
  read only for a total. A total sums one number per event: the per-index sums of a list (such as
  NanoAOD's `LHEScaleSumw`) are not a total yet.
- The outage rule asks CERN's Rucio server when a press reads one site, so a press of one site
  at CERN cannot tell CERN down from its own network gone; it stops and keeps what it read.
- A signature proves the header has not changed and which key signed it. Whether that key's holder
  is trusted remains a local decision.

## 14. Conclusion

RowVer replaces the repeated reading of distributed ntuples with one robust read into a single
file. That file is compact, opens in milliseconds, and describes itself well enough to be checked,
compared and reproduced from its own header. A few design choices make this possible: a plain
header that states every fact, per-sample checksums that make headers comparable, a reproducible
and checkable event choice, and a press that records what it could not read instead of hiding it.
Analysis then moves from batch systems to the analyst's own processor, and the grid is used once.

## References

- D. E. Knuth, *The Art of Computer Programming*, vol. 2, *Seminumerical Algorithms*, 3rd ed.,
  Addison-Wesley, 1997, §3.4.2 (Algorithm S).
- G. L. Steele Jr., D. Lea, C. H. Flood, "Fast splittable pseudorandom number generators",
  OOPSLA 2014.
- Y. Collet, M. Kucherawy, "Zstandard Compression and the 'application/zstd' Media Type",
  RFC 8878, 2021.
- S. Josefsson, I. Liusvaara, "Edwards-Curve Digital Signature Algorithm (EdDSA)", RFC 8032, 2017.
- S. Tuecke et al., "Internet X.509 Public Key Infrastructure (PKI) Proxy Certificate Profile",
  RFC 3820, 2004.
- The `.rvr` specification, [FORMAT.md](FORMAT.md); the command reference,
  [crates/rvr](../crates/rvr/README.md); the recipe language,
  [crates/rvr-recipe](../crates/rvr-recipe/README.md).
