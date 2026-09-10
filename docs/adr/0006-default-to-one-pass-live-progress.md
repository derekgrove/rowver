# Default to one-pass live progress

`rowver run` uses a fast live display that observes the real `Cart.skim` pass and
never pre-opens sources for presentation, because minimum time to a completed
cartridge takes priority over exact progress totals. Plain one-pass execution
remains available as `--no-live`. The indexed display chosen by ADR-0005 stayed
available as `--live-robust` until 2026-09-10, when decision D2 in
`docs/plan-bare-bones-presser.md` removed it: a display-only pre-pass that opens
every source twice was not worth exact totals, percentage bars, and ETA.
