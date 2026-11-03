#!/usr/bin/env bash
# Runs the walkthrough of README.md here from scratch, every command whose output the guide quotes
# (sections 2 to 9 and 11; section 10's needs your own files), so that those outputs can be checked
# against what rvr prints today.
#
#   walkthrough.sh FOLDER [RVR]
#
# FOLDER (made if missing) gets the studies, and each step's output in FOLDER/out/NN_step.txt.
# RVR is the rvr binary (default: rvr). It reads from the grid, so it needs a grid proxy. PY
# (default: python) is the command that runs the Python snippets, with rowver and matplotlib.
# SKIM is the local ROOT file (tree SMS_300_200) the holes study of section 11 links 90 times;
# unset, that part is skipped. Nothing is written outside FOLDER (RVR_CONFIG and matplotlib's
# caches point inside it); delete FOLDER after.
set -u

DIR=${1:?usage: walkthrough.sh FOLDER [RVR]}
RVR=${2:-rvr}
PY=${PY:-python}
SKIM=${SKIM:-}
case $SKIM in "" | /*) ;; *) SKIM="$PWD/$SKIM" ;; esac
command -v "$RVR" > /dev/null || { echo "no rvr at $RVR: give the rvr binary as the second argument." >&2; exit 2; }
case $RVR in */*) RVR="$(cd "$(dirname "$RVR")" && pwd)/$(basename "$RVR")" ;; esac
mkdir -p "$DIR" && DIR=$(cd "$DIR" && pwd)
[ -z "$(ls -A "$DIR")" ] || { echo "$DIR is not empty: give an empty folder." >&2; exit 2; }
mkdir -p "$DIR/out" "$DIR/rvr-config" "$DIR/zmumu"
export RVR_CONFIG="$DIR/rvr-config" MPLCONFIGDIR="$DIR/matplotlib"
n=0

# next NAME: f becomes the next output file, out/NN_NAME.txt
next() { n=$((n + 1)); f=$(printf "%s/out/%02d_%s.txt" "$DIR" "$n" "$1"); }

# run NAME ARGS...: rvr ARGS in the current folder, not at a terminal, the output saved
run() {
  local name=$1 a; shift
  next "$name"
  {
    printf '$ rvr'
    for a in "$@"; do
      case $a in *[!A-Za-z0-9_./=-]*) printf ' "%s"' "$a" ;; *) printf ' %s' "$a" ;; esac
    done
    printf '\n'
    "$RVR" "$@" < /dev/null 2>&1
    echo "(exit $?)"
  } > "$f"
  cat "$f"
}

# py NAME: the Python on stdin, written to NAME.py and run in the current folder
py() {
  next "$1"
  cat > "$1.py"
  { $PY "$1.py" < /dev/null 2>&1; echo "(exit $?)"; } > "$f"
  cat "$f"
}

# edit FILE PERL: one substitution, as the guide makes it; one that fails or changes nothing stops
# the run, as the guide and this script then disagree
edit() {
  local before
  before=$(cat "$1")
  perl -pi -e "$2" "$1" || { echo "edit $1: perl refused $2" >&2; exit 1; }
  [ "$(cat "$1")" != "$before" ] || { echo "edit $1: $2 changed nothing" >&2; exit 1; }
}

DY=/DYJetsToLL_M-50_TuneCP5_13TeV-amcatnloFXFX-pythia8/RunIISummer20UL18NanoAODv9-106X_upgrade2018_realistic_v16_L1v1-v2/NANOAODSIM
cd "$DIR/zmumu" || exit 1

# 2. Make your first file
cat > samples.toml <<EOF
[[sample]]
name = "DY"
dataset = "$DY"
tree = "Events"
events = 100_000
EOF
cat > recipe.toml <<'EOF'
format = "rvr-recipe"
version = 1

[define]
good_muon = "Muon_pt > 20 and abs(Muon_eta) < 2.4 and Muon_tightId and Muon_pfRelIso04_all < 0.15"

[keep]
two_muons = "count(good_muon) == 2"

[columns]
branches = ["MET_pt", "nJet"]
mass = "first(pair_mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon], Muon_mass[good_muon]), -1)"

[totals]

[derived]
EOF
run 2_resolve samples resolve samples.toml
run 2_press press
run 2_validate validate zmumu.rvr
run 2_branches branches --find muon
# (the prose's claim: a name the file lacks is answered with the nearest ones)
run 2_try_typo try "count(Muon_ptt > 20)"
run 2_try_count try "count(good_muon)"
run 2_try_pt try "Muon_pt[good_muon]" --kept --events 5
run 2_try_mass try "first(pair_mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon], Muon_mass[good_muon]), -1)" --kept --events 5

