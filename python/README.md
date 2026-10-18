# RowVer (Python presser)

The Python RowVer 13 turns ROOT ntuples into one `.rvr` file that you open in
[RowVer Web](https://rowver.web.cern.ch) to plot, cut, and make tables in the browser.

## Install

```bash
git clone https://gitlab.cern.ch/dgrove/rowver.git
cd rowver/python
mamba env create -f environment.yml
mamba activate rowver
pip install -e .
```

(`conda` works in place of `mamba`.)

## Run the demos

Each study is a folder under `studies/` with one `cart.py` in it.
`rowver run` writes the `.rvr` file into the same folder. Open
[RowVer Web](https://rowver.web.cern.ch) and drop the file onto the page.

### 1. CMS Open Data dimuons

Read over https, so it needs no grid certificate.

```bash
rowver run studies/dimuon_open_data
```

### 2–4. CMS files from DAS

These read `root://` files, so they need a grid proxy first. On lxplus the
certificate directory is already set; elsewhere, set it yourself:

```bash
voms-proxy-init --voms cms --valid 192:00
export X509_USER_PROXY=/tmp/x509up_u$(id -u)
export X509_CERT_DIR=/cvmfs/grid.cern.ch/etc/grid-security/certificates
```

Each remote file can take a few minutes.

```bash
rowver run studies/simple                   # 2. one MC file, branches copied as they are
rowver run studies/data_and_mc              # 3. data and MC in one file; data gets a weight of 1
rowver run studies/advanced --parallel 5    # 4. Run 2 and Run 3 data and MC, five files read at once
```

## Make your own study

```bash
mkdir studies/my_study
cp studies/simple/cart.py studies/my_study/
```

Edit `studies/my_study/cart.py`: point it at your files, choose which events
to `keep`, and list the `columns` you want. Hooks such as `keep` go above the
`# ---- recipe ----` line; below it, `FILESET`, `COLUMNS`, `LABELS` and `CART` hold plain
values, which is what lets the RowVer app edit the file in a form
([docs/recipe-dialect.md](docs/recipe-dialect.md)). Try the recipe on the first 1,000 events
before pressing everything:

```bash
rowver check studies/my_study             # the first sample; --sample NAME, or --all for every one
rowver probe root://.../some/file.root    # the trees and branches of one file, with first values
```

Then:

```bash
rowver run studies/my_study
rowver validate studies/my_study/my_study.rvr
```

## Look inside a file

```bash
rowver dump studies/my_study/my_study.rvr             # everything: header, recipe, first values of every column
rowver dump studies/my_study/my_study.rvr --fileset   # only the files that were read
rowver dump studies/my_study/my_study.rvr --cart      # only the recipe
rowver dump studies/my_study/my_study.rvr --columns   # only the first values of every column
rowver validate studies/my_study/my_study.rvr         # check the file agrees with itself
rowver diff old.rvr new.rvr                           # which column of which sample changed, from the headers alone
```

The file format is described in [FORMAT.md](../FORMAT.md).

`rowver diff` reads only the two headers: every column carries one checksum per
sample, so after pressing again it names each column and sample whose values changed,
the samples and columns only one file has, and the row counts that differ. It exits 0
when it finds no difference and 1 when it finds one.

A sample's metadata that is one number, text or truth value per sample, such as a
cross-section, is also written as a sample column: one value per sample, read for a
row through its sample index (`rvr.sample_column`), and compared by `rowver diff` too.

A file written by rowver 12 is refused; press its study again.

## When a file cannot be read

A file that cannot be opened or read is tried once more. If it fails again, the press
names it, leaves it out, and carries on with the other files; the summary at the end
says how many were left out. A sample none of whose files could be read is left out of
the `.rvr` file. When no file at all can be read, nothing is written.

## Adding columns to a file already pressed

```bash
python -m rowver run studies/my_study --onto studies/my_study/my_study.rvr
```

`--onto` adds the columns the recipe has and the file lacks, and makes none of the
others again: the sample files are read once more, `prepare` and `keep` run, and only
the new columns are computed. Nothing is added unless `keep`, `prepare`, the metadata
and every column the file holds are what they were when it was pressed, and every
sample file gives exactly the rows the file holds; the press then stops as `differs`
and says what changed, or as `nothing` when the recipe has no new column. The file is written beside its target and put in place whole,
so `--onto` may name the file it writes. `--plan` with `--onto` only says which
columns would be added. With `--progress json` the `wrote` line carries `added`, the
names of the new columns, and `--plan` prints one `plan` line with `columns`, `files`
and `skipped` (recipe files the cartridge was not pressed from, which are not read).
A derived file (below) is refused, as `differs`, and so is a new column with the name
of a sample column the file holds.

## Columns derived from a file's columns

```python
from rowver.derive import derive

def mt2(columns):
    return columns["MET"] * 2

derive("my_study.rvr", "my_study.mt2.rvr", source, {"mT2": mt2}, by="my program 1.0")
```

`derive` writes a second `.rvr` file that holds only the new columns, on the same rows
as its parent, without copying the parent's columns. The file records which parent it
came from by its checksums, which of the parent's columns were read, and `source`, the
Python text that computed the columns. Each function gets the parent's own columns and
sample columns by name and returns one number, or one list of numbers, per row. There
is no command for it. Check the pair with `rowver validate my_study.mt2.rvr --parent my_study.rvr`.
See [FORMAT.md](../FORMAT.md).

## For programs: `--out` and `--progress json`

```bash
rowver --version                                             # rowver 13.0.1
python -m rowver run studies/my_study --out /some/where.rvr --parallel 4 --progress json
```

`--out` writes the file somewhere other than `<study>/<study>.rvr`; a file half written
when a press stops is removed, and an `--out` that names a folder, or sits under a
file, ends with a `write` error. `--progress json` prints one JSON object per line on
stdout instead of the table, and a recipe's own `print` goes to stderr. The exit status
is 0 when the file was written and 1 otherwise. SIGTERM or SIGINT stops the press, ends
its workers and exits 1.

| `event` | fields |
| --- | --- |
| `start` | `study`, `samples`, `files`, `workers`, `presser` (the version), `out`, `inputs` (`[sample, path]` in recipe order) |
| `file` | `index` (into `inputs`), `path`, `state`, and by state: `opening` (again on the retry); `reading` with `done`, `total`, `kept` (one line per 100,000 entries); `done` with `entries`, `kept`, `seconds`; `failed` with `message`, the reader's own |
| `pack` | `rows`: every file is read and the columns are being written |
| `wrote` | `path`, `rows`, `columns`, `bytes`, `seconds`, `failed` (`[{sample, path, message}]`), `left_out` (samples with no file read) |
| `error` | `kind` (`recipe`, `input`, `write` or `cancelled`), `message`, `traceback` (empty for `input` and `cancelled`) |

`check`, `probe` and `recipe` speak JSON too, each with `--json`:

```bash
python -m rowver check studies/my_study --json [--sample NAME | --all] [--parallel N]
python -m rowver probe some/file.root --json [--tree Events]
python -m rowver recipe studies/my_study --json
```

- `check` prints a `start` line (`study`, `samples`, `workers`), then one line per sample:
  `checked` with `index`, `sample`, `path`, `tree`, `entries`, `read`, `kept`, `touched` (the
  branches the recipe read) and `columns` (`name`, `kind` of `flat`, `jagged` or
  `unsupported`, `type`, `first` values, and a `note` when a column has the wrong number of
  values); or `error` with `kind` `recipe` or `input`. A recipe that does not load, or a
  `--sample` that is not in it, is one `error` line and nothing else. It exits 0 when every
  sample was checked. Each sample costs one file open, so `--all` on many remote samples takes a while.
- `probe` prints one `probe` line: `path`, `uuid`, `seconds` and `trees`, each with `name`,
  `entries` and `branches` (`name`, `type`, `kind`, the first three values). NaN and
  infinities come as the text `"nan"`, `"inf"` and `"-inf"`.
- `recipe` reads `cart.py` without running it and prints what the app's form needs, or why
  the file is shown as text; [docs/recipe-dialect.md](docs/recipe-dialect.md) has the fields.

With `--parallel`, the recipe first runs on the first 1,000 events of the first file
in the presser itself, so a broken hook is an `error` of kind `recipe` before any worker
starts. Workers write their own `file` lines, each shorter than 512 bytes so lines from
different workers never mix.

## Tests

```bash
python -m pytest tests        # or, without pytest: python -m unittest discover -s tests
```

The tests write small ROOT files and press them locally. One test opens the pressed file
with RowVer Web's own reader; it runs when `ROWVER_WEB` names a rowver_web checkout with
a built `dist/validate.mjs`, and is skipped otherwise.
