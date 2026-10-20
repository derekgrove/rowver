# rowver

Read, validate and write `.rvr` files: event columns from a physics analysis, stored as
compressed arrays behind a JSON header you can read. Every file carries what made it (the
recipe, the samples it read and exactly which files, the tool and its commit), checksums of
every array, and optionally an ed25519 signature, so a file says on its own where its numbers
came from and that they have not changed. The crate's major version is the format version it
reads and writes (14).

```rust,no_run
use rowver::rvr::Reader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Opening checks the whole header; a file without a valid record of what made it is refused.
    let file = Reader::open("study.rvr")?;
    println!("{} events", file.rows()?);
    if let Some(met) = file.column("MET_pt")? {
        println!("MET_pt is {} ({} bytes)", met.kind, met.bytes.len());
    }
    // Check every array against its checksum.
    let summary = rowver::rvr::validate_file("study.rvr")?;
    println!("{} columns valid", summary.columns);
    Ok(())
}
```

- `rowver::rvr`: open, read and validate files (`Reader`, `validate_file`).
- `rowver::write`: build the arrays and write a file.
- `rowver::sign`: sign a header with ed25519 and check a signature.
- `rowver::choice`: the random event choice a file can record.

Arrays are byte-shuffled and compressed with Zstandard in pure Rust; reading a column is one read
and one decode. The format is specified in
[FORMAT.md](https://gitlab.cern.ch/dgrove/rowver/-/blob/rowver-v14.1.0/FORMAT.md).

The [`rvr`](https://crates.io/crates/rvr) command makes `.rvr` files from ROOT files and
inspects them; the Python package `rowver` reads them as NumPy and Awkward arrays.

Part of [RowVer](https://gitlab.cern.ch/dgrove/rowver). Licensed under the MIT license.
