# Test ROOT files

Copied byte for byte from hep-crates `crates/hep-root/tests/vectors` at 367aa92, where their
README says how each was made: `uproot_flat.root` by uproot (`make_ntuple_fixtures.py`),
`stl_vectors.root` by ROOT 6.38 (`make_root_fixtures.C`), and `codecs-zlib.root` (a seven-entry
`Events` tree and an `Empty` one) by uproot 5.7.5 (`make_codec_fixtures.py`). `tests/press.rs` presses them.
