# Offer source-parallel pressing as an opt-in

RowVer keeps sequential pressing as the default and offers source-level worker
processes only through `rowver run STUDY --parallel [N|max]`, because remote studies
benefit substantially while local compression-bound studies do not. Workers
read and select sources from a dynamic queue, but the coordinator alone merges
in declaration order, checks the global size bound, reports progress, packs,
validates, compresses, and atomically publishes, preserving the sequential
cartridge contract at the cost of higher peak memory from process-returned
arrays.
Parallel runs add no scheduling pre-pass.
Bare `--parallel` reserves two logical CPUs; `--parallel max` requests every
logical CPU, with either mode capped by the number of runnable sources.
