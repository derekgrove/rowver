# RowVer

RowVer turns ROOT ntuples into one `.rvr` file that you open in
[RowVer Web](https://rowver.web.cern.ch) to plot, cut, and make tables in the browser.
A `.rvr` file is a JSON header followed by compressed arrays, and it carries the recipe that
made it. [FORMAT.md](FORMAT.md) is the specification.

| folder | what it is |
|---|---|
| [`crates/rvr`](crates/rvr/README.md) | the `rvr` command: start a study, resolve its samples, press them, and validate, dump and diff `.rvr` files |
| `crates/definition` | the recipe language: TOML whose expressions are evaluated over whole columns of events |
| [`crates/rowver`](crates/rowver/README.md) | the Rust library that reads, validates and writes `.rvr` files; the reference builder of the format |
| [`python/`](python/README.md) | the Python presser (RowVer 13), kept until `rvr press` replaces it everywhere (the app still uses it) |

## Install `rvr`

You need Rust 1.91 or later (`rustup update`).

```sh
cargo install --git https://gitlab.cern.ch/dgrove/rowver rvr
```

The shared HEP crates it uses (the ROOT reader and the grid tools) come from
[hep-crates](https://gitlab.cern.ch/dgrove/hep-crates).
