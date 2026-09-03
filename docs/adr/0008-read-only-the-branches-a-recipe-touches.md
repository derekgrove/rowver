# Read only the branches a recipe touches

The Presser reads each sample in two kinds of chunk. The first 1024 entries of
its first file are read as lazy placeholders and the recipe's hooks run on them;
the branches whose placeholders materialised are the branches the recipe touches.
Every later chunk, in that file and the sample's remaining files, reads exactly
those branches in one batched request, with the remaining branches still
available as lazy placeholders so a hook that reaches for a new branch part-way
through keeps working and merely enlarges the touched set for the chunks that
follow. A file lacking a touched branch keeps that name as a placeholder.

A byte-budget `--step-size` (default `100 MB`) now bounds the uncompressed bytes
of the touched branches, sized from the per-branch totals ROOT keeps in the tree
header and re-derived whenever the touched set grows. Backends without branch
totals (RNTuple) are sized from the file's own bytes per entry, which over-counts
and so only makes chunks smaller. The earlier default asked uproot for `num_entries_for`, which reads every
basket header of every branch: on KUAnalysis ntuples with 83-entry baskets that
cost 44 seconds per file before any event was read, and made a 19-million-event
local study take about ninety minutes where the previous fixed 250k-entry chunks
took ten. Sizing from the header costs no I/O, and reading touched branches in
one request instead of one lazy request per branch per chunk cuts the read itself
by roughly a further factor of two locally and removes a per-branch round trip
per chunk for remote sources.

Cartridges are compressed at gzip level 6 instead of 9: on a 360 MB buffer level 9
measured 2.5x slower for a 0.3% smaller file. The reader accepts any level.

Supersedes nothing; ADR 0007's opt-in source parallelism composes with this
change, since each worker runs the same sequential skim.
