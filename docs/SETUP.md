# Set up rvr on your laptop

`rvr` is meant to run on your own laptop. It reads CMS datasets straight from grid storage, so
your laptop needs a grid proxy, and for that it needs your grid certificate. You only need to set this up once.

## 1. Install rvr and rowver

`rvr` makes `.rvr` files; the Python package `rowver` reads them
(from [step 4 of the examples](examples/README.md#4-plot-it) on). One `pip install` builds the package from this repository and brings the `rvr`
command with it, with no clone. It needs Python 3.11 or later and Rust 1.91 or later
([rustup](https://rustup.rs)):

```sh
pip install "rowver[all] @ git+https://gitlab.cern.ch/dgrove/rowver.git#subdirectory=crates/rowver-py"
rvr --version
```

(`[all]` adds Awkward, pandas and pyarrow, for list columns and tables. For `rvr` alone, run
`cargo install --locked rvr`, or download it from the
[releases](https://gitlab.cern.ch/dgrove/rowver/-/releases) with no Rust at all; see
[Install](../crates/rvr/README.md#install).)

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
Passphrase for /Users/…/.globus/userkey.pem:
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

Now you are ready to move on to [the examples](examples/README.md).
