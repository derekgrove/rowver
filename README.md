# RowVer cartridge builder

RowVer turns ROOT ntuples into a shareable `.rvr` file for interactive plots, tables,
cuts, and region maps. This repository builds cartridges; a matching RowVer 6 Web build
opens them entirely in the browser.

## Install

Mamba is recommended because it resolves the HEP Python stack quickly and reproducibly.

```bash
git clone https://gitlab.cern.ch/dgrove/rowver.git
cd rowver
mamba env create -f environment.yml
mamba activate rowver
```

Conda can use the same file: `conda env create -f environment.yml`.

## Build your cartridge

```bash
cp -R studies/template studies/mine
# Edit studies/mine/cart.py: files, weights, baseline selection, and columns.
python run_rowver.py mine
```

The output is `studies/mine/mine.rvr`. Drop it onto the web app, arrange the page, and
save it. Send that one file to a collaborator; they can reopen it and change the cuts or
binning without your ROOT files or Python environment.

Relative file paths and globs in `cart.py` resolve from that study directory.

Your `cart.py` must say how every sample is weighted:

```python
"metadata": {"weights": "genWeight"}  # simulated events
"metadata": {"weights": None}         # observed data
```

Weights may also be a function. An omitted declaration is rejected so forgotten MC
weights cannot silently look like data.

Useful commands:

```bash
python run_rowver.py --list
python run_rowver.py mine --out /path/to/result.rvr
python rvr6.py check /path/to/result.rvr
python rvr6.py show /path/to/result.rvr
python gate.py
```

## Programmatic use

A study's `cart.py` recipe is the front door (the library that presses it is
`press.py`), but a script can also write a cartridge directly:
`meta, buf = rvr6.pack(by_sample, weighted=..., weights=...)` then
`rvr6.write(path, meta, buf)`. Both `write` and `read` run `rvr6.check` internally, so an
invalid file can be neither produced nor opened. `contract/v6/cases.json` holds the worked
example of a valid header.

The browser builder is experimental. After building the frontend's `viewer.html`, start its
authenticated local helper with
`python builder.py --root data=/path/to/ntuples --ui ../rowver_web/viewer.html`. The printed
URL contains the pairing token. Arbitrary Python expressions remain a `cart.py` feature and
are never evaluated by the helper.

## What is shared

A cartridge includes the selected numeric columns and the exact `cart.py` provenance,
including input paths. Review that metadata before sharing outside your collaboration.

RowVer supports `.rvr` version 6 only. [FORMAT.md](FORMAT.md) defines the contract and
`rvr6.py` is the reference implementation.
