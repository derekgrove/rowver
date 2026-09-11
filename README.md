# RowVer presser

RowVer turns ROOT ntuples into a shareable `.rvr` file for interactive plots, tables,
cuts, and region maps. This repository is the presser; a matching RowVer Web build
opens the cartridges entirely in the browser.

## Quick start: DATA + MC

The smallest complete example is
[`studies/jetmet_2018_simple/cart.py`](studies/jetmet_2018_simple/cart.py). It reads one
2018 MET DATA file and one UL18 TTJets MC file from CMS DAS. Every path, tree, column,
selection, and weight declaration is written directly in the study—there are no setup
helpers to follow.

```bash
git clone https://gitlab.cern.ch/dgrove/rowver.git
cd rowver
mamba env create -f environment.yml
mamba activate rowver
mamba install -n rowver xrootd voms
pip install -e .
voms-proxy-init --voms cms --valid 192:00
rowver run jetmet_2018_simple
```

This writes `studies/jetmet_2018_simple/jetmet_2018_simple.rvr`. Open
[RowVer Web](https://rowver.web.cern.ch), drop the `.rvr` file onto the page, and start
plotting. The two lines that keep DATA and MC yields correct are visible beside each
sample:

```python
"metadata": {"kind": "data", "weights": None}       # do not weight DATA
"metadata": {"kind": "mc", "weights": "genWeight"}  # weight MC event by event
```

The remote files require CMS grid access. If your site does not set the proxy and CA
locations automatically, use the setup under [Remote CMS files](#remote-cms-files).
For a larger local stress study with reusable helpers, see
[`studies/jetmet_2018_advanced/cart.py`](studies/jetmet_2018_advanced/cart.py).

## Install

RowVer is a pip-installable package with one CLI:

```bash
pip install rowver          # from a checkout: pip install .
rowver new mystudy
# Edit mystudy/cart.py: files, weights, baseline selection, and columns.
rowver run mystudy/
```

For development, mamba resolves the HEP Python stack quickly and reproducibly:

```bash
git clone https://gitlab.cern.ch/dgrove/rowver.git
cd rowver
mamba env create -f environment.yml
mamba activate rowver
pip install -e .
```

Conda can use the same file: `conda env create -f environment.yml`.

## Remote CMS files

Remote files (`root://…` in a fileset) additionally need the XRootD client —
`mamba install -n rowver xrootd` — and, for GSI-protected redirectors such as
FNAL EOS, a valid grid proxy in the shell that runs the press. To obtain one
you need your grid certificate installed in `~/.globus/` (usercert.pem +
userkey.pem, from your VO's certificate authority) and the VOMS client tools
(`mamba install -n rowver voms` or your site's `voms-proxy-init`), then:

```bash
voms-proxy-init --voms cms --valid 192:00   # your VO; renew when it expires
export X509_USER_PROXY=/tmp/x509up_u$(id -u)
export X509_CERT_DIR=<your CA certificates directory, e.g. /cvmfs/grid.cern.ch/etc/grid-security/certificates>
```

`voms-proxy-info --timeleft` shows what remains on the proxy; rerun
`voms-proxy-init` to renew it. On lxplus and most CMS/ATLAS interactive nodes
the certificate tooling and `X509_CERT_DIR` are already in place.

Purely local filesets need none of this.

## Build your cartridge

```bash
rowver new mine
# Edit mine/cart.py: files, weights, baseline selection, and columns.
rowver run mine/
```

`rowver run` takes any study directory path; a bare name is also looked up
under `./studies/`. The output is `mine/mine.rvr`. Drop it onto the web app, arrange the page, and
save it. Send that one file to a collaborator; they can reopen it and change the cuts or
binning without your ROOT files or Python environment.

Relative file paths and globs in `cart.py` resolve from that study directory.

Your `cart.py` must say how every sample is weighted:

```python
"metadata": {"weights": "genWeight"}  # simulated events
"metadata": {"weights": None}         # any intentionally unweighted sample
```

Weights may also be a function. An omitted declaration is rejected so forgotten
weights cannot silently look intentional.

The display is live and one-pass: it reports the current sample and source,
scanned and kept rows, elapsed time, and cartridge pressing without opening
sources merely to calculate progress. `--no-live` is plain one-pass execution
with no dashboard:

```bash
rowver run --no-live mine/
```

Pressing is sequential by default. Studies with several independent, slow sources
can opt into source-level worker processes:

```bash
rowver run mine/ --parallel 4   # at most four workers
rowver run mine/ --parallel     # all but two logical CPUs, capped by source count
rowver run mine/ --parallel max # all logical CPUs, capped by source count
rowver run mine/ --parallel 1   # the sequential implementation
```

Before source processing, RowVer reports the workers it will use and the cores
available; default sequential runs also suggest `--parallel`.

Results are always merged in Study declaration order, and only the coordinator
packs, validates, compresses, reports progress, and publishes the cartridge.
`--no-live --parallel` constructs no dashboard. Remote services
may throttle concurrent readers, so choose an explicit count when site-specific
evidence favors a lower limit.

Useful commands:

```bash
rowver run --list
rowver run mine/ --out /path/to/result.rvr
rowver run mine/ --parallel 4
rowver run --no-live mine/
rowver check /path/to/result.rvr
rowver show /path/to/result.rvr
python gate.py          # release gate, from a checkout
sh wheel_gate.sh        # the documented workflow from an installed wheel in a clean venv
```

## Programmatic use

A study's `cart.py` recipe is the front door (the library that presses it is
`rowver/press.py`; `from rowver import Cart`), but a script can also write a
cartridge directly:
`meta, buf = rvr.pack(by_sample, weighted=..., weights=..., made=...)` then
`rvr.write(path, meta, buf)`. Both `write` and `read` run `rvr.check` internally, so an
invalid file can be neither produced nor opened. `contract/v9/cases.json` holds the worked
example of a valid header.

## What is shared

A cartridge includes the selected columns and a snapshot of the recipe: the
whole `cart.py` text, each sample's metadata, the bare file names with their
tree and entry count, and the presser version that built it. The file records
carry no directory, but the recipe text is stored verbatim, input paths and
URLs included. Review that snapshot before sharing outside your collaboration.

The format is `.rvr` 9, specified whole in [FORMAT.md](FORMAT.md). A cartridge
carries only the data, its statistics, and the recipe snapshot; a tool that
keeps state in one does so under a top-level key of its own. `rowver/rvr.py` is
the reference implementation, and `contract/v9/cases.json` is the corpus an
independent reader must agree with. The RowVer Web reader for version 9 is a
pending handoff; until it lands the viewer opens only earlier cartridges.
