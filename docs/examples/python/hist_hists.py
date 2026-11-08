"""Histograms of a .rvr file's columns with hist (scikit-hep), drawn with mplhep: each sample
filled into one histogram with a category axis, yields, data against stacked simulation, a list column, a 2D
histogram, and the histograms written to a ROOT file with uproot.

    pip install hist mplhep uproot
    python hist_hists.py [zmumu.rvr]

Writes hist_stack.png, hist_mass_pt.png and hists.root here."""

# %% Read what the histograms need
import sys

import awkward as ak
import hist
import matplotlib.pyplot as plt
import mplhep
import numpy as np
import uproot

import rowver

# The file named as the argument, or zmumu.rvr here (a notebook kernel's own arguments begin with -).
path = sys.argv[1] if sys.argv[1:] and not sys.argv[1].startswith("-") else "zmumu.rvr"
f = rowver.open(path)
label = dict(zip(f.samples, f.labels))
color = {"DY": "#2a78d6", "TT": "#eb6834"}

# Only part of the data was read: scale the simulation to the same share of its luminosity.
data = f.samples.index("data")
read, total = f.sample_columns["events_read"], f.sample_columns["events_total"]
share = read[data] / total[data]
weight = f["weight"] * np.where(f["sample"] == data, 1.0, share)
mass = f["mass"]

# %% Every sample in one histogram, with a category axis
# (an axis cannot be named "sample": hist's fill takes sample= for something else.)
h = (hist.Hist.new
     .StrCat(f.samples, name="process")
     .Reg(30, 60, 120, name="mass", label="dimuon mass [GeV]")
     .Weight())  # keeps the sum of the squared weights too, for the errors
for name in f.samples:  # each sample's rows, filled under its name
    rows = f.rows_of(name)
    h.fill(process=name, mass=mass[rows], weight=weight[rows])

# Events between 80 and 100 GeV, each sample: sum over that range of the mass axis.
window = h[:, hist.loc(80):hist.loc(100):sum]
for name in f.samples:
    print(f"{name:5} {window[name].value:8.1f} ± {np.sqrt(window[name].variance):.1f} events between 80 and 100 GeV")

# %% Data against the stacked simulation, and their ratio
fig, (top, bottom) = plt.subplots(2, 1, sharex=True, figsize=(6, 5.5), height_ratios=[3, 1])
mplhep.histplot([h["TT", :], h["DY", :]], ax=top, stack=True, histtype="fill",
                label=[label["TT"], label["DY"]], color=[color["TT"], color["DY"]],
                edgecolor="white", linewidth=1, flow="none")
mplhep.histplot(h["data", :], ax=top, histtype="errorbar", color="black", markersize=4, label=label["data"],
                flow="none")  # flow="none": no marks for the entries outside the axis
top.set_yscale("log")
top.set_ylim(0.3, 3e4)  # room for the legend
top.set_ylabel("events / 2 GeV")
top.set_xlabel("")
top.legend(frameon=False, loc="upper right")

simulated = h["DY", :] + h["TT", :]
seen = h["data", :]
expected = simulated.values()
ratio = np.divide(seen.values(), expected, out=np.full_like(expected, np.nan), where=expected > 0)
error = np.divide(np.sqrt(seen.variances()), expected, out=np.full_like(expected, np.nan), where=expected > 0)
bottom.errorbar(h.axes["mass"].centers, ratio, yerr=error, fmt="o", color="black", markersize=4)
bottom.axhline(1, color="0.5", linewidth=1)
bottom.set_ylim(0, 2)
bottom.set_ylabel("data / sim.")
bottom.set_xlabel(h.axes["mass"].label)
fig.tight_layout()
fig.savefig("hist_stack.png", dpi=120)

# %% A list column, one entry per muon, each taking its event's weight
muon_pt = f["Muon_pt"]
muons = (hist.Hist.new
         .StrCat(f.samples, name="process")
         .Reg(50, 0, 150, name="pt", label="muon $p_T$ [GeV]")
         .Weight())
for name in f.samples:
    rows = f.rows_of(name)
    per_muon = ak.flatten(ak.broadcast_arrays(weight[rows], muon_pt[rows])[0])
    muons.fill(process=name, pt=ak.flatten(muon_pt[rows]), weight=per_muon)
print(muons["DY", :].sum())

# %% A 2D histogram, and its projections
dy = f.rows_of("DY")
h2 = (hist.Hist.new
      .Reg(30, 60, 120, name="mass", label="dimuon mass [GeV]")
      .Reg(25, 0, 100, name="pt", label="dimuon $p_T$ [GeV]")
      .Weight())
h2.fill(mass=mass[dy], pt=f["pt"][dy], weight=weight[dy])
fig, ax = plt.subplots(figsize=(6, 4.5))
drawn = mplhep.hist2dplot(h2, ax=ax, cmap="Blues", cbarextend=False, flow="none")
drawn.cbar.set_label("events")
ax.set_title(label["DY"])
fig.tight_layout()
fig.savefig("hist_mass_pt.png", dpi=120)
assert np.allclose(h2.project("mass").values(), h["DY", :].values())

# %% Into a ROOT file: uproot writes each histogram as a TH1D or TH2D
with uproot.recreate("hists.root") as out:
    for name in f.samples:
        out[f"mass_{name}"] = h[name, :]
        out[f"muon_pt_{name}"] = muons[name, :]
    out["mass_pt_DY"] = h2
print(uproot.open("hists.root").keys())
