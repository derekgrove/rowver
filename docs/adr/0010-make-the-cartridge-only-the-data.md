# Make the cartridge only the data

Decided 2026-09-11.

A `.rvr` cartridge is the data product and nothing else: the columns of a
study, the statistics that describe their distributions, and a snapshot of the
recipe that produced them. It knows nothing about any tool that reads it.
Version 9 of the format (`FORMAT.md`) is this decision written as a
specification.

- The header is `rowver`, `data`, `made`, and consumer keys. `data` holds the
  buffer geometry, statistics, provenance, and digests; `made` holds the
  recipe snapshot with the study's identity at its head (`study_name`,
  `title`, `note`), and the whole snapshot is required.
- A tool that keeps state in a cartridge keeps it under one top-level key
  named for the tool, as an object. The presser never writes such a key and
  every writer preserves it. The viewer's `view`, `tabs`, and `opens_on`
  drawers and the `about` drawer leave the file and are rejected if they
  reappear.
- Distribution statistics stay because every plotting consumer benefits:
  `lim`, `nice`, all thirteen percentiles, `vals` with no cap, and value
  labels as `data.cols[].names`.
- Sample and file records are keyed objects (sample name, then ROOT file
  UUID) holding the bare filename, tree, and entry count; no path is stored.
  Four digests prove the bytes (the buffer and each column), the recipe
  text, and the snapshot.
- There is one version. Versions 6 to 8 are deleted from the repository with
  their documents and corpora; nothing reads them, and `rvr6.py` becomes
  `rvr.py`. The project has no users yet, so no file is stranded.

The full record is D9 to D21 in `docs/plan-bare-bones-presser.md` and
`docs/handoffs/2026-09-11-rvr-is-only-data.md`.

## Consequences

- The validator loses the page grammar, roughly forty percent of the module,
  and the corpus loses every page-grammar case.
- `rowver_web` validates and owns its own state under its key and refuses
  cartridges of any other version; ADR 0004's independent implementations
  cover the data contract only. That work is the viewer handoff after plan
  step 9.2.
- `rowver_verification`'s `extract/core.py` and `extract/flattened.py` read
  the viewer's key instead of `view` and `tabs`.
- Plan steps 9.1 to 9.4 carry the change: this record, code and corpus, the
  docs sweep, and the recipe-snapshot acceptance test.

ADR 0001 stands: the file is still the unit of exchange, and a viewer's page
state travels in it under the viewer's key. Supersedes the format section of
ADR 0009 (the v6 and v7 corpora do not stay, and the reader module is `rvr`,
not `rvr6`) and the "reads v6 and v7" row of the plan's target state.
