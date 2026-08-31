# Default to one-pass live progress

`rowver run` uses a fast live display that observes the real `Cart.skim` pass and
never pre-opens sources for presentation, because minimum time to a completed
cartridge takes priority over exact progress totals. Plain one-pass execution
remains available as `--no-live`; the indexed display chosen by ADR-0005 remains
available as `--live-robust`, explicitly accepting a display-only source pre-pass
in exchange for exact totals, percentage bars, and ETA.
