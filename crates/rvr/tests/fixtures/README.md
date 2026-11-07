# Test ROOT files

Copied byte for byte from hep-root's `tests/vectors` (gitlab.cern.ch/dgrove/hep-root), where their
README says how each was made: `uproot_flat.root` by uproot (`make_ntuple_fixtures.py`),
`stl_vectors.root` by ROOT 6.38 (`make_root_fixtures.C`), and `codecs-zlib.root` (a seven-entry
`Events` tree and an `Empty` one) by uproot 5.7.5 (`make_codec_fixtures.py`). `tests/press.rs` presses them.
`runs.root` (1,200 entries of `Events` in 24 baskets of 50: jets of up to 11 members, MET, weight, a
flag, and each event's run and unique number) was written for this repository by
`make_runs_fixture.py` with uproot 5.7.5, awkward 2.12.0 and NumPy 2.5.1 (`mamba run -n rowver
python tests/fixtures/make_runs_fixture.py tests/fixtures/runs.root`); `tests/core.rs` presses it
in runs of every size, and `tests/diff.rs` compares its presses event by event. `rntuple.root`
(an RNTuple `Events` of three entries of `nJet`) was written by uproot 5.7.5's `mkrntuple`;
`tests/resolve.rs` names it as a tree. `nan.root` (five entries of `Events`: `x`, `y` and `v`, an
`f8`, an `f4` and a list of `f8`, holding NaNs of either sign, with a payload, and signalling) was
written by `make_nan_fixture.py` with uproot 5.7.5, awkward 2.12.0 and NumPy 2.5.1; `tests/press.rs`
checks the bits a press keeps and those it makes the one NaN.

`tls/` holds certificates for the tests' HTTPS storage (`tests/storage`), made once with the
openssl 3 command, each valid for 100 years: `authority/ca.pem`, a test authority the tests trust
through `$X509_CERT_DIR`; `server.pem` (with `server.key`), for `localhost` and `127.0.0.1`, signed
by it; `stranger.pem`, the same names signed by an authority nobody trusts (thrown away); and
`user.pem`, a user certificate signed by the test authority, from which the tests make grid
proxies. The keys are 2048-bit RSA, unencrypted, and protect nothing.
