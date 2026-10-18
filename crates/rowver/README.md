# rowver (Rust)

Read, validate, and write RowVer v13 `.rvr` files in Rust. The format is specified by
`FORMAT.md` at the root of this repository, which this crate implements and does not change.

- `rowver::rvr` reads and validates pressed and derived files.
- `rowver::write` builds the data section and writes a file.

Each module that reproduces Python behaviour names, in its header, the package, version,
functions, and documentation it follows. The tests compare files written here with files the
Python RowVer 13.0.0 wrote from the same arrays, and they agree.

This crate is the reference builder of `.rvr` files. The Python RowVer was retired after 13.0.1
(tag `v13.0.1`).

## Compression and speed

Arrays are zlib streams at level 6, compressed in pure Rust (flate2's `zlib-rs` backend). Such
files hold the same arrays and checksums as the Python RowVer's, but their compressed bytes, and
so each array's `at` and `size`, differ.

`build_with` compresses, checksums (`sha256` and `by_sample`) and measures the facts of every
array on its workers; the bytes do not depend on how many. `Reader` reads each array it is asked
for with one read of its compressed span and one inflate into a buffer of its declared length.
On 2,104,806 rows × 27 columns (231 MB of arrays, Apple M5, 10 cores): build 0.37 s, read every
column 0.05 s (`rvr_format_study/results/RUST.md`).

## Fixtures

The tests read `tests/fixtures/*.hex`, the hexadecimal text of two small files written by
the Python RowVer 13.0.0 from synthetic values; `tests/generate_core_fixture.py` regenerates
them with the Python RowVer of tag `v13.0.1` installed.

MIT licensed.
