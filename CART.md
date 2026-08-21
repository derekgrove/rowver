# The cart — unifying the fileset and the config

*Design proposal, branch `dev-cart`, 2026-08-21. Nothing is built yet. This file is the
spec the branch gets built to; it is deleted (folded into README) when the work lands.*

## What a cart is

A `.rvr` is a cartridge. Plug it into the viewer and it plays — and it also tells you,
without any other file, **exactly what was put on it**: every sample, every complete
path (local or remote), every tree name, every sample's metadata, which columns were
made and how, which function selected the events, and when.

Today a `.rvr` carries the *data* and none of that. A study's identity lives in two
files that must be kept in step by hand:

    studies/<name>/fileset.json    which files, which trees, per-sample metadata
    studies/<name>/config.py       KEEP, COLUMNS, PREPARE, LABELS, HAS_WEIGHTS, ...

`Cart` is one object that owns both, hands each hook what it needs, and stamps the whole
thing into the file it burns.

## The gap this closes, in one real example

`studies/ku_2l_run2/config.py` carries this warning, in the repo, today:

> ⚠️ `run_rowver.py ku_2l_run2` WILL NOT REPRODUCE THE .rvr. The per-era luminosity is
> folded into the weight AFTER the skim, per file, which run_rowver has no way to do — a
> straight run would give every era the same weighting and be wrong by up to a factor of
> 3.5 between UL16 and UL18.

That study needed a scratchpad driver (`run2_skim.py` + `run2_assemble.py`) because a
`COLUMNS` lambda cannot see **which sample** it is running on, let alone **which file**.
The metadata it needs is already sitting in the fileset:

```json
"metadata": {"isRun3": false,
             "eras": {"Summer20UL16APV_106X": 19.5, "Summer20UL16_106X": 16.8,
                      "Summer20UL17_106X": 42.07, "Summer20UL18_106X": 59.56}}
```

With metadata reaching `COLUMNS`, the whole scratchpad driver becomes one line of config:

```python
"weight": lambda a, m: a.weight * m["eras"][era_of(m["file"])] / 137.93,
```

That is the generality being asked for. Everything below serves it.

---

## 1. `cart.py` — the class

`samples.py` and `study.py` are absorbed into it. `pikl.py`, `load.py`, `page.py` stay.

```
studies/<name>/
    cart.py         the study: one Cart instance, named CART
    fileset.json    optional — a big machine-generated fileset stays JSON
    view.py         optional, unchanged
```

**The fileset stays coffea's shape** — `{sample: {"files": {path: tree}, "metadata": {}}}`
— written inline in `cart.py`, or loaded from `fileset.json`, or discovered from a
directory. The Cart does not care which; it holds one dict either way.

```python
class Cart:
    """One study: its fileset, its selection, its columns — and the .rvr it burns to."""

    def __init__(self, name, fileset, columns, keep, prepare=None,
                 labels=None, weights="weight", title="")

    @classmethod
    def load(cls, name)          # read studies/<name>/cart.py
    @classmethod
    def open(cls, path)          # read a .rvr back — fileset, columns, stamp, arrays

    def files(self)              # {sample: {path: tree}}
    def meta(self, sample)       # that sample's metadata, plus sample/file/tree
    def check(self)              # every sample matches a file, every file holds its tree
    def summary(self)            # one line per sample; the cartridge label
    def skim(self, step, jobs, threads)     # -> {sample: {column: ndarray}}
    def stamp(self)              # -> the dict that goes in META["cart"]
    def write(self, dest, tabs, extra)      # -> <dest>/<name>.rvr, stamped
```

`Cart.open()` is the plug-and-play half: drop any `.rvr` on it and get the fileset back
out, complete paths and all, without the study directory that made it.

## 2. Metadata reaches every hook

One rule, everywhere: **any hook may take a second argument, that sample's metadata.**
`PREPARE` and `KEEP` already do (`with_meta` in `pikl.py`). This extends the same call to
`COLUMNS` lambdas, which is the only piece missing.

```python
def PREPARE(a, m): ...
def KEEP(a, m):    ...
COLUMNS = {"weight": lambda a, m: ...}      # NEW — a one-argument lambda still works
```

The metadata handed over is the sample's own, plus three keys the skim knows and the
fileset cannot:

    sample    the sample name
    file      the path of the file this chunk came from — local or remote, complete
    tree      its tree name

Those three names are reserved: a fileset that already uses one fails loudly at load,
rather than having its value quietly overwritten.

## 3. Weights: data and MC in one cart

The page's weights model is global and stays that way — `META.weights` names one column,
`D.__w` is one `Float64Array`, and per-group `lumis` already scales groups differently.
The page needs no change and learns nothing about data vs MC. **The distinction is the
user's, made with groups**, exactly as it is now.

What changes is who fills the column. The Cart declares its name once, and each sample
says what goes in it — **two rules and an off switch**:

