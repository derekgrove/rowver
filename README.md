# RowVer

RowVer turns ROOT ntuples into one `.rvr` file: the events a recipe keeps, the columns it
stores, and everything needed to say how the file was made.
A `.rvr` file is a JSON header followed by compressed arrays, and it carries the whole study
that made it (recipe, samples and lock), so it can be pressed again from itself. [FORMAT.md](FORMAT.md) is the specification.

| folder | what it is |
|---|---|
| [`crates/rvr`](crates/rvr/README.md) | the `rvr` command: start a study, resolve its samples, press them, and validate, dump and diff `.rvr` files |
| `crates/recipe` | the recipe language of `rvr press`: TOML whose expressions are evaluated over whole columns of events |
| [`crates/rowver-py`](crates/rowver-py/README.md) | the Python package `rowver`: `.rvr` files as NumPy, Awkward, Arrow, pandas and Parquet, and the `rvr` command (`pip install` from that folder) |
| [`crates/rowver`](crates/rowver/README.md) | the Rust library that reads, validates and writes `.rvr` files; the reference builder of the format |

## Install `rvr`

You need Rust 1.91 or later (`rustup update`).

```sh
cargo install --git https://gitlab.cern.ch/dgrove/rowver rvr
```

The shared HEP crates it uses (the ROOT reader and the grid tools) come from
[hep-crates](https://gitlab.cern.ch/dgrove/hep-crates).
