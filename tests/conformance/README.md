# The conformance corpus

Inputs of every kind RowVer reads, each with the verdict and the values the specification gives
it, for any implementation of the format (and the reference, `crates/rowver`, which makes and checks
this folder on every test run) to check itself against. Each part is a folder with a
`manifest.json`; texts are written into the manifest, files beside it.

| part | holds | specified by |
|---|---|---|
| `rvr/` | `.rvr` files: valid ones in `valid/`, and every case of `cases.zst` | docs/FORMAT.md |
| `samples/` | `samples.toml` texts | docs/SPEC.md §3 |
| `lock/` | `samples.lock` texts | docs/FORMAT.md §6.3 |
| `recipe/` | `recipe.toml` texts; the expressions are `crates/rvr-recipe/tests/fixtures/recipe.json` and `properties.json` | crates/rvr-recipe/README.md |

## `rvr/manifest.json`

- `valid`: each `{"file", "verdict": "valid", "signed", "contents"}`. `contents` is what reading the
  file gives: `rows`, `empty`, the `sample` and `entry` of each row (null when empty), and
  `columns`, every stored column and derived column by name: its `kind`, its `values` in row
  order (for a list column, every member of every row, and `offsets`, the `rows + 1` offsets), a
  float as the hexadecimal of its bits at its own width (`3ff0000000000000` is the `f8` 1.0,
  `3f800000` the `f4` 1.0), a whole number as itself, a `b1` as true or false.
- `cases`: each `{"rule", "verdict", "refused", "at", "size", "sha256", "rust_says"}`. Decompress
  `cases.zst` (one zstd frame, `zstd -d cases.zst`): case `n` is the `size` bytes from byte `at`,
  whose SHA-256 is `sha256`. `verdict` is `valid`, `opening` (a reader refuses it on opening:
  FORMAT.md §12) or `full` (it opens, and full validation refuses it). `rule` names the rule the
  case breaks, by FORMAT.md's section; `rust_says` is the reference's refusal, for a person: another
  implementation need not say it in those words.

## `samples/`, `lock/` and `recipe/`

Each `manifest.json` holds `cases`, each `{"rule", "text", "verdict"}`: `valid` or `refused`. A
valid `samples.toml` has `samples`, the samples it makes in order (SPEC.md §3.7: name, label,
tree, files asked, event choice as `made.files` records it, metadata); a valid recipe has
`names`, what it names table by table. A refused text has `rust_says`.

## Making it again

`CONFORMANCE_BLESS=1 cargo test -p rowver -p rvr-recipe` writes it again from the reference; a
plain `cargo test` fails when the folder is not what the code makes. Review the change as any
other: a different verdict is a change of the format or the language.
