# Narrow RowVer to pressing cartridges

Decided 2026-09-10.

RowVer does one thing: it presses ROOT ntuples into `.rvr` cartridges. The
product is `rowver new`, `rowver run`, `rowver check`, and `rowver show`, plus
the `Cart` recipe API and the `rvr6` reader and writer. Everything else that
grew alongside the presser leaves this repository:

- **Extraction and rendering** (`rowver extract …`, `rowver render`, the
  `rowver.extract` package, and `extract/core.py`, the Python twin of the
  viewer's authored-state resolver) are verification tooling under ADR 0004.
  They move byte-for-byte to `rowver_verification`, with their tests and the
  `contract/core` corpus. The `v6` and `v7` corpora stay here.
- **The browser builder** (`rowver.builder`, `rowver.builder_spec`) serves only
  `rowver_web`. It leaves this repository; whether it is deleted or relocated
  next to the viewer is decision D1 in `docs/plan-bare-bones-presser.md`.
- The root shims `press.py`, `cart.py`, `rvr6.py`, and `run_rowver.py` are
  deleted. Recipes import `Cart` from the `rowver` package.

Runtime dependencies reduce to numpy, awkward, and uproot. aiohttp, matplotlib,
and mplhep go with the code that needed them.

## Format

Storage is no longer a reason to change the format. The type pilots of
2026-09-07 (`development/type_pilots/REPORT.md`, a private untracked record)
pressed the 18-file tau skim set, 3.5 million events, both ways: preserving the
ROOT scalar types instead of widening to `f8`/`i4` saved 1.06 MB of 148 MB
compressed, 0.72 %. The one measured win was bit-packed booleans, and that is
the whole of version 7 (`FORMAT7.md`): v6 plus `b1` columns.

Version 8, if it comes, is a fidelity change, not a storage one, and is limited
to three things: provenance for each column (copy or derived, its source, and
payload hashes), jagged branches stored as first-class copies the viewer skips,
and the dtype decision D7 (native scalar types in, or keep value-exact copies as
`f8`/`i4`/`b1`). Nothing else is on the table.

## Consequences

- Fewer dependencies, one CLI surface, and a test suite that proves only
  pressing.
- Analysts who used `rowver extract` or `rowver render` run the same modules
  from `rowver_verification`.
- The independence boundary in `docs/agents/verification.md` moves Python
  extraction and the `core` corpus to `rowver_verification`.
- `check_corpus_sync.py` pairs `core` as `rowver_verification` ↔ `rowver_web`
  and keeps `v6`/`v7` as `rowver` ↔ `rowver_web`.

Supersedes nothing. ADRs 0001–0008 stand; ADR 0004's independent
implementations are unchanged, only the home of the Python twin moves.
