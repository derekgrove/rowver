# Default to the live press display

`rowver run` uses the live press display by default for both terminal and
redirected output, accepting a deliberate metadata pre-pass that opens every
source before the Presser reopens it to skim events. This extra remote and local
I/O makes blocked source opens and later processing phases observable; users who
prefer single-pass execution can select `--no-live` explicitly.
