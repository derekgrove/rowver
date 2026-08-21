# RowVer

A slim data analysis tool for exploring high-energy physics data. RowVer allows you to roam the rows of phase space, enabling you to rapidly iterate on cuts and selections of your samples and see how they affect histograms and yields in real time. At the end of your tuning, you may export the final plots and cuts and move back to your original framework. The only overhead is one initial conversion: `.root` files in, one slim and interactive `.rvr` file out.

`.rvr` files are also built for collaboration — send the file (typically tens of MB) to a collaborator and they can pick up your selection and keep tinkering, no reprocessing required.

**Your .rvr files are never uploaded or processed remotely** -- everything runs in your browser, on your own hardware. A prebuilt copy of the site also ships in the project directory, so you can work offline with no network connection at all.

[Check out some demos here.](https://rowver.web.cern.ch/)

If you would like to get started with generating your own `.rvr` file, follow the below instructions.
___
# Install

*Python 3.10 or newer is required*

### if using pip:

```
pip install uproot awkward numpy plotly vector
```

### if using conda/mamba:

its recommended you setup a new virtual environment:

```
conda create -n rowver -c conda-forge python=3.10 uproot awkward numpy plotly vector
```
and then activate the env:
```
conda activate rowver
```

### And that's it for the dependencies, all that remains is setting up a study and telling the framework where your `.root` files are.

____
#### futher notes on dependencies:

That is the whole list, and it is what this project is developed and tested against:

| package  | version here | | what needs it |
|----------|--------------|-----------|---------------|
| `uproot` | 5.7.5        | mandatory | reading your `.root` files |
| `awkward`| 2.11 / 2.12  | mandatory | the jagged arrays a config operates on |
| `numpy`  | 2.4.2        | mandatory | every column, and the packing |
| `plotly` | 6.7.0        | mandatory | the page itself |
| `vector` | 1.8.0        | optional  | 4-vectors, if your `cart.py` wants them |
| `fsspec-xrootd` | 0.5.2 | optional | only for `root://` paths — see `studies/remote_example/` |
| `aiohttp`, `requests` | | optional | only for `https://` paths |
| `xrootd` | 5.9.2 | optional | the `xrdcp` command, for copying files down |
| a CA bundle | | optional | only for grid files — see `studies/grid_example/` |

Only `numpy` and `plotly` are needed to build a page from an existing `.rvr`; the rest are
for producing one. Four studies ship as worked examples: `template` (local paths, the one
to copy), `dimuon_demo` (local, the shipped demo), `remote_example` (`root://` open data)
and `grid_example` (CMS DAS through a redirector, needs a proxy). `vector` is used by a study's `cart.py` rather than by RowVer itself —
`studies/dimuon_demo/` builds `Momentum4D` objects with it to get the dimuon invariant
mass — a config that computes its own kinematics, as `studies/template/` does, does not
need it.

The demo skim was checked to give byte-identical columns under `awkward 2.11.0`/`vector
1.8.0` and `awkward 2.12.0`/`vector 1.8.1`.

Developed with `uproot 5.7.5` to take advantage of virtual arrays (virtual arrays only load into memory the branches that are needed from your `.root` file). An older uproot version without `arrays(virtual=...)` still works however it will be slower to parse your `.root` files. On wide files such as NanoAOD this is slower and heavier on memory, ~500 seconds to run as opposed to ~100 seconds. **A warning is printed if you are running without virtual arrays.** If so, simply update your version of uproot to get this speed up.

Install the dependencies `lz4` or `zstandard` only if uproot
complains about your files' compression; most `.root` files need neither.

___
# Configure

Now git clone the project:

```
git clone https://gitlab.cern.ch/dgrove/rowver.git
```

and `cd` into it:

```
cd RowVer
```

## how it works:
`.rvr` files are generated from "studies", which is really just two files:

- `cart.py` (the whole study: which `.root` files, which trees, per-sample metadata, and
  the preselection and columns that turn them into a page)
- `view.py` (**optional** initial file setup: every tab, its plots, cuts, boards and sample groupings)

A **cart** is one object holding all of that. The fileset is an ordinary python dict in
coffea's shape — there is no `fileset.json` riding alongside — and the `.rvr` it burns
carries the whole thing with it, so a built file can always tell you exactly what went
into it.

**It is highly recommended you generate your first `.rvr` file without a `view.py`**, this simplifies everything and you can configure the cuts/plots in the browser page upon first loading anyways, you lose nothing by skipping it. This is mainly useful for if you wanted to duplicate your page setup exactly but after rerunning the data skim.

### Now produce your own study (configuration for your `.rvr` file:)

```
cp -r studies/template studies/mine
# edit studies/mine/cart.py   -> your files and trees, then KEEP and COLUMNS
rm studies/mine/view.py       # RECOMMENDED
python run_rowver.py mine
```

This writes `studies/mine/mine.rvr` — the data alone. Drop it on `viewer.html` (or add
`--html` for a standalone `mine.html` with the viewer baked around the data). Open it,
build your groups in the Sample grouping band, set cuts, make
plots and tables, then `save configuration (.rvr)`. The saved file carries the entire
setup.

To make that setup the default for future builds, extract it:

```
python page.py extract mine_2026-08-14_10-30-00.rvr > studies/mine/view.py
python run_rowver.py mine --no-skim
```

`extract` reconstructs the whole file — every tab, with its cuts, plots, boards, grouping
and colours. Hand-tune in the browser, export, extract, rebuild: the loop needs no code.

## Run

```
python run_rowver.py template            skim, then write template.rvr
python run_rowver.py template --html     also build the standalone template.html
python run_rowver.py template --check    verify chunked == unchunked first
python run_rowver.py template --no-skim  reuse the pikl, rewrite the .rvr
python run_rowver.py --list             what studies exist
```

The `.rvr` is the artifact — drop it on `viewer.html` and the page you already have reads
it. `--html` bakes a private copy of that same viewer around the data, which doubles the
size on disk, so it is asked for rather than assumed.

`--samples "a,b"` puts a subset in the page; `--out NAME` renames the outputs.

The same pieces from python, if you need them individually:

```python
import study, samples, pikl, load, page
st = study.load("template")
pikl.pikl("template", samples.files(st["fileset"]), st["config"],
          metadata=samples.meta(st["fileset"]), dest=st["dir"])
page.export(load.load("template"), "template", dest=st["dir"])
```

## Three artifacts

```python
page.export(columns, "name", dest=d)  # <d>/name.html   ~20 MB, data inside — one file to send
page.data(columns, "name", dest=d)    # <d>/name.rvr    ~11 MB, data alone
page.viewer()                         # viewer.html      ~5 MB, no data — drop a .rvr on it
```

Send the viewer once, then only `.rvr` files. `page.read(path)` opens one in python;
`page.extract(path)` recovers the setup it carries.

## The file carries the view

A `.rvr` holds the columns and how the page opens: the grouping, every tab, and for
each tab which plots in what order and how each is drawn, which samples are shown,
the cuts, the luminosity. It records the analysis only. Which tab was focused, which
folds were open, theme and text size are page posture and deliberately not saved: a
`.rvr` always opens on its first tab with the chrome folded. `studies/<name>/view.py`
is where that starts; the file carries it from there. Both `data()` and `export()`
write the same `tabs` into META, so the single HTML and the `.rvr` cannot drift.

In the page:

- The reset arrow on a plot returns that plot to what the file says, or to a fresh
  plot if you spawned it yourself.
- The reset arrow beside Cuts returns the cuts to the file's. Samples and plots stay.
- `save configuration (.rvr)` writes a new file: the same events, every tab, opening
  on the current one. Send it and the recipient gets your view.

A page cannot write over the file it was opened from (outside Chromium no browser
offers that at all), so the export downloads a new file and you replace the original
yourself.

Programmatic defaults, when you want them in code rather than extracted:

```python
meta, _ = page.pack(d)
st = page.state(meta, cuts={"PT_mu_lead": [20, 500], "nJet": [1, 2, 3]},
                groups={"signal": ["some_signal_sample"],
                        "background": ["some_background_sample"]},
                fom=("signal", "background"), lumi=400, varx="MET", nb=24)
page.data(d, "mine", defaults=st, dest="studies/mine")
```

## What a built file remembers

A `.rvr` is a cartridge: plug it into the viewer and it plays, and it also tells you —
with no other file, on any machine — exactly what was put on it.

```
python cart.py studies/mine/mine.rvr          the cartridge label
python cart.py studies/mine/mine.rvr --json   its whole fileset, as JSON
```

```
cart mixed_demo  (/…/studies/mixed_demo/cart.py)
  Data 2012 DoubleMuParked    1 files  Events   weights=NONE (data)
  MC stand-in x2              1 files  Events   weights=stand_in_weight() — A stand-in …
  3 columns, keep: KEEP() — Both muons measured.
```

Under `META.cart` the file carries its whole fileset — every sample, every complete path
local or remote, tree names, per-sample metadata, the branch that filled each sample's
weights and how many events it contributed — plus how each column was made (the branch it
copied, or the source of the function that computed it), the selection's name and
docstring, when it was built, and the `cart.py` that defined it with its sha256.

It is a record, never a promise. A cart that imports a private module is not reproducible
from its text, and nothing here re-runs anything. But it is enough to answer "which files
is this?" a year later, and enough for a merge to refuse two files whose columns or
selection disagree.

It costs little: the repetitive fileset JSON gzips about 16:1, so a **1,000-file fileset
adds ~15 KB** — about 0.15% of a realistic 10 MB study — and a `cart.py`'s text is ~1 KB
gzipped.

⚠️ It also means **your paths and your cart's source travel with a file you send someone**:
eos paths, usernames, directory layout, and whatever your `cart.py` says.

### Adding a sample without re-skimming

```
python cart.py merge run2.rvr new_signal.rvr
```

`Cart.merge` puts the samples of several `.rvr` files onto one cart. The columns are
column-major, so the buffer is repacked either way — but repacking is a memcpy over
arrays that are already read, while re-skimming is hours. Skim the new sample on its own,
merge, done.

It **refuses** rather than concatenating blind: two files whose columns or whose selection
disagree do not describe the same measurement, and two samples with the same name cannot
share a file. That check is only possible because a cart stamps what it did — and the
merged file records which carts its samples came from.

Weights are per sample, so **a weightless data file merges into a weighted MC one**: the
data samples get a column of ones, and the page still never scales them. Under v4 this
was impossible — one global weights key, no way to say "these rows are counts".

`smoke.py merge:<study>=merge` gates it by splitting a study into one file per sample and
merging it back: the result must equal the one-shot build, column for column.

## The Sample grouping band

Grouping is set up once, in the band across the top, and used everywhere: every tab,
every plot, every table. A group draws as one curve. A group can be dragged inside
another group, where it becomes a bundle: one folded chip that toggles and moves as a
unit, with a chevron that unfolds its samples. Depth stops at two. There is no
separate subgroup concept in the page; a bundle is simply a group living inside
another one, and `SUBGROUPS` in view.py declares exactly that.

Chips drag. A sample chip moves between rows to regroup; dropping it inside a
bundle's dashed box files it into that bundle; a group chip dropped on another group
nests. The same chips drop onto table cells and onto plots. Enabling and disabling
samples happens here and only here; a family with some members off wears a hatched
halfway state so it cannot be mistaken for dead or fully live.

```python
SUBGROUPS = {"QCD": "QCD HT *", "ttbar": "ttbar *"}
GROUPS    = {"signal": ["some_signal_sample"],
             "background": ["QCD", "ttbar", "diboson"]}
FOM       = ("signal", "background")   # optional: which group is S and which is B
```

A string is a glob over sample names, a list is explicit, and a `GROUPS` entry may
name a bundle, a sample, or a glob.

Naming a `FOM` pair adds three cells: S/B, S/sqrt(B+sigma^2), and Z binomial. sigma is
B's own MC statistical error, sqrt(sum w^2). ZBI is Cousins-Linnemann-Tucker: the
background is treated as measured in an off region of size B^2/sigma^2, so B's finite
MC statistics cost significance rather than being assumed away. The incomplete beta
and inverse normal behind it are checked against `scipy.stats` to 2e-9 relative.

- A sample named by nothing draws on its own, so a study with no `view.py` draws
  every sample separately.
- A sample belongs to exactly one group. Overlap raises at build time, naming both;
  every total stays a sum over disjoint parts.
- **Untick a member and the curve says so** — `QCD [2 of 3]`, in the legend and in
  `export cuts`. A curve labelled `QCD` that is two of three HT bins is wrong in a way
  the plot cannot show you.
- There is no signal/background concept in the code. Name a group `signal` if you
  want one; nothing privileges the name.
- `SHOWN` lists what is visible when the page opens, by name or glob. Everything else
  is present but switched off. Leave it out and all samples are on.

## Tabs, plots and tables

The bar above the stage holds tabs: independent views of the same events. Each tab
keeps its own cuts, plots, shown samples and luminosity; the grouping and the data
are shared. `+ new` opens a factory tab, `+ copy` duplicates the current one,
`+ table` opens a table tab. The Tabs fold offers three locks — cuts, plots, samples —
each meaning: the tab you are on becomes the shared copy, and unticking returns every
tab to its own configuration.

Plots are built in a three-page dialog: arrange the sheet, then pick mode, variables
and members with no rendering, then preview and adjust on the style page. A plot's
membership is an explicit list of names — groups, bundles, or single samples — each
drawn as its own curve in 1D; a 2D or 3D map sums its members into one distribution
and says so. Members arrive by dragging chips from the tray onto a plot card, by the
`add:` dropdown, or later by dropping chips onto the live plot. To sum samples into
one curve, group them: grouping is the one way things combine.

A table tab is a board of cells over a background shelf. The shelf states its own
arithmetic — the tiles are added, with a `+` between them and an `=` before the total —
and it refuses to count anything twice: drop a group onto a shelf that already holds one
of its members and the group replaces the member, because a shelf that held both would
sum those events into the background twice.

Each cell names a source and reports its yield with errors, or one to three figures of
merit against the summed shelf, or a miniature of the current plot channel; double-click
expands a cell into a full plot. A board opens on S/B alone; `FOMs per cell` takes it to
two or three, each slot cycling through the figures the others are not showing, and the
slots reorder by dragging them. Cells fill by drag or dropdown and swap by dragging one
onto another, the board grows and shrinks one row or column at a time up to 4x4, and
everything rides the `.rvr`.

- A plot opens with no error bars. Its bar offers sqrt(sum w^2), mean(w)sqrt(N) and two
  Poisson forms, per plot; a saved `.rvr` keeps whichever it was given.
- Anything a plot has to say about itself — events piled into the end bin, a log axis
  refused, bins that fell back to sqrt(sum w^2) — is a round badge beside the plot's x,
  amber when it is a warning, with the words in its tooltip. It is a badge rather than a
  line of text so that a long remark cannot make one plot in a pair taller than the
  other.
- `export all plots` in the rail writes TWO PNGs per plot at the scale set above it:
  the figure, and `<name>_cuts.png`, the same figure captioned with the selection that
  produced it. Which one a given audience wants is not knowable in advance and the
  second one is nearly free. The caption is a band above the frame, not a box on the
  data — the top margin grows to hold it — and it wraps to the figure's width rather
  than being truncated. A single plot's camera button follows the `stamp the cuts on
  exported plots` checkbox instead.
  plotly.js has no PDF writer; the browser's print dialog produces a vector PDF
  because plotly renders SVG.
- 3D is the 2D plot on its side: same bins, same colour scale, drawn as a surface.
  Every 2D control applies, including the comparison views.
- Colours wrap at eight. Loose samples take the palette in order, then groups.

Column and sample names are checked against the data, so a typo raises at build time
rather than being ignored in the browser.

## Checking a build

```
python -c "import page; page.viewer()"            build viewer.html — drop/golden need it
python smoke.py studies/x/x.html=boot             boot it in JavaScriptCore
python smoke.py viewer.html+studies/x/x.rvr=drop  what dropping that file does
python smoke.py rowver:x=rt                       write, read back, compare
python smoke.py golden=golden                     the frozen files still open, unchanged
python smoke.py merge:mixed_demo=merge            split a study apart and merge it back
```

`golden/` holds .rvr files frozen at the version that wrote them (open data, built by
`golden/make_goldens_0_13_0.py`, never re-run). The `golden` check opens each with
today's viewer and compares exact integers against pins in `smoke.py`; the day a page
reads different numbers out of one, the battery goes red. Ten are frozen, and each
guards something different: `_tabs` every optional key, `_table` a board with no `met`
key (absence must keep meaning all three FOMs), `_met` a `met` list deliberately out of
canonical order, `_data` a file with no weights at all, and `_mixed` real data and
weighted MC on ONE cart — counted through the page's own cutflow at two luminosities,
because the property it pins is that one of them does not move. A new format capability
should add a new golden beside the old ones, never replace one.

A page holds roughly 150k-800k events; over 40 MB `export` refuses with the size
rather than truncating. Build several pages from one pikl instead.

## Layout

```
studies/<name>/cart.py        the whole study: the fileset, KEEP, COLUMNS, PREPARE
studies/<name>/view.py        how the page opens: VIEW, GROUPS, FOM, COLOUR    (optional)
studies/<name>/<name>.pkl     the skim, native dtypes, every sample   (generated)
studies/<name>/<name>.rvr     columns + the opening view              (generated)
studies/<name>/<name>.html    the page, data inside                   (generated)
viewer.html                   no data — drop a .rvr on it             (generated)
```

The pikl comes first and the .rvr is packed from it. Keeping both is deliberate: the
pikl holds every sample in its native dtypes, the .rvr holds the cut you ship, in the
only two types the page has (float64, int32). `--samples` is what makes them differ.

A study is self-contained. Two studies that share physics should re-use it rather than
copy it — a cart is an object, so `CART.keep` and `CART.columns` can be handed straight
to another `Cart(...)`. `studies/ku_2l_run2` does exactly that.

Note that a study may need packages the tool does not. A cart is ordinary python
and may import whatever your selection needs. `studies/template/` needs nothing beyond
uproot and awkward.

## Adding a study

Copy `studies/template/`, rename it, and edit the one file in it. Everything in it is
a placeholder marked mandatory or optional; point `FILESET` at your own files and edit
the branch names to match.

```
cp -r studies/template studies/mine
# edit studies/mine/cart.py to point at your ROOT files
python run_rowver.py mine
```

A study is `studies/<name>/` with a `cart.py`, plus an optional `view.py`.

Every fileset shipped here holds machine-local paths, so a study fails on a fresh
clone until you point it at your own ROOT files; the error says which file to edit.
`template` ships with placeholder paths for the same reason — it is a starting point to
copy, not a study to run.

### FILESET

Coffea's shape, as an ordinary python dict. A path may be a glob; each path names its own
tree, so one file holding many trees can supply several samples. A path may also be a
remote URL — `root://` through an XRootD redirector, or `https://` — which is read in
place and never globbed.

```python
FILESET = {
    "ttbar 2024": {
        "files": {"/data/TTto2L2Nu_2024/*.root": "Events"},
        "metadata": {"isRun3": True, "weights": "genWeight"},
    },
}
```

`metadata` is yours to fill and reaches every hook (see below). `weights` is the one key
the cart reads itself.

`python cart.py mine --json` prints the fileset as JSON whenever you want to read it that
way, and `python cart.py studies/mine/mine.rvr` prints what a **built** file holds — every
sample, every complete path, which branch filled its weights, how many events it
contributed, and the config it was built from.

### the selection

A config declares three things. The difference between them is not stylistic — it is what
the skim does with each one:

```python
chunk = PREPARE(chunk)              # returns a CHUNK   — it replaces the chunk
kept  = chunk[KEEP(chunk)]          # returns a MASK    — it indexes the chunk
value = COLUMNS[name](kept)         # returns NUMBERS   — one per surviving event
```

**`PREPARE(a)` is the only hook that can change the events.** It runs once per chunk,
before anything else reads it, and whatever it returns *becomes* the chunk. Use it for a
transformation everything downstream then depends on: attaching a per-sample weight,
building a derived object both `KEEP` and `COLUMNS` want, or putting a collection into the
order your columns assume. It is optional — leave it out and the chunk is used as read.

**`KEEP(a)` only selects.** It returns a boolean mask, and the skim applies that mask to
the chunk it already has. Work done inside `KEEP` is invisible to everything else: sort a
collection in there and you sort a local copy, derive your mask from it, and hand back
only the mask — `COLUMNS` then runs on the *unsorted* events. Nothing raises. It is worth
knowing about in advance, because the result looks like it worked.

**`COLUMNS` derives one number per event.** That is the file format, not a convention:
each column is written with `.tobytes()` and read back by the page as
`new Float64Array(buffer, offset, n)`, so every column has to be exactly `n` long. A
branch that is already one-per-event — `nJet`, `MET_pt` — can be named directly. A jagged
branch cannot: `Muon_pt` is `float[nMuon]`, one value per *muon*, so it needs a reduction
to one number per event first. That reduction is what the lambda is for.

**Every hook may take a second argument**, that sample's `metadata` — `def KEEP(a, meta):`,
`def PREPARE(a, meta):`, and any `COLUMNS` entry that is a function. Write the
one-argument form and it still works. That is how a per-sample cross-section or scale
factor reaches the events.

The metadata handed over is the sample's own, plus three keys the skim knows and the
fileset cannot: `sample`, `file` — the complete path of the file this chunk came from —
and `tree`. A column can therefore depend on which file it is reading, which is what
`studies/ku_2l_run2` needs: each era has its own luminosity, so its weight is

```python
"weights": lambda a, m: a.weight * m["eras"][era_of(m["file"])] / 137.93
```

Those three names are reserved; a fileset that already uses one fails at load rather than
having its value quietly overwritten.

#### Why the split matters, worked through

`studies/dimuon_demo/` reads CMS open data whose muon collection is **not** pT-ordered. Of the
501,037 events holding two muons, 235,668 store a harder muon at index 1 than at index 0,
and only 40% are fully descending. So `[:, 0]` does not mean "the leading muon" until
something makes it true, and the config sorts in `PREPARE`:

```python
def PREPARE(a):
    order = ak.argsort(a.Muon_pt, axis=1, ascending=False)
    for f in ("Muon_pt", "Muon_eta", "Muon_phi", "Muon_mass", "Muon_charge"):
        a = ak.with_field(a, a[f][order], f)
    return a
```

That one call changes what every later line means. `PT_mu_lead` takes a different value in
51% of events. `q_pair` changes in 10% of them — with more than two muons the two
*hardest* are not the first two *stored*, so the sort decides which pair the column
describes at all. And `KEEP`'s own bound, `Muon_pt[:, 0] < 600`, only rules out
mismeasured muons *anywhere in the event* once index 0 is the hardest one; unsorted it
constrains the first two slots, and a 157 TeV muon sitting at index 2 sails through.

The same sort written inside `KEEP` would be discarded. Written inside each lambda it
would run five times, and `KEEP` would still need its own copy. A whole-chunk
transformation belongs in `PREPARE`: once, before anything reads it.

The one case where you need no `PREPARE` at all is a config whose selection fixes the
multiplicity and whose columns are order-independent — a pair mass, a charge product. The
moment you want a column called *leading*, something has to sort.

A value in `COLUMNS` is either a branch name or a lambda over the event chunk.

```python
import awkward as ak

def KEEP(a):
    return a.nMuon >= 2

COLUMNS = {
    "MET": "PuppiMET_pt",
    "nJet": "nJet",
    "PT_mu_lead": lambda a: ak.max(a.Muon_pt, axis=1),
}
```

**Weights are per sample, which is how data and MC share one file.** Each sample's
`metadata["weights"]` says what fills its weights column, in exactly the grammar `COLUMNS`
uses — a branch name, or a function — so every sample may name a different branch:

```python
FILESET = {
    "ttbar 2018":      {"files": {...}, "metadata": {"weights": "genWeight"}},
    "DY 2018":         {"files": {...}, "metadata": {"weights": "LHEWeight_originalXWGTUP"}},
    "SingleMuon 2018": {"files": {...}, "metadata": {}},
}
```

**A sample that declares none is real data**: its rows are 1.0 and **the page never scales
them by a luminosity**, whatever its group is set to. So its number is its event count,
always, and there is no per-group luminosity to remember to set to 1.

If no sample on the cart declares weights, no weights column is written at all — the page
then hides the weighted/raw toggle and the luminosity box, counts every event once, and
drops the MC-statistical term from `S/sqrt(B+sigma^2)`, which on unweighted events would
just be B's own Poisson error counted twice. `studies/dimuon_demo/` is the worked example:
it is CMS open data, so it carries none. `studies/mixed_demo/` is the other one: the same
file read twice, once as data and once weighted, so only the weighting differs.

⚠️ **Mixing the two INSIDE one group makes a total that is part yield and part count.**
That is legitimate — a data-driven background estimate looks like this — but it is worth
knowing, so the page puts a `!` in the corner of every plot, table and region map that
draws such a group, naming the samples with no weights. Unweighted samples on their own,
grouped with each other, or drawn beside weighted ones in their own group say nothing.
`run_rowver.py` prints the same warning at build time.

A lambda that can return nothing for some events is rejected at skim time, naming the
column; give `KEEP` a condition that guarantees a value.

`LABELS` says what an integer column's codes mean, so a chip reads `Medium` rather
than `5`. Every value present in the data needs an entry or the build refuses.

```python
LABELS = {"jetcat": {0: "S jet", 1: "ISR jet"}}
```

## Running on the grid

`studies/remote_example/` reads public open data over `root://` and needs no certificate.
`studies/grid_example/` goes through a redirector to CMS NanoAOD, which does — and from a
laptop that means setting **both** of these before the run, in the shell you run it from:

```
export X509_USER_PROXY=/tmp/x509up_u$(id -u)
export X509_CERT_DIR=/path/to/certificates
```

For getting a proxy onto your own machine in the first place — the certificate, the CA
bundle, `voms-proxy-init` without a grid login — this walkthrough is the one that works:
<https://hackmd.io/@andrzejnovak/SJxscCZvD>

**`voms-proxy-init` ends in red even when it succeeds.** After `Your proxy is valid
until …` it usually prints `Error: Certificate verification failed.` /
`outdated CRL found` / `CRL has expired`. That is its own post-creation check tripping
over the stale certificate revocation lists inside the conda CA bundle, which no laptop
refreshes; the proxy itself is fine. The line that matters is `Your proxy is valid
until …`. `remote/README.md` shows the full output and how to confirm.

**Unset, the failure is a hang rather than an error.** XRootD prints

```
security protocol 'ztn' disallowed for non-TLS connections.
```

and then retries forever. That line is not the fault: it says the server offered token
auth, which the client refuses without TLS, so it falls back to your proxy — and with no
`X509_CERT_DIR` it cannot validate anything and has nothing left to try. Measured here:
3.9 s to open the file with both set, against never with them unset. Ctrl-C needs several
presses because the retry loop is inside XRootD's C++ layer. Meanwhile `xrdfs stat` on the
same path keeps working, which makes it look like a bug in this project — it finds the CA
bundle by itself, and uproot has to be told.

Grid speed is not predictable: the same two files skimmed in 29 s on one run here and 15
minutes on another the same afternoon, depending on which replica the redirector picks.
If you are iterating on `KEEP`, copy the files down once with `xrdcp` and point
`FILESET` at the local copies.

## Things that will bite

- Integer columns must fit in `int32`. An `eventnum` will not; store it as a float or
  leave it out. The packer refuses rather than wrapping silently.
- Every sample opens ungrouped unless `view.py` says otherwise. Group them in the
  Sample grouping band.
- An unweighted sample is never scaled by luminosity. That is the point, but it means a
  group holding one beside a weighted sample has a total that is part yield and part raw
  count — the page marks exactly that case with a `!`.
- The `.rvr` format is versioned (`ROWVER` in `page.py`, `const ROWVER` in
  `page_template.html`). They must change together, and `READS` on both sides says which
  versions still open. A file saved from the page is written at the viewer's own version
  and keeps whatever cart stamp it arrived with.

## Licence

MIT — see `LICENSE`.