# 3. Look inside it
run 3_dump dump zmumu.rvr --columns

# 4. Plot it (the ROOT snippet only where Python has ROOT)
py 4_plot_mass <<'EOF'
import matplotlib.pyplot as plt
import rowver

f = rowver.open("zmumu.rvr")
plt.hist(f["mass"], bins=60, range=(60, 120))
plt.xlabel("dimuon mass [GeV]")
plt.ylabel("events")
plt.savefig("mass.png")
EOF
if $PY -c "import ROOT" > /dev/null 2>&1; then
py 4_plot_root <<'EOF'
import numpy as np
import ROOT
import rowver

f = rowver.open("zmumu.rvr")
mass = f["mass"].astype(np.float64)
met = f["MET_pt"].astype(np.float64)
ones = np.ones(len(mass))

h = ROOT.TH1D("mass", ";m_{#mu#mu} [GeV];events", 60, 60, 120)
h.FillN(len(mass), mass, ones)

h2 = ROOT.TH2D("mass_met", ";m_{#mu#mu} [GeV];p_{T}^{miss} [GeV]", 30, 60, 120, 25, 0, 100)
h2.FillN(len(mass), mass, met, ones)

c = ROOT.TCanvas("c", "", 1200, 500)
c.Divide(2)
c.cd(1)
h.Draw()
c.cd(2)
ROOT.gPad.SetRightMargin(0.15)
h2.SetStats(0)
h2.Draw("colz")
c.SaveAs("mass_root.png")
EOF
else
  next 4_plot_root; echo "skipped: $PY has no ROOT" > "$f"
fi

# 5. Add a column you forgot
edit recipe.toml 's/^branches = \["MET_pt", "nJet"\]$/branches = ["MET_pt", "nJet", "Muon_pt"]/'
run 5_status status
run 5_press press
run 5_status_after status
# (the prose's claim: Python reads Muon_pt as an Awkward array)
py 5_muon_pt_type <<'EOF'
import rowver
print(type(rowver.open("zmumu.rvr")["Muon_pt"]))
EOF

# 6. Add data and another background
cat > samples.toml <<EOF
[defaults]
tree = "Events"
events = 100_000
lumi = 31.84                  # fb⁻¹ of 2018D

[[sample]]
name = "data"
dataset = "/SingleMuon/Run2018D-UL2018_MiniAODv2_NanoAODv9-v1/NANOAOD"
kind = "data"

[[sample]]
name = "DY"
dataset = "$DY"
xsec = 6077.22                # pb

[[sample]]
name = "TT"
dataset = "/TTTo2L2Nu_TuneCP5_13TeV-powheg-pythia8/RunIISummer20UL18NanoAODv9-106X_upgrade2018_realistic_v16_L1v1-v1/NANOAODSIM"
xsec = 88.29
EOF
cat > recipe.toml <<'EOF'
format = "rvr-recipe"
version = 1

[define]
good_muon = "Muon_pt > 20 and abs(Muon_eta) < 2.4 and Muon_tightId and Muon_pfRelIso04_all < 0.15"

[keep]
trigger = "HLT_IsoMu24"
two_muons = "count(good_muon) == 2"

[columns]
branches = ["MET_pt", "nJet", "Muon_pt"]
mass = "first(pair_mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon], Muon_mass[good_muon]), -1)"
weight = "if(sample.kind == 'data', 0 * MET_pt + 1, genWeight * sample.xsec * 1000 * sample.lumi)"

[totals]

[derived]
leading_muon_pt = "first(Muon_pt, -1)"
EOF
run 6_resolve samples resolve samples.toml
run 6_status status
run 6_press press
run 6_dump dump zmumu.rvr --columns

# 7. Data against simulation
cat > recipe.toml <<'EOF'
format = "rvr-recipe"
version = 1

[define]
good_muon = "Muon_pt > 20 and abs(Muon_eta) < 2.4 and Muon_tightId and Muon_pfRelIso04_all < 0.15"

[keep]
trigger = "HLT_IsoMu24"
two_muons = "count(good_muon) == 2"

[columns]
branches = ["MET_pt", "nJet", "Muon_pt"]
mass = "first(pair_mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon], Muon_mass[good_muon]), -1)"
weight = "if(sample.kind == 'data', 0 * MET_pt + 1, genWeight * sample.xsec * 1000 * sample.lumi)"

[totals]
sumw = "if(sample.kind == 'data', 0, genWeight)"

