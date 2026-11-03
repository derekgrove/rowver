# What is a .rvr file?

Maybe it's best to first say what problems a `.rvr` file solves:

1) Speed
2) Compact data (able to be sent and shared with colleagues easily)
3) Local (on your laptop, with *your* processor) data analysis

### Speed

Much of the time a HEP analysis takes goes into opening and reading `.root` files. The usual answer is to split the files into chunks, send the chunks to batch computing (condor), and process each one there. With a lot of data that looks like the obvious, fastest way to work. But every chunk is opened, read, turned into an output, and closed again, every time the analysis runs. Splitting the files further makes that overhead worse, not better.

`.rvr` takes the opposite approach (not a new idea, but rarely the one taken): instead of keeping the files apart and splitting them further, *merge them*. Read the ROOT files once, keep only the events and branches you need, and put them in a single file. From then on you open one file, once.

```text
  DY_1.root
    MET_pt  [ 12.4  21.2   5.9   8.4 ]
    nJet    [    0     3     1     3 ]
    nMuon   [    1     2     0     2 ]
    … 1,500 more
  ──────────────────────────────────────
  DY_2.root
    MET_pt  [ 33.1  17.8   9.7 ]
    nJet    [    2     0     1 ]
    nMuon   [    2     1     2 ]
    … 1,500 more
  ──────────────────────────────────────
  TT_1.root
    MET_pt  [ 64.2  51.5  80.3 ]
    nJet    [    4     3     5 ]
    nMuon   [    1     2     1 ]
    … 1,500 more

                    │ rvr press
                    ▼

  zmumu.rvr    DY                          TT
               DY_1          DY_2          TT_1
    sample   [     0     0 │     0     0 │     1 ]
    entry    [     1     3 │     0     2 │     1 ]
    MET_pt   [  21.2   8.4 │  33.1   9.7 │  51.5 ]
    nJet     [     3     3 │     2     1 │     3 ]
    mass     [  88.6  93.6 │  91.0  90.3 │ 142.7 ]

    header: DY = rows 0–3 (DY_1 0–1, DY_2 2–3), TT = row 4
```

The events kept are those with two muons. `nMuon` is read for that cut but not stored, and
`entry` says which event of its file each row is. Each column is compressed on its own, so
reading `mass` reads nothing else.

So the opening and reading of ROOT files is done once, by `rvr press`, and never again. After that, every time you change a binning, add a cut or remake a plot, you open one small file and get the columns you asked for as NumPy arrays: no loop over files, no chunks, no jobs to wait on.

To put a number on it: take a sample of 30 ROOT files (30 copies of one TChiWZ skim: 212,130 events, 4.1 GB) and a recipe that keeps the events with `MET > 150` and two leptons, and stores 7 branches. Reading those 7 branches from the 30 files with uproot and applying the cut takes 2.1 s, and it takes 2.1 s every time you do it. `rvr press` does the same once, in 0.4 s; after that, opening the `.rvr` and reading all 7 columns takes 6 ms, about 350 times faster. (Measured on an Apple M5 laptop on 2026-10-29, with the ROOT files on its own disk. Read from a grid site, the ROOT loop also waits on the network every time; the `.rvr` is already on your laptop.)

### Compact data

Most of what is in a ROOT file is never used by any one analysis: the file holds every branch of every event, and you need a handful of branches for the events that pass your preselection. A `.rvr` file holds only that. One of the files above is 136 MB (its main tree alone is 22.7 MB, 285 branches); pressed with the recipe above, it becomes a 157 kB `.rvr`. That is small enough to send in a message, keep in a git repository, or keep on your laptop for every study you have.

A `.rvr` file also describes itself: it carries the whole text of the `samples.toml`, `recipe.toml` and `samples.lock` that made it. Whoever you send it to can see exactly what is in it and how it was cut, check it with `rvr validate`, and press it again from the same inputs. [Step 9 of the examples](examples/README.md#9-share-it-check-it-sign-it) shows how to sign a file, so they also know it came from you.

### Local analysis

Because the file is small and opens in milliseconds, the analysis itself moves to your laptop. The grid is used once, to press the file (when the files are there), and `rvr press` reads only the branches the recipe uses, from wherever the files are stored. Everything after that (plots, cuts, fits, trying out ideas) runs on your own processor, in Python, with no batch jobs, no queue and no grid proxy. When you change your mind about what to store, change the recipe and press again: `rvr` reads only the new branches, and only for the events it already kept ([step 5 of the examples](examples/README.md#5-add-a-column-you-forgot)).

### The rules

1) **One study, one file.** Every sample of a study lives in one `.rvr` file, each sample in its own block of rows.
2) **The file says how it was made.** The recipe and the samples are inside it. You never edit a `.rvr` file by hand: you change the recipe and press again.
3) **Store what you use.** A `.rvr` holds the columns of your analysis, not a copy of the ntuples. When you need another branch, add it to the recipe.

To make your first one, follow [the examples](examples/README.md).
