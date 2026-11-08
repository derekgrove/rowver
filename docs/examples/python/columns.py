"""Working with the columns of a .rvr file: what it holds, its columns as NumPy and Awkward
arrays, one sample's rows, cuts, new quantities, weights, and pandas.

    python columns.py [zmumu.rvr]

The file is the one the guide's study makes (README.md here says how). Each `# %%` is a cell, for
an editor that runs them one at a time (VS Code, Jupyter with jupytext)."""

# %% Open it, and see what it holds
import sys

import awkward as ak
import numpy as np

import rowver

# The file named as the argument, or zmumu.rvr here (a notebook kernel's own arguments begin with -).
path = sys.argv[1] if sys.argv[1:] and not sys.argv[1].startswith("-") else "zmumu.rvr"
f = rowver.open(path)
print(f)  # <rowver.File 'zmumu.rvr': 26649 rows, 4 columns, 2 derived, 3 samples>
print(f.samples)  # ['data', 'DY', 'TT']: names, as every tool calls them
print(f.labels)  # ['Data, Run 2012 B and C', 'Drell-Yan', 'tt̄']: what a plot shows
print(f.columns)  # ['nMuon', 'Muon_pt', 'mass', 'pt']: stored
print(f.derived)  # ['leading_muon_pt', 'weight']: worked out when first read
print(f.keys())  # every name f[...] reads: sample and entry first
print(len(f))  # 26649 rows, one per kept event
print(f.recipe)  # the recipe.toml that made the file

# %% A column is a NumPy array, one value per event
mass = f["mass"]
print(mass.dtype, mass[:3])  # float32 [113.64686   88.297104  88.33483 ]
print(mass.mean(), np.median(mass))

# Cuts are masks, and new quantities are NumPy arithmetic.
z_window = (mass > 80) & (mass < 100)
print(z_window.sum(), "events between 80 and 100 GeV")
pt_over_m = f["pt"][z_window] / mass[z_window]

# The arrays are read-only, shared with what f holds: copy one to change it.
clipped = mass.copy()
clipped[clipped > 150] = 150

# %% A list column is an Awkward array, a list per event
muon_pt = f["Muon_pt"]
print(muon_pt.type)  # 26649 * var * float32
print(ak.num(muon_pt)[:5])  # muons in each event: [2, 2, 2, 2, 2]
print(muon_pt[:, 0][:3])  # the first muon of each (every event kept has two or more): not
# always the highest pT, as these files do not sort their muons
hard = muon_pt[muon_pt > 30]  # the muons above 30 GeV, still a list per event
print(ak.sum(ak.num(hard) >= 2), "events with two muons above 30 GeV")
every_muon = ak.to_numpy(ak.flatten(muon_pt))  # one value per muon, for a histogram
print(len(every_muon), "muons")

# A derived column reads as any other: this one is greatest(Muon_pt, -1), worked out from Muon_pt.
assert np.array_equal(f["leading_muon_pt"], ak.to_numpy(ak.max(muon_pt, axis=1)))

# %% Each event's sample, and one sample's rows
sample = f["sample"]  # each row's index into f.samples
print(np.bincount(sample))  # rows per sample: [10420 14483  1746]
dy = f.rows_of("DY")  # a slice, as a sample's rows come together
print(dy)  # slice(10420, 24903, None)
print(mass[dy].mean(), "GeV: the mean Drell-Yan mass")  # a view of mass, not a copy
for name, label in zip(f.samples, f.labels):
    rows = f.rows_of(name)
    print(f"{name:5} {rows.stop - rows.start:6} events, mean mass {mass[rows].mean():.1f} GeV  ({label})")

# Where each event came from: its entry in its file's tree.
print(f["entry"][:3])

# %% A value per sample, and the weights
values = f.sample_columns  # each sample's metadata and counts, in the order of f.samples
print(values["xsec"])  # [None, 3503.7, 225.2]: the data have none
print(values["events_read"], values["events_total"])

# A per-sample value for every event: index it with the sample of each (None becomes nan).
xsec = np.array(values["xsec"], dtype=float)[sample]

# The recipe's derived weight scales each simulated sample to the data's luminosity: data 1,
# simulation xsec * lumi / the sum of the weights it read.
weight = f["weight"]
print({name: np.unique(weight[f.rows_of(name)]).tolist() for name in f.samples})  # data 1, DY 406.4, TT 26.1
# Only part of the data was read: scale the simulation to the same share of its luminosity.
data = f.samples.index("data")
share = values["events_read"][data] / values["events_total"][data]
for name in ["DY", "TT"]:
    rows = f.rows_of(name)
    expected = weight[rows][z_window[rows]].sum() * share
    print(f"{name}: {expected:.1f} events expected between 80 and 100 GeV")
print(f"data: {z_window[f.rows_of('data')].sum()} events seen")

# %% Several columns at once: Awkward records, pandas, Parquet
events = f.arrays(["mass", "pt", "weight"])  # a record per event, sample (its name) and entry first
z = events[(events.mass > 80) & (events.mass < 100) & (events.sample == "DY")]
print(len(z), z.fields)

frame = f.to_pandas(["mass", "pt", "weight"])  # sample is a pandas Categorical
print(frame.groupby("sample", observed=True)["mass"].describe())
frame.to_csv("zmumu.csv", index=False)
f.to_parquet("zmumu.parquet")  # every column, the list ones as Arrow lists

# %% A large file
# Each column is read when first asked for, and held until f.forget lets it go:
f.forget("mass")
# f.slice reads only part of a column, decoding only the chunks that hold those rows:
rows = f.rows_of("TT")
tt_mass = f.slice("mass", rows.start, rows.stop)
# Closing lets go of the file and what f holds; arrays already read stay as they are.
f.close()
with rowver.open(path) as f:
    pt = f["pt"]
print(len(pt), "values of pt, read before the file closed")
