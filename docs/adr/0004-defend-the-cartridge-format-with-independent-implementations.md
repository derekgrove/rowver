# Defend the cartridge format with independent implementations

The `.rvr` format is treated as a cross-implementation contract: the Python
presser and browser viewer validate it independently against shared fixtures,
and a separate verification suite recomputes displayed results without using
viewer code. The duplication is deliberate because agreement between independent
implementations catches compatibility and silent-correctness failures that a
single shared validator would reproduce.
