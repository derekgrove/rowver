# Set up rvr on your laptop

`rvr` is meant to run on your own laptop. It reads ROOT files on your computer, or straight from
grid storage over HTTPS. Files anyone can read, such as CMS Open Data, need nothing more than
step 1. Files on grid storage that needs a login (LPC EOS, most CERN EOS areas, a grid site) need a
grid proxy, and for that your grid certificate: steps 2 to 4. You only need to set this up once.

## 1. Install rvr and rowver

`rvr` makes `.rvr` files; the Python package `rowver` reads them
(from [step 4 of the examples](examples/README.md#4-plot-it) on). One `pip install` brings both:
the package, and the `rvr` command with it. It needs Python 3.11 or later:

```sh
pip install "rowver[all]"
rvr --version
```

(`[all]` adds Awkward, pandas and pyarrow, for list columns and tables; `pip install rowver` needs
only NumPy. For `rvr` alone, with no Python, download the binary for Linux x86_64 or macOS from the
[releases](https://gitlab.cern.ch/dgrove/rowver/-/releases).) To build from a checkout instead, see
[Building from a checkout](#5-building-from-a-checkout).

The rest of this page is for files on grid storage that needs a login.

## 2. Install your grid certificate on your laptop

`rvr` reads it from `~/.globus`, as the grid
tools on lxplus and the LPC do. If you already use the grid there, copy the certificate you have:

```sh
mkdir -p ~/.globus
scp username@lxplus.cern.ch:'.globus/user*.pem' ~/.globus/
chmod 600 ~/.globus/userkey.pem
```

If you have no certificate yet, ask the [CERN Certification Authority](https://ca.cern.ch) for a
grid user certificate and join the CMS virtual organisation with it, as the
[CMS WorkBook](https://twiki.cern.ch/twiki/bin/view/CMSPublic/WorkBookStartingGrid) explains
(approval can take a day). The CA gives you a `.p12` file; `rvr` reads it as it is:

```sh
mkdir -p ~/.globus
mv ~/Downloads/myCertificate.p12 ~/.globus/usercred.p12
chmod 600 ~/.globus/usercred.p12
```

## 3. Make a grid proxy

A proxy is a pass made from your certificate that lasts 8 days. Make one
now, and again whenever it runs out. The passphrase is whatever you used when you created your certificate:

```text
$ rvr proxy init
Passphrase for /home/you/.globus/userkey.pem:
Asking voms-cms-auth.cern.ch for your cms attributes...
Wrote /tmp/x509up_u501:
…
The grid proxy at /tmp/x509up_u501 is valid for 8 days.
```

## 4. Check it

`rvr proxy` shows the proxy `rvr` will use. You are ready when its last line says
it is valid:

```text
$ rvr proxy
proxy     /tmp/x509up_u501
whose     /DC=ch/DC=cern/OU=Organic Units/OU=Users/CN=…
VO        cms (attributes until 2026-11-06T18:49:23Z)
left      7 days 23 hours, until 2026-11-06T18:49:23Z
The grid proxy at /tmp/x509up_u501 is valid for 7 days.
```

If you already use `voms-proxy-init -voms cms` on your laptop, the proxy it makes works too.
`rvr` needs no certificate folder: it trusts the grid's certificate authorities with no setup
([the specification, §10](SPEC.md#10-keys-and-proxies), says which, and every other rule of the
proxy and of your signing key).

Now you are ready to move on to [the examples](examples/README.md).

## 5. Building from a checkout

For working on RowVer itself. It needs Rust 1.91 or later ([rustup](https://rustup.rs)) and, for
the Python package, a Python 3.11 or later with [maturin](https://www.maturin.rs) and NumPy:

```sh
git clone https://gitlab.cern.ch/dgrove/rowver.git && cd rowver
cargo build --release -p rvr                  # the command, as target/release/rvr
cargo test --workspace --exclude rowver-py    # every Rust test
cd crates/rowver-py
maturin develop --release                     # the Python package, into the active Python
python -m unittest discover tests             # its tests
```

`cargo test -p rvr --test examples -- --ignored` presses every [example study](examples/README.md)
again, reading Open Data over the network.
