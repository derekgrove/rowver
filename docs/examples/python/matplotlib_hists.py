"""Histograms of a .rvr file's columns with matplotlib: data against stacked simulation with a
ratio, the shape of each sample, a list column, and a 2D histogram.

    python matplotlib_hists.py [zmumu.rvr]

Writes mpl_stack.png, mpl_shapes.png and mpl_mass_pt.png here."""

# %% Read what the plots need
import sys

import awkward as ak
import matplotlib.pyplot as plt
import numpy as np

import rowver

# The file named as the argument, or zmumu.rvr here (a notebook kernel's own arguments begin with -).
path = sys.argv[1] if sys.argv[1:] and not sys.argv[1].startswith("-") else "zmumu.rvr"
f = rowver.open(path)
mass, pt, weight = f["mass"], f["pt"], f["weight"]
label = dict(zip(f.samples, f.labels))
color = {"data": "black", "DY": "#2a78d6", "TT": "#eb6834"}

# Only part of the data was read: scale the simulation to the same share of its luminosity.
read, total = f.sample_columns["events_read"], f.sample_columns["events_total"]
share = read[f.samples.index("data")] / total[f.samples.index("data")]

# %% Data against the stacked simulation, and their ratio
bins = np.linspace(60, 120, 31)
centers = (bins[1:] + bins[:-1]) / 2
data = f.rows_of("data")
seen, _ = np.histogram(mass[data], bins)
simulated = ["TT", "DY"]  # the smallest at the bottom of the stack
rows = [f.rows_of(name) for name in simulated]

fig, (top, bottom) = plt.subplots(2, 1, sharex=True, figsize=(6, 5.5), height_ratios=[3, 1])
stacked, _, _ = top.hist([mass[r] for r in rows], bins, weights=[weight[r] * share for r in rows],
                         stacked=True, histtype="stepfilled", edgecolor="white", linewidth=1,
                         color=[color[n] for n in simulated], label=[label[n] for n in simulated])
top.errorbar(centers, seen, yerr=np.sqrt(seen), fmt="o", color="black", markersize=4, label=label["data"])
top.set_yscale("log")  # tt̄ is a few events of the thousands
top.set_ylim(0.3, 3e4)  # room for the legend
top.set_ylabel("events / 2 GeV")
top.legend(frameon=False, loc="upper right")
expected = np.atleast_2d(stacked)[-1]  # the top of the stack: every simulated sample (one alone too)
ratio = np.divide(seen, expected, out=np.full_like(expected, np.nan), where=expected > 0)
bottom.errorbar(centers, ratio, yerr=np.sqrt(seen) / np.where(expected > 0, expected, np.nan),
                fmt="o", color="black", markersize=4)
bottom.axhline(1, color="0.5", linewidth=1)
bottom.set_ylim(0, 2)
bottom.set_ylabel("data / sim.")
bottom.set_xlabel("dimuon mass [GeV]")
fig.tight_layout()
fig.savefig("mpl_stack.png", dpi=120)

# %% The shape of each sample, and a list column
fig, (left, right) = plt.subplots(1, 2, figsize=(10, 4))
for name in f.samples:
    left.hist(mass[f.rows_of(name)], bins, density=True, histtype="step", linewidth=2,
              color=color[name], label=label[name])
left.set_xlabel("dimuon mass [GeV]")
left.set_ylabel("fraction of events / GeV")
left.set_ylim(0, 0.2)  # room for the legend
left.legend(frameon=False, loc="upper left")

# Muon_pt holds a list per event: flattened, one value per muon. Each muon takes its event's weight.
muon_pt = f["Muon_pt"]
dy = f.rows_of("DY")
per_muon = ak.flatten(ak.broadcast_arrays(weight[dy] * share, muon_pt[dy])[0])
pt_bins = np.linspace(0, 150, 51)
right.hist(ak.flatten(muon_pt[dy]), pt_bins, weights=per_muon, histtype="step", linewidth=2,
           color=color["DY"], label="every muon")
right.hist(f["leading_muon_pt"][dy], pt_bins, weights=weight[dy] * share, histtype="step", linewidth=2,
           color=color["DY"], linestyle="--", label="the leading muon")
right.set_yscale("log")
right.set_xlabel("muon $p_T$ [GeV]")
right.set_ylabel("Drell-Yan muons / 3 GeV")
right.legend(frameon=False)
fig.tight_layout()
fig.savefig("mpl_shapes.png", dpi=120)

# %% A 2D histogram: mass against the pair's pt, for Drell-Yan
fig, ax = plt.subplots(figsize=(6, 4.5))
*_, image = ax.hist2d(mass[dy], pt[dy], bins=[bins, np.linspace(0, 100, 26)], weights=weight[dy] * share,
                      cmap="Blues", cmin=1e-9)
fig.colorbar(image, label="events")
ax.set_xlabel("dimuon mass [GeV]")
ax.set_ylabel("dimuon $p_T$ [GeV]")
ax.set_title(label["DY"])
fig.tight_layout()
fig.savefig("mpl_mass_pt.png", dpi=120)
