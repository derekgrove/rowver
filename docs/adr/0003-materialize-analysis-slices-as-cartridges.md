# Materialize analysis slices as cartridges

RowVer reads source ntuples directly and materializes only the selected events
and named columns needed for interactive work. This avoids making private copies
of broad upstream skims and keeps browser working sets manageable, at the cost of
requiring a new cartridge when a study needs data it did not retain.