1. If `COLUMNS` defines the weights column, that wins (today's behaviour, untouched).
2. Otherwise each sample's `metadata["weights"]` names its own branch, whatever it is
   called — and **a sample without one is filled with 1.0.**
3. `weights=None` writes no weights column at all (today's `HAS_WEIGHTS = False`).

So a mixed cart needs no config code at all, only a fileset:

```json
{
  "ttbar 2018":       {"files": {...}, "metadata": {"is_MC": true,  "weights": "genWeight"}},
  "DY 2018":          {"files": {...}, "metadata": {"is_MC": true,  "weights": "weight"}},
  "SingleMuon 2018":  {"files": {...}, "metadata": {"is_MC": false}}
}
```

`HAS_WEIGHTS` and `WEIGHTS_BRANCH` retire into the Cart's `weights=` argument. The seven
shipped studies get updated with it; no compatibility shim.

⚠️ **The trap this creates, and it is a real one.** The page computes yield as
`w × LUMI(group)`. Data events carry `w = 1.0`, so a data group left at the tab's
luminosity is scaled by 137.93 and reads 137.93× its own event count. The fix exists
already — set that group's entry in per-group `lumis` to 1 — but nothing makes you.
**Proposed: the Cart prints the warning at build time**, naming the unweighted samples
and the `lumis` entry they need. Not a page change, not a format change. See decision D.

## 4. The stamp — what a cart records about itself

One new optional META key, `cart`. It replaces `origin`, which only helper builds wrote.

```
cart {
  name      str                    the study
  built     str                    ISO-8601 UTC — when this file was burned
  made_by   str                    the RowVer version (already in META.made_by)
  fileset   {sample: {files: {path: tree}, metadata: {...}}}
                                   coffea shape, globs already expanded, complete paths
  columns   {name: str}            per column, how it was made: the branch name it
                                   copied, or the source line of the lambda that
                                   computed it
  keep      str                    KEEP's name and the first line of its docstring
  prepare   str | null             the same for PREPARE
  weights   str | false            the weights column's name
  config    {path, mtime, sha256, source}
                                   the file that defined it, and its text
}
```

Measured cost (topic file, 2026-08-21): the repetitive fileset JSON gzips about 16:1, so
**1,000 files cost +14.9 KB**, 5,000 cost +60 KB, and a config's source is ~1 KB
gzipped. Against a realistic 10 MB study a 1,000-file fileset is ~0.15%.

Two things worth saying out loud about this:

- ⚠️ **`built` costs byte-determinism.** `remote/helper.py` deliberately writes no
  timestamp and gzips with `mtime=0` so two builds of one spec produce byte-identical
  files (the R3 property). A timestamp ends that. The helper's cache is keyed on the
  *spec* sha, not the bytes, so nothing breaks operationally — a cached file simply
  carries the first build's timestamp. See decision C.
- ⚠️ **A cart now carries your paths and your config source.** eos paths, usernames,
  local directory layout, and whatever your `config.py` says all travel with a file you
  might send to someone. `origin` already carried the paths; the source text is new.

## 5. Format version

Nothing in the binary layout moves — the stamp is an additive META key, exactly as
`origin` was. **Recommendation: stay at `rowver 4`.** A bump would cost the nine frozen
goldens their gate for no gain; the presence or absence of `cart` already tells a
pre-cartridge file from a cartridge one. See decision A.

## 6. What gets touched

| file | change |
|---|---|
| `cart.py` | NEW — the class |
| `samples.py`, `study.py` | DELETED, absorbed |
| `pikl.py` | `with_meta` extended to `COLUMNS`; weights fill |
| `run_rowver.py` | drives a Cart |
| `page.py` | `pack`/`build`/`data`/`export` take the stamp |
| `remote/helper.py` | writes `cart` instead of `origin` |
| `smoke.py` | line 1765 (`study.weights`); a mixed data+MC golden |
| `studies/*/` | seven studies moved to `cart.py` |
| `README.md`, `handbook/` | the "Adding a study" section |

Untouched: `page_template.html`, `load.py`, `bridge/`, `yields.py`, `golden/`.

## Staging

1. **Core** — `cart.py`, metadata into `COLUMNS`, per-sample weights, the stamp,
   `run_rowver.py` on top of it, seven studies converted, battery green, plus a new
   golden that is genuinely data + MC in one file.
2. **Builder** — `remote/helper.py` writes the same stamp; the console shows the label.
3. **Docs** — README, handbook, the `ku_2l_run2` warning deleted because it is fixed.

---

## Decisions needed before code

**A. Format version.** Stay at `rowver 4` (my recommendation — nothing in the layout
moves, and the goldens keep gating), or bump to 5 so a cartridge file is version-visible?

**B. Config source in the stamp.** Embed the `config.py` text (~1 KB gzipped, makes the
cart genuinely self-describing), or record only path + mtime + sha256? Note a config that
imports a private framework module is not reproducible from its text either way.

**C. Timestamps vs byte-determinism.** `built` is explicitly asked for and it ends
"two builds of one spec are byte-identical". Keep the timestamp (my recommendation —
provenance is the point of a cartridge, and the helper caches on the spec sha), or make
it optional so the helper can leave it out?

**D. The data-lumi trap.** Cart prints a build-time warning naming unweighted samples and
the `lumis` entry they need (my recommendation), or leave it entirely to the user?

**E. One file or two.** Does `fileset.json` survive beside `cart.py` for big
machine-generated filesets (my recommendation — `ku_2l_run2` is ~100 samples × 4 files
and belongs in JSON), or is "unified" meant literally, one file per study, always?
