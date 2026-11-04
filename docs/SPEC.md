# RowVer: the specification

Every numbered statement is a requirement. Three documents together specify RowVer, each rule in
one place:

| document | specifies |
|---|---|
| this one | what RowVer does: its inputs, how a press runs, every command, the Python package |
| [FORMAT.md](FORMAT.md) | the `.rvr` file, byte for byte, and `samples.lock`, which every file carries |
| [the recipe reference](../crates/rvr-recipe/README.md) | the recipe language: every table, operator and function, to the bit |

## 1. What RowVer is

**1.1** RowVer makes one `.rvr` file from ROOT files: the events a study's cuts keep, the columns its
recipe stores, and the study itself, inside the file.

**1.2** It reads ROOT TTrees from files named by path or URL, on this computer or on storage that
answers HTTPS (a WebDAV door). It reads every experiment's files alike, and asks no dataset catalog.

**1.3** It serves no one program. The format and the tools know nothing of the programs that read
`.rvr` files.

**1.4 On rails.** Every input is written in exactly the form this specification gives. A key, table,
flag or name the specification does not define is refused, naming the nearest defined one. Something
required that is missing is refused; where the fix is mechanical (a table to write in, empty), rvr
offers it at a terminal and goes on when the answer is yes.

**1.5** Every refusal says what is wrong and how to fix it, in plain words, and a validator lists
every problem it finds in its input, not only the first.

**1.6** The parts:

| part | is |
|---|---|
| `rowver` (Rust crate) | the `.rvr` file, and the study texts it carries: reads, validates and writes them. The reference implementation. |
| `rvr-recipe` (Rust crate) | the recipe language: reads a recipe and evaluates its expressions |
| `rvr` (Rust crate, the command) | the validators of the options, the agreement check, the Plan and the pressing core; every command |
| `rowver` (Python package) | reads `.rvr` files through the Rust reader, and installs `rvr` |

**1.7** Every crate and the Python package share one version, whose major number (15) is the format
version.

## 2. How a press runs

```
samples.toml ─▶ samples validator ─┐
recipe.toml  ─▶ recipe validator  ─┤
samples.lock ─▶ lock validator    ─┼─▶ agreement check ─▶ Plan ─▶ pressing core ─▶ .rvr
options      ─▶ options validator ─┤
earlier .rvr ─▶ file validator    ─┘
```

**2.1** A press has five inputs: `samples.toml`, `recipe.toml`, `samples.lock`, the command's
options, and the `.rvr` already at the target, when there is one.

**2.2** Each input has one validator, which turns its text into a checked value or refuses it. A
validator reads only its input: none reads an event or opens a ROOT file.

| validator | input | in | specified by |
|---|---|---|---|
| samples | `samples.toml` | `rowver` | §3 |
| recipe | `recipe.toml` | `rvr-recipe` | the recipe reference |
| lock | `samples.lock` | `rowver` | FORMAT.md, "samples.lock" |
| options | the command's arguments | `rvr` | §5 |
| file | the earlier `.rvr`'s header | `rowver` | FORMAT.md §12 |

**2.3 The agreement check** takes the five checked values and refuses when they do not fit together:

- the lock was resolved from this `samples.toml`: its `samples_toml_sha256` is that text's SHA-256,
  and its samples are the expansion of `samples.toml` (names, labels, trees, metadata, event
  choices, the paths and patterns asked);
- every branch the recipe reads from the event tree is in the branch list (in the lock) of every
  sample, with a type rvr reads (a `[totals]` total over another tree reads branches of that tree,
  which the lock does not list);
- every expression of the recipe type-checks against those branches' kinds, and every column has
  one kind in every file;
- no name `[define]` gives is the name of a branch in a sample's branch list, so that each name
  means one thing;
- no name of the recipe's `[totals]` or `[derived]` is a key of a sample's metadata;
- the options fit the samples (`--seed` needs a random choice; `--events` and `--random` as §5);
- what the earlier file can give: rows and columns to build on, holes to fill, or why not (§8).

**2.4 The Plan** is one value of one fixed form, everything the core needs and nothing more:

