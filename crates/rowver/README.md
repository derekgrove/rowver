# rowver (Rust)

Read, validate, and write RowVer 14 `.rvr` files in Rust. The format is specified by
`FORMAT.md` at the root of this repository, which this crate implements; the crate's major
version is the format version.

- `rowver::rvr` reads and validates pressed and derived files: `Reader` opens a file and checks
  its whole header, then reads the columns asked for; `validate_file` checks every array too.
- `rowver::write` builds the data section and writes a file.
- `rowver::choice` is the random event choice a file names (`splitmix64-selection-sampling`).
- `rowver::sign` signs a header with ed25519 and checks a signature.

This crate is the reference builder of `.rvr` files.

## Compression and speed

Arrays are byte-shuffled, then one Zstandard frame each, compressed and read in pure Rust
(`ruzstd`, the same crate hep-root reads ROOT's zstd baskets with). `ruzstd` has only its
fastest level.

`build_with` compresses, checksums (`sha256` and `by_sample`) and measures the facts of every
array on its workers; the bytes do not depend on how many. `Reader` reads each array it is asked
for with one read of its compressed span and one decode into a buffer of its declared length.
On 2,104,806 rows × 27 columns (227 MB of arrays, Apple M5, 10 threads), against rowver 13's
zlib 6: build 0.32–0.38 s (13: 0.37 s), open and read every column 0.054–0.059 s
(13: 0.051–0.057 s), 143.0 MB of arrays (13: 131.0 MB) (`rvr_format_study/results/RUST.md`).

## Fixtures

`tests/fixtures/rowver-v14.hex` is the hexadecimal text of a small file this crate's writer
made (`tests/common/mod.rs`); the reader tests read it as committed, so a change that stops a
reader from reading files already written fails them. `ROWVER_BLESS=1 cargo test -p rowver`
writes it again, for a change to the format that is meant.

MIT licensed.
