# RowVer

RowVer turns ROOT ntuples into one `.rvr` file that you open in
[RowVer Web](https://rowver.web.cern.ch) to plot, cut, and make tables in the browser.

## Install

```bash
git clone https://gitlab.cern.ch/dgrove/rowver.git
cd rowver
mamba env create -f environment.yml
mamba activate rowver
pip install -e .
```

(`conda` works in place of `mamba`.)

## Run the demos

Each study is a folder under `studies/` with one `cart.py` in it.
`rowver run` writes the `.rvr` file into the same folder. Open
[RowVer Web](https://rowver.web.cern.ch) and drop the file onto the page.

### 1. CMS Open Data dimuons

Read over https, so it needs no grid certificate.

```bash
rowver run studies/dimuon_open_data
```

### 2–4. CMS files from DAS

These read `root://` files, so they need a grid proxy first. On lxplus the
certificate directory is already set; elsewhere, set it yourself:

```bash
voms-proxy-init --voms cms --valid 192:00
export X509_USER_PROXY=/tmp/x509up_u$(id -u)
export X509_CERT_DIR=/cvmfs/grid.cern.ch/etc/grid-security/certificates
```

Each remote file can take a few minutes.

```bash
rowver run studies/simple                   # 2. one MC file, branches copied as they are
rowver run studies/data_and_mc              # 3. data and MC in one file; data gets a weight of 1
rowver run studies/advanced --parallel 5    # 4. Run 2 and Run 3 data and MC, five files read at once
```

## Make your own study

```bash
mkdir studies/my_study
cp studies/simple/cart.py studies/my_study/
```

Edit `studies/my_study/cart.py`: point it at your files, choose which events
to `keep`, and list the `columns` you want. Then:

```bash
rowver run studies/my_study
rowver validate studies/my_study/my_study.rvr
```

## Look inside a file

```bash
rowver dump studies/my_study/my_study.rvr             # everything: header, recipe, first values of every column
rowver dump studies/my_study/my_study.rvr --fileset   # only the files that were read
rowver dump studies/my_study/my_study.rvr --cart      # only the recipe
rowver dump studies/my_study/my_study.rvr --columns   # only the first values of every column
rowver validate studies/my_study/my_study.rvr         # check the file agrees with itself
```

The file format is described in [FORMAT.md](FORMAT.md).
