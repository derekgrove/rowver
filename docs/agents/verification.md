# Independent verification

The private `dgrove/rowver_verification` repository verifies RowVer from outside
the product build. It owns independent numerical recomputations, stress studies,
external-framework comparisons, and frozen trust anchors. Product code and
ordinary tests remain in this repository.

## When this applies

Read the verification repository's `README.md` and `FROZEN.md` when changing:

- `.rvr` parsing, validation, writing, or shared contract fixtures;
- selection, weighting, luminosity, binning, or extraction semantics;
- local, EOS, DAS, or mixed-source pressing behavior;
- outputs compared with the viewer or an external analysis framework.

If the private checkout is available as `../rowver_verification`, use its relevant
outside-in check after this repository's own tests pass. At minimum, changes to
either shared corpus must pass:

```bash
python ../rowver_verification/check_corpus_sync.py
```

The verification README selects the appropriate site, yield, or stress tool for
other changes. Record the command, fixture, and result in the issue or handoff so
the evidence is reproducible.

## Independence boundary

- `rowver` owns cartridge creation, source-data access, and the canonical v6
  and v7 fixture corpora (ADR 0009).
- `rowver_web` owns browser interaction, rendering, save fidelity, viewer
  security, offline packaging, and deployment.
- `rowver_verification` owns checks that reproduce product observables without
  sharing their result-producing logic, Python extraction and rendering, the
  Python twin of the viewer's authored-state resolver, and the `core` fixture
  corpus.

Keep verification out of product imports, runtime dependencies, and release
artifacts. A byte reader may be reused only when the verification explicitly
states that byte access is outside the behavior being proved. A frozen verifier
changes only under the procedure in `rowver_verification/FROZEN.md`.

If the private checkout or its required data is unavailable, run the complete
local product gate and report independent verification as not run. Its absence is
never evidence that a verification requirement passed.
