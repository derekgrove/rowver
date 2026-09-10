# RowVer product status

This is a dated status baseline, not a specification or backlog. It was split
from the retroactive design record written on 2026-08-29 against RowVer Web
`ce90b16`, RowVer `fd5931a`, and RowVer Verification `a8ac54c`; current repository
behavior takes precedence. On 2026-09-10 the scope was narrowed to pressing
(ADR 0009): extraction and rendering left this repository, and the format is v7.

## Product

RowVer is a web app for CMS high-energy physicists to analyze collision and
simulated Monte Carlo data. A Python presser filters ntuples into a compact
`.rvr` v6 cartridge; the browser viewer opens it for plots, cut sliders, yield
tables, and region maps without an installation or server. Collaboration is
pass-a-file: a cartridge carries the author's view and a collaborator can adjust
its selections locally. There are no accounts, synchronization, or hosted
backend.

This repository owns cartridge creation, source-data access, and the canonical
v6 and v7 contract and fixture corpora. The `rowver_web` repository owns browser
interaction, rendering, save fidelity, viewer security, offline packaging, and
deployment. The `rowver_verification` repository owns independent end-to-end
comparisons of viewer results and, since 2026-09-10, Python extraction and
rendering.

## Current capabilities and limits

- **Remote, credentialed data:** the presser reads local files and `root://`
  URLs, including mixed filesets, once the analyst has configured the required
  CERN credentials. RowVer does not provision or delegate those credentials.
- **Workflow-neutral primitives:** plots, yield tables, region maps, selections,
  and sample grouping do not hard-code an analysis workflow. They have so far
  been exercised mainly by one analysis group, so broader generality remains a
  design claim rather than demonstrated adoption.
- **Slice-only preprocessing:** a study's `prepare` and `keep` hooks derive and
  filter data at press time, and `columns` names what the cartridge retains.
  Pressing still runs on the analyst's machine and network.
- **Local interactivity:** verified historical fixtures include a 15.46-million-
  row, 272 MiB cartridge and a 347 MB stress cartridge that loaded in about 2.5
  seconds. The viewer runs on the main thread, so approaching the format's 1 GiB
  buffer ceiling is expected to freeze the tab.
- **Portable views:** an `.rvr` contains the selected data and saved interactive
  view. Format validation is implemented independently in Python and TypeScript
  against a shared contract corpus, with a separate headless verification suite
  for rendered yields.
- **Small authoring surface:** `rowver new` creates a study whose `cart.py`
  identifies source samples, preparation and selection logic, and retained
  columns.
- **Combine extraction (moved):** the flattened extractor emits per-bin,
  per-process yields from either external bin definitions or a saved region map.
  It lives in `rowver_verification` since 2026-09-10; exercising its output
  against a real external-framework target remains open there.

## Product-priority scoreboard

1. **Easy, polished, feature-rich plotting — done:** the viewer includes plot
   creation, per-plot sources, stacking, error modes, exports, 2D plots, cutflows,
   and Sankey views.
2. **Fast exploration of small slices — done at proven scale:** the historical
   fixtures above establish the current baseline; work near the 1 GiB ceiling is
   not yet supported well.
3. **Hide remote authentication and preprocessing complexity — partial:** data
   access is hidden after credential setup, but setup remains manual.
4. **Adapt to different groups — partial:** the primitives are intended to be
   general but have mainly been exercised by one group.
5. **Minimize duplicated skims, storage, and computation — partial:** cartridges
   replace private skims for interactive slices but do not alter upstream ntuple
   production.
6. **Portable and reproducible views — done:** the v6 cartridge and offline
   viewer carry data and view state together.
7. **Create a cartridge from one small document — done:** the normal authoring
   surface is one `cart.py` study recipe.

## Open directions

The maintained backlog for these directions lives in GitLab issues rather than
this status document:

- [Move viewer selection and binning off the main thread](https://gitlab.cern.ch/dgrove/rowver_web/-/work_items/1)
- [Exercise flattened extraction against an external framework and decide the v6 lock](https://gitlab.cern.ch/dgrove/rowver_verification/-/work_items/1)
- [Decide whether credentialed hosted cartridge pressing is feasible](https://gitlab.cern.ch/dgrove/rowver/-/work_items/2)
