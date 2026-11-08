"""Histograms of a .rvr file's columns with ROOT, from Python: TH1D and TH2D filled from the
columns, data against a THStack with their ratio, an RDataFrame over the columns, and a ROOT file
with the histograms and a TTree of the columns.

    python root_hists.py [zmumu.rvr]

It needs a Python that has both ROOT and rowver (conda install -c conda-forge root, then
pip install "rowver[all]", which brings awkward). Writes root_stack.png, root_mass_pt.png, root_hists.root and
zmumu_tree.root here."""

# %% Read what the histograms need
import sys

import awkward as ak
import numpy as np
import ROOT

import rowver

ROOT.gROOT.SetBatch(True)
ROOT.gStyle.SetOptStat(0)
# A palette of one hue, light to dark: few events pale, many dark.
ROOT.TColor.CreateGradientColorTable(3, np.array([0.0, 0.5, 1.0]), np.array([0.93, 0.16, 0.04]),
                                     np.array([0.95, 0.47, 0.17]), np.array([0.99, 0.84, 0.38]), 99)
# The file named as the argument, or zmumu.rvr here (a notebook kernel's own arguments begin with -).
path = sys.argv[1] if sys.argv[1:] and not sys.argv[1].startswith("-") else "zmumu.rvr"
f = rowver.open(path)
label = dict(zip(f.samples, f.labels))
# ROOT's text does not draw every character: tt̄ is t#bar{t} in its own markup.
drawn = {name: text.replace("tt̄", "t#bar{t}") for name, text in label.items()}
color = {"DY": ROOT.TColor.GetColor("#2a78d6"), "TT": ROOT.TColor.GetColor("#eb6834")}

# Only part of the data was read: scale the simulation to the same share of its luminosity.
data = f.samples.index("data")
read, total = f.sample_columns["events_read"], f.sample_columns["events_total"]
share = read[data] / total[data]

# FillN takes float64 values and one weight for each: astype makes float64 copies of float32 columns.
mass = f["mass"].astype(np.float64)
pt = f["pt"].astype(np.float64)
weight = f["weight"] * np.where(f["sample"] == data, 1.0, share)

# %% A TH1D for each sample
hists = {}
for name in f.samples:
    rows = f.rows_of(name)
    h = ROOT.TH1D(f"mass_{name}", ";dimuon mass [GeV];events / 2 GeV", 30, 60, 120)
    h.Sumw2()
    n = rows.stop - rows.start
    if n:  # FillN takes no empty arrays
        h.FillN(n, mass[rows], weight[rows])
    hists[name] = h
    print(f"{name:5} {h.Integral(h.FindBin(80.5), h.FindBin(99.5)):8.1f} events between 80 and 100 GeV")

# %% Data against the stacked simulation, and their ratio
stack = ROOT.THStack("stack", ";;events / 2 GeV")
for name in ["TT", "DY"]:  # the smallest at the bottom of the stack
    hists[name].SetFillColor(color[name])
    hists[name].SetLineColor(ROOT.kWhite)
    stack.Add(hists[name])
data_hist = hists["data"]
data_hist.SetMarkerStyle(20)
data_hist.SetLineColor(ROOT.kBlack)
data_hist.SetMarkerSize(0.8)

canvas = ROOT.TCanvas("stack_canvas", "", 720, 660)
top = ROOT.TPad("top", "", 0, 0.3, 1, 1)
bottom = ROOT.TPad("bottom", "", 0, 0, 1, 0.3)
top.SetBottomMargin(0.02)
bottom.SetTopMargin(0.03)
bottom.SetBottomMargin(0.3)
for pad in (top, bottom):
    pad.Draw()
top.cd()
top.SetLogy()
stack.SetMinimum(0.3)  # tt̄ is a few events of the thousands
stack.SetMaximum(3e4)  # room for the legend
stack.Draw("hist")
stack.GetXaxis().SetLabelSize(0)
data_hist.Draw("e1 same")
legend = ROOT.TLegend(0.58, 0.68, 0.88, 0.88)
legend.SetBorderSize(0)
legend.SetFillStyle(0)
legend.AddEntry(data_hist, drawn["data"], "lep")
for name in ["DY", "TT"]:
    legend.AddEntry(hists[name], drawn[name], "f")
legend.Draw()

bottom.cd()
simulated = hists["DY"].Clone("simulated")
simulated.Add(hists["TT"])
for b in range(simulated.GetNbinsX() + 2):
    simulated.SetBinError(b, 0)  # the ratio's errors are the data's alone
ratio = data_hist.Clone("ratio")
ratio.Divide(simulated)
ratio.SetTitle(";dimuon mass [GeV];data / sim.")
ratio.SetMinimum(0)
ratio.SetMaximum(2)
for axis in (ratio.GetXaxis(), ratio.GetYaxis()):
    axis.SetLabelSize(0.1)
    axis.SetTitleSize(0.11)
ratio.GetYaxis().SetTitleOffset(0.4)
ratio.GetYaxis().SetNdivisions(505)
ratio.Draw("e1")
one = ROOT.TLine(60, 1, 120, 1)
one.SetLineColor(ROOT.kGray + 1)
one.Draw()
canvas.SaveAs("root_stack.png")

# %% A TH2D, and a list column flattened into a TH1D
dy = f.rows_of("DY")
h2 = ROOT.TH2D("mass_pt_DY", f"{drawn['DY']};dimuon mass [GeV];dimuon p_{{T}} [GeV]", 30, 60, 120, 25, 0, 100)
if dy.stop > dy.start:  # FillN takes no empty arrays
    h2.FillN(dy.stop - dy.start, mass[dy], pt[dy], weight[dy])
canvas2 = ROOT.TCanvas("mass_pt_canvas", "", 720, 540)
canvas2.SetRightMargin(0.15)
h2.Draw("colz")
canvas2.SaveAs("root_mass_pt.png")

# Muon_pt holds a list per event: flattened, one value per muon, each with its event's weight.
muon_pt = f["Muon_pt"][dy]
every = ak.to_numpy(ak.flatten(muon_pt)).astype(np.float64)
per_muon = ak.to_numpy(ak.flatten(ak.broadcast_arrays(weight[dy], muon_pt)[0]))
muons = ROOT.TH1D("muon_pt_DY", ";muon p_{T} [GeV];muons / 3 GeV", 50, 0, 150)
if len(every):
    muons.FillN(len(every), every, per_muon)

# %% An RDataFrame over the columns
# FromNumpy takes flat columns (one value per event) of any NumPy type, float32 included.
frame = ROOT.RDF.FromNumpy({"mass": f["mass"], "pt": f["pt"], "weight": weight, "sample": f["sample"]})
in_window = frame.Filter("mass > 80 && mass < 100", "Z window")
per_sample = {name: in_window.Filter(f"sample == {i}").Sum("weight") for i, name in enumerate(f.samples)}
pt_dy = in_window.Filter(f"sample == {f.samples.index('DY')}").Histo1D(
    ("pt_DY", ";dimuon p_{T} [GeV];events / 4 GeV", 25, 0, 100), "pt", "weight")
for name, value in per_sample.items():
    print(f"{name:5} {value.GetValue():8.1f} events between 80 and 100 GeV (RDataFrame)")

# A TTree of the flat columns, for macros that read one.
frame.Snapshot("Events", "zmumu_tree.root")

# %% The histograms into a ROOT file
out = ROOT.TFile("root_hists.root", "RECREATE")
for h in [*hists.values(), h2, muons, pt_dy.GetValue()]:
    h.Write()
out.Close()