# Worked out from the stored muons when it is read.
[derived]
leading_muon_pt = "first(Muon_pt, -1)"
scaled = "if(sample.kind == 'data', weight, weight / sample.sumw)"
EOF
run 7_press press
run 7_dump dump zmumu.rvr --columns
py 7_plot_stack <<'EOF'
import matplotlib.pyplot as plt
import numpy as np
import rowver

f = rowver.open("zmumu.rvr")

# Only part of the data was read: scale the simulation to the same share of its luminosity.
read, total = f.sample_columns["events_read"], f.sample_columns["events_total"]
share = read[0] / total[0]

bins = np.linspace(60, 120, 31)
mass, scaled, sample = f["mass"], f["scaled"], f["sample"]
counts, _ = np.histogram(mass[sample == 0], bins)
plt.errorbar((bins[1:] + bins[:-1]) / 2, counts, yerr=np.sqrt(counts), fmt="ko", label="data")
plt.hist([mass[sample == 2], mass[sample == 1]], bins, stacked=True, label=["TT", "DY"],
         weights=[scaled[sample == 2] * share, scaled[sample == 1] * share])
plt.xlabel("dimuon mass [GeV]")
plt.ylabel("events")
plt.legend()
plt.savefig("stack.png")
EOF

# 8. Read more events
run 8_plan_all samples plan samples.lock --events all
run 8_plan_random samples plan samples.lock --events 50000 --random --seed 7

# 9. Share it, check it, sign it
run 9_reproduce reproduce zmumu.rvr
run 9_key_new key new "A. Physicist"
# Shown as the guide shows it: the key in its usual folder, the home folder written ~.
perl -pi -e "s{\Q$RVR_CONFIG\E}{~/.config/rvr}g" "$f"
run 9_press_full press --full
run 9_validate validate zmumu.rvr

# 11. When something goes wrong: a misspelt branch, then a dataset's wrong version
edit recipe.toml 's/first\(pair_mass\(Muon_pt\[good_muon\]/first(pair_mass(Muon_ptt[good_muon]/'
run 11_press_typo press --events 1000
edit recipe.toml 's/Muon_ptt\[good_muon\]/Muon_pt[good_muon]/'
edit samples.toml 's{^(dataset = "/TTTo2L2Nu_.*_L1v1)-v1/}{$1-v2/}'
run 11_resolve_typo samples resolve samples.toml
edit samples.toml 's{^(dataset = "/TTTo2L2Nu_.*_L1v1)-v2/}{$1-v1/}'

# 11. A file with holes: 90 copies of one local skim in three samples; one goes, then comes back
if [ -z "$SKIM" ] || [ ! -f "$SKIM" ]; then
  next 11_holes; echo "skipped: no skim (set SKIM to a local ROOT file with tree SMS_300_200)" > "$f"
  exit 0
fi
mkdir -p "$DIR/three/files" && cd "$DIR/three" || exit 1
# Hard links, else copies (clones where the disk makes them): a symbolic link is the file it names,
# so 90 of them would be one file reached 90 times.
for i in $(seq -w 1 90); do ln "$SKIM" "files/f$i.root" 2> /dev/null || cp -c "$SKIM" "files/f$i.root" 2> /dev/null || cp "$SKIM" "files/f$i.root"; done
cat > samples.toml <<'EOF'
format = "rvr-samples"
version = 1

[places]

[defaults]

[eras]

[[sample]]
name = "early"
label = "first files"
files = "files/f0?.root"
tree = "SMS_300_200"

[[sample]]
name = "middle"
label = "middle files"
files = ["files/f1?.root", "files/f2?.root", "files/f3?.root"]
tree = "SMS_300_200"

[[sample]]
name = "late_and_long_named_sample"
label = "the rest"
files = ["files/f4?.root", "files/f5?.root", "files/f6?.root", "files/f7?.root", "files/f8?.root", "files/f9?.root"]
tree = "SMS_300_200"
EOF
cat > recipe.toml <<'EOF'
format = "rvr-recipe"
version = 1

[define]

[keep]
met = "MET > 150"
leptons = "Nlep >= 2"

[columns]
branches = ["MET", "PTISR", "RISR", "Mperp", "Nlep", "weight", "PT_lep"]

[totals]

[derived]
EOF
run 11_holes_resolve samples resolve samples.toml
rm files/f45.root
run 11_holes_press press
run 11_holes_status status
ln "$SKIM" files/f45.root 2> /dev/null || ln -s "$SKIM" files/f45.root
run 11_holes_fill press --fill
run 11_holes_validate validate three.rvr --complete