- the target path and its staging folder;
- the three study texts, as the file will carry them;
- every sample in order: its name, label, metadata and event choice, and every file in order: where
  to read it (a local path, or an HTTPS URL), the tree, the identity to check before reading (bytes,
  adler32, ROOT UUID, entries, and its sample's branch list), and the entries to read;
- the recipe, checked: the cuts, the stored columns with the kind of each, the totals, the derived
  columns;
- what is taken from the earlier file (columns copied, rows kept, holes to fill);
- the number of files read at once, and the toolchain the file will name;
- the key the file is signed with, when it is signed (yours, when you have one), whether the press
  keeps a journal and what its first line says the press is;
- every name the recipe reads, so that the branches it names are fetched ahead;
- the study's name and the folder a relative path in the lock is read from;
- the earlier file, as opened (§2.1), and its tool keys, which the new file carries (FORMAT.md §11).

**2.5 The pressing core** takes only a Plan. It checks nothing a validator or the agreement check has
checked. It reads the entries the Plan names, evaluates the cuts and columns, and writes the file.

**2.6** The core can fail in these ways only:

| failure | what the core does |
|---|---|
| a file cannot be read: gone, changed since the lock, damaged, its storage unreachable or refusing it | makes a hole of what could not be read and goes on (§7) |
| a failure that is not one file's: the network gone, a proxy or certificate refused, a disk not mounted or full, a permission refused, the process stopped | stops, keeps what it read, and the next press carries on (§7) |
| a value in the data the recipe cannot take (the recipe reference's list): a conversion out of the target kind's range, a mask or two lists taken member by member whose lengths differ in one event, an event past the pair limit, a whole number to a negative power, a total that is not finite | stops, naming the file, entry, column and value, and keeps what it read |
| a file that lacks the tree a `[totals]` total over another tree names, or a branch that total reads in it, or has one there of a type rvr cannot read | stops, naming the file, the tree and the branch, and keeps what it read; a failure reading that tree's bytes is a hole, as for the event tree |

Floats may be NaN or infinite anywhere, stored and derived alike; a float stops the core only as
the row above says (a total that is not finite, or a NaN or infinite value converted to a whole
kind).

**2.7** Before the file is put in place, its header passes the file validator; full
validation, every array decompressed, is `rvr validate`.

**2.8** `rvr status` and `rvr samples plan` show what the Plan would be, without running the core.

## 3. `samples.toml`

`samples.toml` says what a study reads. A person writes it.

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

### 3.1 The document

**3.1.1** `samples.toml` is TOML 1.0 in UTF-8.

**3.1.2** Its top level has exactly the keys `format`, `version`, `places`, `defaults`, `eras` and
`sample`.

**3.1.3** `format = "rvr-samples"` and `version = 1`, both required.

**3.1.4** `[places]`, `[defaults]` and `[eras]` are required, each possibly empty, so that the file
says at a glance what it uses. A missing one is refused; at a terminal, `rvr samples resolve` offers
to write the missing ones in, empty, in their places, checks that the file then reads as before
with those tables added, and goes on.

**3.1.5** `sample` is an array of tables (`[[sample]]`), with at least one.

### 3.2 `[places]`

**3.2.1** Each key is a place name: one or more ASCII letters, digits and `_`, and not `era` or
`file`, which §3.6 gives.

**3.2.2** Each value is text: a folder, a path or a URL. One trailing `/` is dropped.

**3.2.3** A place is used as `{NAME}` (§3.6).

### 3.3 `[defaults]`

**3.3.1** `[defaults]` may hold exactly `tree`, `eras`, `events`, `choose`, `seed` and `metadata`;
not `name`, `label`, `files` or `trees`, which belong to one sample.

**3.3.2** Each applies to every sample that does not give it. A sample's own value replaces it;
`metadata` is merged key by key, the sample's key replacing the default's. `seed` applies only to
the samples that choose at random; a default `seed` no sample uses is refused.

### 3.4 `[eras]`

**3.4.1** Each key is an era name: one or more ASCII letters, digits, `_` and `-` (it becomes part of
sample names).

**3.4.2** Each value is a table: the era's metadata (`[eras.UL18]` with `era = 3`), whose values
follow §3.5.10.

### 3.5 `[[sample]]`

**3.5.1** A sample has exactly the keys `name`, `label`, `files`, `tree`, `trees`, `eras`, `events`,
`choose`, `seed` and `metadata`.

**3.5.2** `name`, required: one or more ASCII letters, digits, `_` and `-`. What every tool calls the
sample. Other text is refused with the name it would make (§3.7.2), and the advice to put the text
in `label`.

**3.5.3** `label`: any nonempty text; what plots show. The name when left
out.

**3.5.4** `files`, required: text, or a nonempty list of texts. Each is where files are (§3.8).

**3.5.5** `tree` or `trees`: exactly one of them, from the sample, or `tree` from `[defaults]`; a
sample's own `trees` replaces a default `tree`, as a sample's own value replaces any default.
`tree` is the tree's name. `trees` is a nonempty table of `LABEL = TREE`, making one sample per
entry: as for the mass points of a signal scan, each its own tree in the same files.

**3.5.6** `eras`: a nonempty list of era names, each a key of `[eras]`, making one sample per era.

**3.5.7** `events`: a whole number from 1, or `"all"`. How many entries to read before any cut.
`"all"` when left out.

**3.5.8** `choose`: `"first"` (the first entries, in the files' order) or `"random"`. `"first"` when
left out. `"random"` needs `events` to be a number.

**3.5.9** `seed`: a whole number from 0 to 2^63 − 1 (TOML's largest), only with `choose = "random"`,
where it is 1 when left out. A sample that gives it without choosing at random is refused.

**3.5.10** `metadata`: a table. Each key is a name a recipe can read as `sample.KEY`: a letter or `_`,
then letters, digits and `_`. Not `events_read`, `events_total`, `events_missing` or `file`, which
rvr gives, nor `sample` or a word of the recipe language (`and`, `in`, `true` and the rest: the
recipe reference's). Each value is text, a whole number (TOML's 64-bit integers), a finite float, `true` or
`false`, or a list or table of these. A date or time is refused: write it as text.

### 3.6 Placeholders

**3.6.1** In `files` and in the text values of metadata, `{NAME}` is replaced by: `{era}`, the
era's name (for a sample made by `eras`); a key of that era's table; or a place. An era key named
like a place is refused, so that a name means one thing. In an era's own text values, a
placeholder is a place, or `{era}`.

**3.6.2** An era value put into text must be text or a whole number (its digits).

**3.6.3** In the text values of metadata only, `{file}` is the file name (without its folder) of the
sample's one file, known once it is resolved; a sample whose metadata uses `{file}` must resolve to
exactly one file.

**3.6.4** Every `{` begins a placeholder closed by the next `}`. A placeholder that is none of the
above, a `{` never closed, and a `}` with no `{` before it are refused.

### 3.7 Expansion

**3.7.1** The samples are, in order: each `[[sample]]` in the order written; within it, each era in
the order of `eras`; within that, each tree of `trees` in the order written.

**3.7.2** A sample made by `trees` and `eras` is named `NAME_POINT_ERA` and labelled
`LABEL POINT (ERA)`, leaving out what does not apply. In the label, `POINT` is the `trees` label as
written (`Signal 300/292.5 (UL17)`); in the name, it is that label made into a name: ASCII letters,
digits and `-` are kept, `.` becomes `p`, any other run of characters becomes one `_`, and `_` is
trimmed from both ends (`300/292.5` becomes `300_292p5`, so `Signal_300_292p5_UL17`). A label that
gives nothing as a name is refused.

**3.7.3** A sample's metadata is the era's values, then the `[defaults]` metadata, then its own, a
later key replacing an earlier one.

**3.7.4** After expansion, no two samples have the same name, and no two the same label.

### 3.8 Where files are

**3.8.1** Each entry of `files` is one of:

| written as | is | read |
|---|---|---|
| `/path/file.root`, `~/path/file.root`, `path/file.root` | a file on this computer: absolute, under the home folder, or relative to the folder holding `samples.toml` | from the file system |
| `root://host[:port]//path/file.root` | a file at a grid storage | through that host's HTTPS door, from the WLCG door list built into rvr |
| `davs://host[:port]/path/file.root`, `https://host[:port]/path/file.root` | a file behind an HTTPS door | over HTTPS, as written (`davs` read as `https`) |

An `https://` or `davs://` entry holding a query (a `?` followed later by `=`) is refused: rvr
names a storage file by its path, and storage access goes by the grid proxy.

**3.8.2** `*` (any run of characters, also none) and `?` (one character) may appear in the file
name, not in its folder. Nothing else is special. A name is matched whole, byte for byte, case
counting.

**3.8.3** An entry matches files in its folder in byte order of their names. A pattern that matches
no file is refused, with the nearest names in that folder. A local symbolic link to a regular file
counts as that file.

**3.8.4** A sample's files are its entries' matches, entry after entry. A file reached twice, by two
entries or by one pattern and a name, is refused.

**3.8.5** No other form is read: `xroot://`, `file://` and the like are refused, naming the three
forms above.

## 4. `recipe.toml`

The recipe language (every table, operator and function, its kinds and its bits) is the
[recipe reference](../crates/rvr-recipe/README.md). This section says how a press uses a recipe.

### 4.1 The document

**4.1.1** `recipe.toml` holds exactly these keys at its top: `format = "rvr-recipe"` and
`version = 1` (both required), `title` (text, optional), and the five tables `[define]`, `[keep]`,
`[columns]`, `[totals]` and `[derived]`. Any other key is refused, with the nearest known key when
one is within the slip a "Did you mean" names (§1.4).

**4.1.2** The tables hold these, and nothing else:

| table | entries |
|---|---|
| `[define]` | `NAME = "expression"` |
| `[keep]` | `NAME = "condition"`, or `NAME = { if = "condition", apply = "condition" }`: `apply` required, `if` optional |
| `[columns]` | `branches = ["…", …]`, nonempty texts each naming a branch as the file spells it; `NAME = "expression"`, or `NAME = { value = "expression", stored = "KIND" }`: `value` required, `stored` optional, one of the eleven kinds (FORMAT.md §4) |
| `[totals]` | `NAME = "expression"`, or `NAME = { value = "expression", tree = "TREE" }`: `value` required, `tree` optional, nonempty |
| `[derived]` | `NAME = "expression"` |

**4.1.3** Every `NAME` is a plain name: a letter or `_`, then letters, digits and `_`, and no word of
the language. A column `branches` names keeps the branch's name as the file spells it. The other
rules of names (which are reserved, which may not repeat) are the recipe reference's.

**4.1.4** No two columns share a name; none is `sample`, `entry` or `file`; none shares a name with a
derived column.

**4.1.5** The recipe validator (`Recipe::read`) refuses a recipe with every problem it finds, one a
line, each naming its place (`[columns] ht: …`). It checks every expression as the recipe reference
says a recipe is read: its syntax, its depth, that every function it calls exists and is given a
number of values it takes, the fixed values some functions take, and true-or-false values wherever
one is needed, as far as the text tells.

**4.1.6** Each table may be empty but `[columns]`, which stores at least one column. The recipe
validator refuses a recipe that leaves a table out, or whose `[columns]` stores nothing. At a terminal,
`rvr press` and `rvr try` first offer to write the missing tables in, empty, in their places, check
that the recipe then reads as before with those tables added, and go on. They do not offer when
`[columns]` is missing or stores nothing, which only the recipe's author can mend, nor for
`rvr press --fill`, which presses the recipe the file carries.

### 4.2 What the core evaluates

**4.2.1** The core reads each file in runs of at most 50,000 entries. A random choice cuts a run where
the next chosen entry lies 2,000 entries or more past the run's end in a local file, or 50,000 in a
remote one, and reads no run without a chosen entry.

**4.2.2** An event is *chosen* when the event choice chose it, and *kept* when it is chosen and
passes every cut. On each run the core evaluates the cuts and the totals over the run's chosen
entries only, and the stored columns over its kept events only; a press that builds on columns
(§8.2) evaluates the columns it reads over the events the file kept only.

**4.2.3** A data error (§2.6) stops the core only when it lies in a chosen event, and for a stored
column only in a kept event.

### 4.3 Cuts

**4.3.1** The cuts of `[keep]` apply in the order written. An event is kept when it passes every one.

**4.3.2** A cut written `{ if = "…", apply = "…" }` applies only where `if` is true: to the samples
whose metadata makes it true, or, when `if` is one truth value per event, to those events. A
comparison with a field a sample's metadata lacks is false, so such a sample passes a cut whose `if`
compares it (`if = "sample.era == 3"`); an `if` that gives a field the sample lacks
(`if = "sample.vbf"`) does not apply to it either.

**4.3.3** The cutflow is the number of chosen events left after each cut, in order, summed over the
files read.

### 4.4 Stored columns

**4.4.1** A column `branches` names is copied with the branch's kind.

**4.4.2** A computed column `NAME = "expression"` has the kind its expression gives; written
`NAME = { value = "expression", stored = "KIND" }`, its values are converted to `KIND` by the
recipe reference's conversion rule (a value that does not fit is a data error).

**4.4.3** A column has one kind in every file and sample; the agreement check makes sure of it before
the core starts.

### 4.5 Totals

**4.5.1** A total `NAME = "expression"` is, for each file, the expression's value over every entry
the choice chose of it, before any cut, each widened to an `f8` and added in entry order from +0.0.

**4.5.2** A total `NAME = { value = "…", tree = "TREE" }` is, for each file, the value over every
entry of that other tree of the file, added the same way; only for a sample that reads its files
whole, and never the tree the events are read from.

**4.5.3** A sample's total is its files' totals added from +0.0 in the lock's file order.

**4.5.4** A total is one number per entry; a list is refused. A total that is not finite is a data
error, since a file's metadata holds only finite numbers.

### 4.6 Derived columns

**4.6.1** A derived column is kept in the file as its expression; the core works it out over the rows
it wrote and records its kind, checksums and facts (FORMAT.md §10).

**4.6.2** Its expression uses only the stored columns, the derived columns above it, and `sample.KEY`:
the sample's metadata with its totals and `file` (the path of the row's file in the lock), without
`events_read`, `events_total` and `events_missing`.

**4.6.3** In every press that reads events with its stored columns in hand, the core works each
derived column out on the first run of each sample that keeps an event, so that a data error in one
stops the press there, not after every file is read. A press that builds on columns (§8.2), whose
copied columns are not read, works them out once at the end (§4.6.4).

**4.6.4** A derived column that fails when worked out over the whole file leaves the file written
without its derived columns; the press says so and exits 1. Mending `[derived]` and pressing again
reads no input (§8).

## 5. The commands and their options

### 5.1 Every command

**5.1.1** A command line is `rvr COMMAND`, then words and flags in any order. The command is the
first argument that is not a flag, the flags before it passed over with their values
(`rvr --holes status` is `rvr status --holes`); a word there that names no command is wrong use,
said with the nearest command (`rvr prss` gives `Did you mean: press?`). For `rvr samples` and
`rvr key`, what it does is the next argument that is not a flag, and one of `expand`, `resolve`,
`check`, `plan`, or `new`, `show`, `trust`, else wrong use; for `rvr proxy`, it is `init` or
nothing.

**5.1.2** A flag is `--NAME`, with its value, when it takes one, as the next argument (an argument
that begins with `--` is not a value); `--NAME=VALUE` is not read. `-f`, for `rvr empty --force`, is
the only short flag. `--fill [KINDS]` takes the next argument as its KINDS exactly when every
comma-separated part of it is a kind of hole (§7.3.1), whatever folders there are; otherwise that
argument is a word (the study folder). An argument after `--fill` that is not kinds, but holds a
comma or has a part a slip of a letter or two from a kind, and is no folder there, is wrong use,
whatever follows it: the refusal says the kinds and the nearest (`did you mean --fill damaged?`).

**5.1.3** A flag the command does not take (said with the nearest flag it does take), a flag given
twice (but `--drop`, §8), a flag without its value, a value not of its flag's form (§5.1.4), or a
word too many or too few is wrong use: the command does nothing, prints `rvr COMMAND: what is wrong`
and its usage on standard error, and exits 2. An argument that is not UTF-8 text is wrong use too:
rvr runs nothing, says `rvr: the argument … is not UTF-8 text` and how to fix it, and exits 2.

**5.1.4** A count given as a flag's value (`--events N`, `--workers N`, `--hours H`, `rvr try
--events N`) is a whole number from 1, written in digits, with `_` allowed between two digits
(`50_000`); `--events` is at most 2^63 − 1 and `--hours` at most 720. `--seed S` is a whole number
from 0 to 2^64 − 1, written alike. `--workers` is the number of cores when not given, and 1 when the
system says none.

**5.1.5** What a command reports goes to standard output. The line that ends a failing command goes
to standard error, as `rvr COMMAND: what is wrong, and how to fix it`.

**5.1.6** Exit status:

| status | means |
|---|---|
| 0 | done, or, for `status`, `diff`, `samples check` and `proxy`, the answer is yes |
| 1 | failed, or the answer is no (a file that differs, a study not current, a file invalid) |
| 2 | wrong use (§5.1.3), an unknown command, or a file `rvr diff` cannot read |
| 3 | `rvr press` wrote, or found, a file with holes |
| 141 | the output pipe closed before the command finished, as a C tool stopped by SIGPIPE |

**5.1.7** rvr asks a question (`[y/N]`) only when standard input and standard error are both
terminals; `y` or `yes`, in any case, is yes, and any other answer is no.

### 5.2 The commands

| command | does | § |
|---|---|---|
| `rvr new DIR` | writes a study that presses as it is | 5.3 |
| `rvr samples expand FILE [--json]` | lists the samples `samples.toml` makes, asking nothing of any storage | 6 |
| `rvr samples resolve FILE` | finds every file and writes `samples.lock` | 6 |
| `rvr samples check LOCK` | asks again whether every file is as the lock recorded it | 6 |
| `rvr samples plan LOCK [--events N\|all] [--random] [--seed S]` | what a press would read, from the lock alone | 6 |
| `rvr branches [DIR] [--sample NAME] [--find TEXT]` | the branches of a sample's first file | 5.4 |
| `rvr try "EXPRESSION" [DIR] [--events N] [--sample NAME] [--kept]` | an expression's values on a sample's first events | 5.5 |
| `rvr press [DIR] [--events N\|all] [--random] [--seed S] [--out FILE] [--workers N] [--full] [--fill [KINDS]] [--restart]` | presses the study | 7, 8 |
| `rvr status [DIR] [--out FILE] [--events N\|all] [--random] [--seed S] [--holes]` | whether the file holds the study as it is, and what a press would read | 9 |
| `rvr derive FILE 'NAME = EXPRESSION'… \| --drop NAME… \| --rename OLD NEW` | adds, drops or renames derived columns of a file | 8 |
| `rvr validate FILE… [--complete]` | checks files in full | 9 |
| `rvr dump FILE [--files] [--recipe] [--samples] [--lock] [--columns] [--all]` | the inside of a file as text | 9 |
| `rvr header FILE` | the header as the file holds it | 9 |
| `rvr diff A B [--rows] [--key COLUMN,…] [--sample NAME]` | where two files differ: from their headers, or (`--rows`) event by event | 9 |
| `rvr reproduce FILE [DIR] [--study FOLDER] [--workers N]` | presses a file's own study again and compares | 9 |
| `rvr empty FILE… [--study FOLDER] [--anyway] [--force \| -f]` | lets a file's arrays go, keeping its header | 8 |
| `rvr refill FILE [--study FOLDER] [--workers N]` | presses an empty file's arrays again | 8 |
| `rvr key new [NAME]`, `rvr key show`, `rvr key trust KEY NAME…` | the key that signs your files, and the keys you trust | 10 |
| `rvr proxy`, `rvr proxy init [--hours H]` | the grid proxy, shown or made | 10 |
| `rvr --version` | `rvr VERSION (reads and writes rowver 15; commit COMMIT)` | 12 |
| `rvr`, `rvr help`, `rvr --help` | the usage, every command and flag | |

### 5.3 `rvr new DIR`

**5.3.1** Makes `DIR` when needed, and writes into it `samples.toml`, `recipe.toml` and `.gitignore`:
two CMS Open Data samples that need no grid proxy, a muon and tau selection for them, and all five
recipe tables. The recipe's title is `DIR`'s own name, the name a press gives the file (for `.`, the
current folder's name).

**5.3.2** It writes nothing when any of the three is already there, and says which.

**5.3.3** The `.gitignore` leaves out `*.rvr`, `*.rvr.partial/`, `*.reproduced/` and rvr's staged
files (`*.rvr.derive`, `*.rvr.refill`, `*.rvr.refilled`, and `*.rvr-new`: those of `recipe.toml`,
`samples.toml` and `samples.lock`).

### 5.4 `rvr branches`

**5.4.1** Lists every branch of the first file of the study's first sample (or of `--sample NAME`):
from the sample's branch list in the lock when the study has one (no file is opened), else from
that file itself, found through `samples.toml` (only that sample's first file is found and opened,
and nothing is written), so that a study's branches can be listed before its recipe is written.

**5.4.2** One line per branch, ordered by name ignoring case: its name as a recipe writes it, its
type, and its shape (`flat`, `jagged`, `N per event`, or `jagged, N per element`). Branches rvr cannot read follow under
`Not readable by rvr:`, each with why. `--find TEXT` shows only the names holding TEXT, ignoring case.

### 5.5 `rvr try`

**5.5.1** Evaluates one expression, with the recipe's defined names, on the first `--events N`
events (10 when not given) of the first file that has entries of the study's first sample, or of
`--sample NAME`,
reading only the branches it names; `--kept` shows only the events the cuts keep.

**5.5.2** It reads `samples.lock`, `samples.toml` and `recipe.toml` through their validators and the
agreement check, as a press does; a total stands in as 1, and says so.

**5.5.3** One line per event: `entry E` and the value.

## 6. Resolving: `rvr samples` and `samples.lock`

`samples.lock` pins every file a study reads. `rvr samples resolve` writes it, and nobody edits it;
its form is in FORMAT.md, "samples.lock", since every `.rvr` file carries it.

### 6.1 `rvr samples expand FILE [--json]`

**6.1.1** Runs the samples validator on `FILE` and lists the samples it makes, each with its name,
label, tree, files asked and event choice; `--json` prints them as JSON, each with its metadata too.
No storage is asked.

### 6.2 `rvr samples resolve FILE`

**6.2.1** Runs the samples validator (offering, at a terminal, what §3.1 lets it offer) and the
recipe validator on the `recipe.toml` beside `FILE` (offering what §4.1.6 lets it offer; a study
with no `recipe.toml` is refused: `rvr new` writes one), then finds every file of every sample,
runs the agreement check (§2.3) on the samples, the recipe and the lock it would write (every part
of it but the options and the earlier file, which belong to a press; each sample's event choice is
the one `samples.toml` gives, as the study as written must press with no options), and writes
`samples.lock` beside `FILE` only when every check passes. A lock exists only when the study's recipe can be
pressed from it: what is left to fail is the data streaming in (§2.6).

**6.2.2** For each file it records the identity a press checks before reading it: its size in bytes,
its adler32 (when its storage gives one), its ROOT UUID and its tree's entries. For each sample it
records the branch list of its first file's tree, which every file of the sample must have (§7.2.3):
a branch list needs a tree's whole record, which for a skim can be megabytes, so it is read once a
sample.

**6.2.3** It asks:

| the files are | resolve asks |
|---|---|
| on this computer | the file system, for each folder's names and each file's size; each file's whole bytes, for its adler32; its ROOT header, for its UUID and entries (and, for a sample's first file, its tree's whole record, for the branch list) |
| on storage | its HTTPS door: one listing (`PROPFIND`, depth 1) per folder; one `HEAD` per file with `Want-Digest: adler32`; and the byte ranges that open the file and begin its tree's record, for its UUID and entries (for a sample's first file, the whole record, for the branch list) |

**6.2.4** No event is read.

**6.2.5** It works on 12 samples at once, the files of one sample one after another; a folder listed
once is not listed again in that run.

**6.2.6** A listing or `HEAD` that fails as a busy storage fails (§7.4) is asked again after 5, 15
and 30 seconds.

**6.2.7** All or nothing: when any sample fails, or the agreement check refuses, no lock is written
(an existing one is left as it was), and each failure is a line
`  FAIL  NAME: why`, with the fix: the nearest names in the folder for a name that matches nothing,
the trees a file has for a tree it lacks.

**6.2.8** The lock is written through a file beside it, then put in its place, so it is whole or the
old one.

**6.2.9** It then names the `.root` files in the folders it read, on this computer and on storage,
that no sample reads, as a check that none was forgotten: the first 3 in byte order, then
`… and N more`. This is not a failure.

### 6.3 `rvr samples check LOCK`

**6.3.1** Runs the lock validator, then asks every file's size and adler32 again (on this computer:
the file system and the whole file; on storage: a `HEAD`), and compares the SHA-256 of the
`samples.toml` beside the lock with the lock's.

**6.3.2** It lists each file that differs, each it could not ask (and why), and whether
`samples.toml` was edited since; it exits 0 when everything is as the lock recorded it, else 1. A
file that differs is what a press would make a `changed` hole of; to take a new version of it,
resolve again and review the new lock.

### 6.4 `rvr samples plan LOCK [--events N|all] [--random] [--seed S]`

**6.4.1** Says what a press would read of each sample (entries, of how many; files, of how many; and
where the first N stops), from the lock alone, asking nothing of any storage.

**6.4.2** The options are the press's (§7.1.2).

## 7. The pressing core

### 7.1 The choice of events

**7.1.1** Each sample reads what its event choice says: every entry, the first N in the lock's file
order, or N chosen at random from a seed (FORMAT.md §6.1).

**7.1.2** `rvr press`, `rvr status` and `rvr samples plan` take the same options:

| options | every sample reads |
|---|---|
| none | its own choice, from `samples.toml` |
| `--events N` | its first N entries |
| `--events all` | every entry |
| `--events N --random` | N chosen at random, seed 1 |
| `--events N --random --seed S` | N chosen at random, seed S |
| `--random` | its own N, chosen at random: a sample already random with its own seed, any other with seed 1; refused when a sample reads every entry |
| `--seed S` | its own choice, with seed S for a random one; refused when no sample is random |

`--events all --random` is refused. S is a whole number from 0 to 2^64 − 1.

### 7.2 Reading

**7.2.1** The core reads `--workers N` files at once (the number of cores when not given, and 1
when the system says none), and only the branches the
recipe uses.

**7.2.2** Whatever the number of workers, the rows lie in the order of the samples, then of each
sample's files in the lock, then of entries within a file.

**7.2.3** Before reading a file, the core checks it against the lock: its size, then its adler32
(when the lock has one), then its ROOT UUID and entries, then that every branch the recipe reads is
in its tree with the kind and shape its sample's branch list gives (other branches may differ, as a
run's trigger menu adds `HLT_` branches). A difference makes a `changed`
hole of every entry it chose of the file.

**7.2.4** A remote file is read over HTTPS from its door (§3.8.1), following up to 4 redirects; a
connection waits at most 15 s to open, and a read 120 s for its answer.

**7.2.5** Before each run (§4.2.1) of a remote file, every basket of the run that the recipe reads is
asked for in `multipart/byteranges` requests: at most 256 ranges a request, 6 requests at once,
ranges within 16 KiB of each other joined. A `206` that answers every range asked in one part (its
`Content-Range` covering them all, at most twice their bytes and 1 MiB more) is taken as it is. A
storage that answers such a request with `400` or `416`, or otherwise not as the parts asked, is
asked one range a request for the rest of that file, ranges within 512 KiB joined up to 32 MiB.

**7.2.6** A fetch ahead of need is never waited for or asked again: the read that needs the bytes
is.

### 7.3 Holes

**7.3.1** A file the core cannot read is not fatal: the file is written without those entries, and
records a *hole*: the entries it could not read, a kind, and why (FORMAT.md §6.2). rvr writes a run
of entries as `entries A to B`, both A and B among them (`entry A` for one), wherever it says one:
press, status and validate alike; the file, and the Python package's `holes`, record that run as
`[A, B + 1]`.

| kind | the core found |
|---|---|
| `unreachable` | its storage kept failing after every wait, or answered in a way no wait mends (§7.4) |
| `damaged` | a basket that does not decompress, or another error in the file's bytes |
| `gone` | the file is not there any more |
| `changed` | the file is not the one the lock pinned (§7.2.3) |
| `refused` | its storage refuses it, with a valid grid proxy |

**7.3.2** A damaged basket makes a hole of the entries of the run read with it, and the rest of the
file is read. Every other kind makes a hole of every entry the file's choice has left from where it
happened. A file that cannot be opened at all is one hole of every entry it chose.

**7.3.3** A local file missing from a folder that is there is `gone`; its folder missing, or the file
not readable for want of rights, stops the press (not the file's own failure: a disk not mounted).
A file of no entries that the recipe's `[totals]` read (under a choice of all or the first N,
FORMAT.md §6) has no entry to make a hole of: when it cannot be read, the press stops, keeps what
it read, and says which file and why its totals need it.

**7.3.4** `rvr press` exits 3 when the file it writes has holes, prints each with its kind and why,
and says `rvr press --fill reads them again once they can be read.`

### 7.4 When a storage fails

**7.4.1** A read that fails with `403` (with a proxy), `408`, `429`, any `5xx`, a dropped connection,
or a host name that does not resolve is asked again after 5, 15, 30, 60, 120, 240 and 300 seconds:
eight tries over 770 s. Each wait is printed:
`  waiting W s to ask again for FILE (K of 8 tries made): WHY`.

**7.4.2** A read whose eight tries fail makes an `unreachable` hole, or a `refused` one when the
last answer was `403` (with a proxy).

**7.4.3** Answered at once:

| answer | the core |
|---|---|
| `404`, `410` | makes a `gone` hole |
| `401`; a certificate rvr does not trust; `403` with no proxy | stops: every file there would fail the same |
| `200` to a range request (the storage ignored the range); any other `4xx` | makes an `unreachable` hole |

**7.4.4** Waits end as soon as the press stops for another reason.

**7.4.5 The outage rule.** While a read waits, once no storage has answered anything for 120 s, the
core tries for 5 s to open a connection to each door that answered earlier in the press and is not
failing now, or, when every one is failing, to every door that answered. One connection shows the
network works: only the failing storage is down, and its files become holes after their waits. No
connection, twice 15 s or more apart, means the network is gone: the press stops, keeps what it
read, and says so. When every door that answered is at one site (as the WLCG door list groups them),
the core cannot tell the site down from the network gone, and stops and keeps what it read, saying
why.

### 7.5 Stopping and carrying on

**7.5.1** A press is staged in `FILE.partial/` beside its target: a journal of the files finished,
the spool of their kept values, and the arrays as they are packed. Nothing is put at the target
until every file is read or made a hole and every array is packed: the file appears whole, or not
at all.

**7.5.2** While a press runs, it holds a lock on its journal; another press of the same target, and
`rvr empty` or `rvr refill` of it, is refused while it is held.

**7.5.3** At most every 2 seconds while files finish, the spool and then the journal are made safe
on disk (a checkpoint). A press stopped by anything (Ctrl-C, a killed process, a laptop asleep, a
failure that is not a file's own, a data error) loses at most the files finished since the last
checkpoint. A press that builds on columns (§8.2) and is stopped by a file it cannot read keeps
nothing, since only `rvr press --full` goes on from there.

**7.5.4** The next press with the same Plan (the same three texts, event choices, columns read, file
built on, toolchain and number of files) carries on from the files finished at the last checkpoint,
and says so. A file stopped half read is read again from its start, and so is one that became an
`unreachable` or `refused` hole; a `damaged`, `gone` or `changed` hole stays.

**7.5.5** A press with another Plan starts over and says what differs; `--restart` always starts
over, removing what a stopped press left.

**7.5.6** A build whose commit is `unknown` or ends in `-dirty` (§12) keeps no journal, so a press it
makes cannot be carried on.

**7.5.7** The staging folder goes once the file is in place.

### 7.6 What a press holds

**7.6.1** What a press holds in memory does not grow with the events it keeps: each file's kept
values go to the spool as they are read, and each column is packed from the spool a piece at a
time. A press needs free disk for the spool, the kept values compressed where that pays, a piece at a
time, beside the file it writes.

**7.6.2** A press that builds on a file (§8) also holds 8 bytes per row of that file, and one of its
columns at a time.

### 7.7 What a press shows

**7.7.1** It first says what it presses: `Pressing S samples, F files, E entries, W at a time:`.
A press that builds on columns says `FILE has the events this recipe keeps (K) and C of its N
columns.`, then `Reading only NAMES for those events, F files, W at a time:` (or `Nothing to read:
every column is copied from it.`); one that builds on samples, `FILE holds the rows of H of the S
samples as samples.lock has them.`, then `Reading only the new samples NAMES, F files, E entries,
W at a time:`; a fill, `Filling FILE: reading H holes of F files, E entries, W at a time; every
other row is the file's own:`.

**7.7.2** On a terminal (standard output a terminal, `TERM` not `dumb`, `RVR_TRACE` not set, at
least 7 rows and 41 columns), it draws one bar for each sample the press reads, finished ones
too, with the entries read of those chosen; when they are more than fit (10, or the terminal's
rows less 5), those under way and then those to come get a bar, one fewer than fit, and the rest
one line, `and N more: D done, C to come, U under way`. Under the bars, one line has the events
kept so far, the holes, the read rate and the time. They are redrawn every 200 ms, each line cut
to 79 characters or the terminal's width less one; what happens meanwhile (a wait, a hole) is
printed above the bars. Elsewhere it prints one line per file as it finishes:
`  N/F  ENTRIES entries  KEPT kept   FILE`.

**7.7.3** Then the cutflow, `Read E entries; kept after each cut:` and one line per cut, or `with no
cuts, every one is kept.`; then, when the file is signed, `Signed with your key K…`; then
`Wrote FILE (R rows, C columns[ and D derived], S MB) in T s.`, with its holes listed when it has
any. A press that builds on columns, or on samples with none new, prints no cutflow, as it runs no
cut.

**7.7.4** `RVR_TRACE` set (to anything) prints every byte-range request on standard error, with what
answered it (its status, or why it failed), for looking into a storage's answers.

### 7.8 What a press writes

**7.8.1** The file FORMAT.md specifies, with the study's three texts, every file read with its
identity and how much of it was read, each sample's metadata with `events_read` and `events_total`
(and `events_missing` when the file has holes), the totals, and what built rvr.

**7.8.2** It is signed when you have a key (§10).

## 8. Building on a file

When `rvr press` finds a file at its target, made from the same study, it builds on it where it can,
reading only what is new.

**8.1 A current file.** When the file holds the study as it is (the same three texts and event
choices, made by this build, nothing new to read, no holes), `rvr press` says
`FILE is current: nothing to press.`, leaves it byte for byte and exits 0; with holes, it says so and
exits 3. An empty file (§8.7) is never current.

**8.2 New and changed columns.** When the file was made by this build from the same `samples.toml`
and lock texts, with the same cuts, totals and event choices, and its carried recipe reads, only the
columns new to the recipe, or whose definition changed (their expression, their stored kind, or a
defined name they use, however deep), are read, for the events the file kept, from the runs that
hold one of them; a file of which the earlier file kept no event is not opened, as no input that
gives nothing is (§8.3). Every other column is copied from the file, checked against its record as it is.
A column dropped from the recipe is dropped from the file. While building on columns, a file that
cannot be read stops the press, as a hole would change the rows the columns are built on;
`rvr press --full` presses everything and makes holes.

**8.3 Nothing to read.** When only `[derived]` changed, or columns were only dropped, no input is
opened: the stored columns are copied, the derived ones worked out from them.

**8.4 New samples.** When the file was made by this build with the same cuts, totals and columns, and
every sample it holds is in the lock as it was (its files, tree, metadata and event choice), only the
new samples are read; a sample taken out, or the samples put in another order, reads nothing. Every
column is packed again, in the lock's order of samples; a sample it holds keeps its holes.

**8.5** Any other difference (other cuts, totals or event choices, a sample whose files changed, a
column changed together with the samples, another build) means every column is pressed, and the
press says why: `Pressing every column: FILE cannot be built on, as WHY.` `--full` always presses
every column.

**8.6** A build whose commit is `unknown` or ends in `-dirty` never builds on a file.

**8.7 `rvr press --fill [KINDS]`** reads only the holes of the file at the target (`KINDS`: a
comma-separated list of hole kinds, every part one, to read only those) and writes the file again
with their rows in place, merged by entry within each file. The study's `recipe.toml` and
`samples.toml`, and the options' event choices, must be the ones the file carries, so that the
recipe it presses is the file's own; it says which differs otherwise. The lock may have been resolved again for the files it fills (put back, or
another copy in their place). A hole that fails again stays a hole, with its new kind and why; when
none can be read, the file is left as it was. A file's totals gain the sum over the entries filled.

**8.8 `rvr derive FILE`** changes a file's derived columns: `'NAME = EXPRESSION'` adds one (the name a
plain name, used by no column, derived column, defined name, total, metadata key, or `sample`,
`entry`, `file`); `--drop NAME…` drops them (a derived column another uses only with it, or after
it); `--rename OLD NEW` renames one no other uses. An expression is never changed in place. It first
checks the file's derived columns against their records; it records only what is new; it changes the
recipe the file carries to match, copies the arrays byte for byte, and signs the file again with your
key, saying whose signature it replaces (or that the file is no longer signed). The study's own
`recipe.toml` is not changed, and it says so.

**8.9 `rvr empty FILE…`** lets a file's arrays go and keeps its header byte for byte: the file then
ends where its header does. It first checks each input the lock names is as the lock recorded it
(unless `--anyway`), refuses a file with holes, and refuses a file made by another rvr version unless
`--anyway`. At a terminal it asks first, naming the file's rows, columns and size; `--force` (`-f`)
empties without asking, and is needed where nobody is at a terminal. Each file is its own; the exit
status is the worst.

**8.10 `rvr refill FILE`** presses the study an empty file carries again, beside it, with the same
event choices, and puts the arrays back once each has the checksum its record states: with this
build, the file is the one it was, byte for byte, its signature as it was; with a build that
compresses otherwise, the header takes the arrays' new places and is signed again with your key,
saying whose signature it replaces. Other values are refused, naming the columns; the file stays
empty.

## 9. Inspecting files

**9.1 `rvr validate FILE… [--complete]`** checks each file in full (FORMAT.md §12): one line per file
on standard output, `FILE: valid, …` (rows, columns, derived columns, entries read of those the
samples hold, holes by kind, emptiness, the signer) or `FILE: INVALID: WHY`. `--complete` also fails a
valid file that has holes or is empty. It exits 1 when any file fails.

**9.2 `rvr dump FILE`** prints the inside of a file: the header in full, the three texts, and every
column with its kind, length and first values, a derived column worked out with its expression.
`--files`, `--recipe`, `--samples`, `--lock` and `--columns` print only those parts, in the order
given; `--all` prints every value of every column: a line naming them, then one line a row, the
values apart by tabs (its sample's name, its entry, then each stored column and each derived column
worked out, a list as `[a, b]`), then the sample columns. `--all` goes alone or with `--columns`;
with another part it is wrong use. A file whose header fails its
checks is still shown, after a line saying why.

**9.3 `rvr header FILE`** prints the header's bytes exactly as the file holds them. When the header
fails its checks it says why on standard error, and exits 1.

**9.4 `rvr diff A B`** says where two files differ, from their headers alone: the rows, in all and of
each sample; samples, columns, derived columns and sample columns only one has; each sample's label;
kinds and expressions that changed; the entries, and each column and derived column, of each sample
whose contents changed (every array has one checksum per sample); and each sample column's value
that changed. It says when a
header fails its checks, and when one file is empty. Exit 0 when the headers show no difference, 1
when they do, 2 when one cannot be read.

**9.4.1 `rvr diff A B --rows [--key COLUMN,…] [--sample NAME]`** compares the two files' events
themselves, so that files holding the same events in another order or layout (samples reordered,
another implementation's press, two skims of the same data) can be compared. It reads the arrays.
Rows are matched by sample name and an event's identity: by default the ROOT UUID of its file (from
the lock the file carries), which of the sample's files with that UUID it is (the first, the second:
copies of one file share a UUID), and its entry in that file; with `--key`, the values of those stored
flat columns, which both files hold with the same kind (`--key run,luminosityBlock,event` matches
an event's copies in two different skims). Two rows of one file with the same key are refused,
naming the sample and the key. It says, per sample, how many events only A holds, how many only B
holds, and, of the events both hold, compared column by column over the stored and derived columns
both hold by name, the first event whose values differ (its sample, key, the column, and both
values), with how many events differ in all; columns only one file holds are named, not compared.
Values compare as FORMAT.md §4 stores them: floats bit for bit, NaN equal to the same NaN.
`--sample NAME` compares that sample alone. Exit 0 when the two hold the same events with the same
values, 1 when they do not, 2 when one cannot be read.

**9.5 `rvr status [DIR]`** says, from the study's texts and the file's header alone (no input is read),
whether the file holds the study as it is: what changed in each text, what a press would do (§8), how
much each sample holds of what it chose, and its holes by kind; `--holes` lists every hole. It shows a
press stopped part way, or under way. With no file at the target, it says what a press would read
(`rvr press would press every column: S samples, F files, E entries.`, or why it would refuse; for
a stopped press that `rvr press` carries on, the files left of it and their entries), then
`FILE is not there yet: rvr press makes it.`, and exits 1. It exits 0 only when `rvr press` with the same options would
have nothing to do: the file current (§8.1) and without holes.

**9.6 `rvr reproduce FILE [DIR]`** writes the three texts FILE carries into DIR (`FILE`'s name with
`.reproduced` when not given), presses them with the same event choices into
`DIR/<the file's study>.rvr`, and runs `rvr diff` on the two, exiting with its status. Relative paths
in the lock are read from FILE's folder, or `--study FOLDER`. A DIR that holds a `.rvr` file, or a
`recipe.toml`, `samples.toml` or `samples.lock` other than FILE's, is refused, naming them.

## 10. Keys and proxies

**10.1** rvr's folder is `$RVR_CONFIG` when it is set and not empty, else `$XDG_CONFIG_HOME/rvr` when
that is set and not empty, else `~/.config/rvr`.

**10.2 `rvr key new [NAME]`** makes your Ed25519 key, `key` in that folder: 64 lowercase hexadecimal
characters and a newline, readable by you alone, never written over. Every file you press is signed
with it from then on. It prints your public key, and the `rvr key trust` line others use.
`rvr key show` prints your public key.

**10.3 `rvr key trust KEY NAME…`** adds `KEY NAME` (the words of NAME joined by spaces) as a line of
`trusted` in that folder; `#` lines and blank lines are passed over. KEY is 64 lowercase
hexadecimal characters, as `rvr key show` prints it. A key trusted already stays under its name:
trusting it by that name says so (exit 0), and by another is refused (exit 1), saying which line to
take out. `rvr validate` names a signer from it: `signed by you`, `signed by NAME (trusted)`, or `signed by key K…, which you do not trust yet`, followed by the `rvr key trust` line that trusts it
(` (rvr key trust KEY NAME)`).

**10.4 `rvr proxy`** shows the grid proxy rvr uses (`$X509_USER_PROXY`, else
`/tmp/x509up_u<your uid>`): its file, whose it is, its VO and how long its attributes last, the time
left, and what to do when it is missing or ending; it warns that the attributes end before the proxy
only when they end an hour or more before it (voms-proxy-init's end a second before). Exit 0 when it
can be used.

**10.5 `rvr proxy init [--hours H]`** makes a proxy for the VO cms with no other grid tool: from
`$X509_USER_CERT` and `$X509_USER_KEY` (a `$X509_USER_CERT` ending `.p12` is read as one PKCS #12
file holding both; with only one of the two set, the other is the one in `~/.globus`), else
`~/.globus/usercert.pem` and `userkey.pem`, else `~/.globus/usercred.p12`, else the only `.p12`
there. It refuses a key file with any permission for its group or other accounts, giving the
`chmod 600` that fixes it; asks the passphrase without showing it; asks CMS's VOMS server
for your attributes; and writes a proxy valid for H hours (1 to 720, 192 when not given, never past
the certificate's end), readable by you alone, through a file beside it put in its place.

**10.6** A press or resolve that reads storage warns when the proxy, or its attributes, have less
than 6 hours left. rvr trusts the public web authorities, CERN's, and the 83 IGTF authorities
(release 1.146), with no setup; `$X509_CERT_DIR`, else `/etc/grid-security/certificates` when it
exists, adds others.

## 11. The Python package

**11.1** `pip install rowver` (`rowver[all]` adds Awkward, pandas and pyarrow); Python 3.11 or later;
NumPy is its one requirement. It installs the `rvr` command too, which runs the same code as the
binary.

**11.2** `rowver.open(path)` gives a `File`, checking the header (FORMAT.md §12, "on opening"); a file
whose header fails raises `rowver.Error`, a missing path `FileNotFoundError`.

**11.3** A `File` has `rows`, `samples`, `labels`, `columns`, `derived`, `sample_columns` (name to one
value per sample), `recipe`, `study` (the three texts by file name), `signed_by` (the public key, or
`None`), `holes` (`[{sample, file, entries: (first, last), kind, why}]`, `last` not included),
`complete` and `empty`; and `path`, `header` (the JSON header), `len(f)` (its rows) and `name in f`.

**11.4** `f[name]` reads a column: a flat one as a read-only NumPy array (not copied), a list one as
an Awkward array, `sample` as `int32`, `entry` as `int64`, a derived one worked out and checked
against its record. An unknown name raises `KeyError`, listing the columns.

**11.5** A column read is held until `f.forget(name, …)` (or `f.forget()`, every one) lets it go.
Columns asked for together (`f.read([…])`, `arrays`, `to_*`) are read in parallel.

**11.6** `f.arrays()` gives an Awkward record array; `f.to_arrow()` a pyarrow Table and
`f.to_pandas()` a pandas DataFrame, each with `sample` (its names, as a categorical: an Arrow
dictionary in `to_arrow()`) and `entry` first;
`f.to_parquet(path)` writes `to_arrow()`.

**11.7** `rowver.validate(path)` checks the file in full, as `rvr validate` does, and returns its
`(rows, columns)`, or raises `rowver.Error`.

## 12. Builds and releases

**12.1** A build of rvr records, in every file it writes (`made.toolchain`): the program (`rvr`), its
version, the format version, the commit it was built from, its compiler (`rustc -V`) and its target
platform, under names any implementation of the format fills alike (FORMAT.md §6).

**12.2** The commit is, first that applies:

| built from | commit |
|---|---|
| a crate from crates.io | the commit `cargo publish` recorded |
| the Python source package | the commit the release wrote into it (its `COMMIT`, one line of 40 lowercase hex digits; anything else is `unknown`) |
| rowver's own git checkout, as committed | that commit |
| that checkout with changes under `crates/`, or to `Cargo.toml` or `Cargo.lock` | the commit, `-dirty-`, and 12 hex digits of git's hash of the changes, or `-dirty` when they cannot be read |
| anything else | `unknown` |

**12.3** A build whose commit is `unknown` or ends in `-dirty` presses whole files only (§7.5.6, §8.6).

**12.4** One tag `vX.Y.Z` on `main` makes a release of every crate and the Python package. Its
workflow tests everything, builds the Python wheels (Linux x86_64 and aarch64, manylinux 2.28; macOS
for both chips) and tests each on Python 3.11 and 3.14, builds the source package with the commit
written into it and checks it builds alone, builds the `rvr` binaries (Linux x86_64 on AlmaLinux 8,
for glibc 2.28 or later; macOS for both chips), and checks that every binary, every tested wheel and
the wheel built from the source package names the commit it was built from. Only then does it publish: the crates to crates.io, the Python package to PyPI, the binaries to
the GitLab release.
