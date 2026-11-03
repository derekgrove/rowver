# rowver

Read, validate and write `.rvr` files: event columns from a physics analysis, stored as
compressed arrays behind a JSON header you can read. Every file carries what made it (the
recipe, the samples it read and exactly which files, the tool and its commit), checksums of
every array, and optionally an ed25519 signature, so a file says on its own where its numbers
came from and that they have not changed. The crate's major version is the format version it
reads and writes (15).

```rust,no_run
use rowver::rvr::Reader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Opening checks the whole header; a file without a valid record of what made it is refused.
    let file = Reader::open("study.rvr")?;
    println!("{} events", file.rows()?);
    if let Some(met) = file.column("MET_pt")? {
        println!("MET_pt is {} ({} bytes)", met.kind, met.bytes.len());
    }
    // Check every array against its checksum, and work out every derived column.
    let summary = rowver::rvr::validate_file("study.rvr")?;
    println!("{} columns and {} derived valid", summary.columns, summary.derived);
    Ok(())
}
```

- `rowver::rvr`: open, read and validate files (`Reader`, `validate_file`). `Reader::column`
  reads a derived column too: worked out from the file's columns with the recipe language
  ([`rvr-recipe`](https://crates.io/crates/rvr-recipe)), and checked against its record
  (`rowver::rvr::derived`). `width` and `push_shown` give a kind's width and its values as text,
  and `span` a run of entries in words.
- `rowver::write`: pack the arrays a piece at a time, through a spool on disk, and write a file
  (`stream`, `write_laid`); measure a derived column's record (`derived`), write a file again
  with another header (`rewrite`), and empty and refill one.
- `rowver::sign`: sign a header with ed25519; a reader checks the signature when it opens a file.
- `rowver::plain`: what went wrong with a file, in plain words (`FILE is not there`, `FILE changed
  while it was read …`). A `Reader` reads through the handle it opened, so a file renamed over it
  meanwhile is read as it was.
- `rowver::choice`: the random event choice a file can record.

Arrays are byte-shuffled and compressed with Zstandard in pure Rust; reading a column is one read
and one decode. The format is specified in
[docs/FORMAT.md](https://gitlab.cern.ch/dgrove/rowver/-/blob/v15.0.0/docs/FORMAT.md).

The [`rvr`](https://crates.io/crates/rvr) command makes `.rvr` files from ROOT files and
inspects them; the Python package `rowver` reads them as NumPy and Awkward arrays.

Part of [RowVer](https://gitlab.cern.ch/dgrove/rowver). Licensed under the MIT license.
