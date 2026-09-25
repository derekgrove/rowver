# The recipe dialect

RowVer's app edits a study's `cart.py` in a form. The form can only edit a `cart.py` it can
read without running it. This page defines which files those are. It also says how
`rowver recipe --json` reports them, and exactly how the form prints them back.

The reader is `rowver/recipe.py`. It parses the file with Python's `ast` module and never
imports or runs it. A file outside the dialect is still a good recipe: the presser presses it
the same way, and the app shows it as text.

The four shipped recipes under `studies/` are in the dialect, in the printed layout.

## The shape of a file

```python
"""The CMS Open Data dimuon spectrum."""

import awkward as ak
import numpy as np

from rowver import Cart


def keep(events):
    """Two or more muons."""
    return events.nMuon >= 2


def mass(events):
    """Invariant mass of the two leading muons."""
    pt, eta, phi = events.Muon_pt, events.Muon_eta, events.Muon_phi
    return np.sqrt(2 * pt[:, 0] * pt[:, 1] * (np.cosh(eta[:, 0] - eta[:, 1]) - np.cos(phi[:, 0] - phi[:, 1])))


def charge(events):
    """Sum of the two leading muons' charges: -2, 0, or 2."""
    return events.Muon_charge[:, 0] + events.Muon_charge[:, 1]


# ---- recipe ----
FILESET = {
    "DoubleMuParked 2012B+C": {
        "files": {
            "https://opendata.cern.ch/.../Run2012BC_DoubleMuParked_Muons.root": {"tree": "Events", "stop": 800000},
        },
        "metadata": {},
    },
}

COLUMNS = {
    "mass": mass,
    "charge": charge,
    "Nmuon": "nMuon",
}

LABELS = {
    "charge": {-2: "mu- mu-", 0: "mu+ mu-", 2: "mu+ mu+"},
}

CART = Cart(
    fileset=FILESET,
    columns=COLUMNS,
    keep=keep,
    prepare=None,
    labels=LABELS,
    title="CMS Open Data dimuons",
    note=None,
)
```

A file has two regions. The **recipe line** is exactly `# ---- recipe ----`, at the start of a
line. There is exactly one recipe line, and it may not sit inside a statement.

- **The hooks region** is everything above the recipe line. It holds any Python at all:
  imports, constants, helper functions and the hooks. The form never changes it. The app edits
  it as text and keeps it byte for byte.
- **The recipe region** is the recipe line and everything below it. It holds only the four
  statements below, written as plain values. The form prints this region again whenever it
  saves, so comments here are not kept.

## The recipe region, statement by statement

Each name is assigned once. `FILESET`, `COLUMNS` and `CART` are required, `LABELS` is optional,
and `CART` comes last.

| Statement | What it may hold |
| --- | --- |
| `FILESET = {...}` | `{sample: {"files": {path: tree}, "metadata": {key: value}}}`. The keys are `"files"` and `"metadata"`, and nothing else. `tree` is a tree name such as `"Events"`, or `{"tree": "Events", "stop": N}` with a whole number `N` of 1 or more (read only the first `N` entries). |
| `COLUMNS = {...}` | `{column: "branch or field"}` or `{column: hook}`. The string names a branch, or a field that `prepare` made. A bare name is a hook bound in the hooks region. |
| `LABELS = {...}` | `{column: {value: "label"}}`. `value` is a whole-number literal, and may be negative, such as `-1`. Every labelled column is in `COLUMNS`. |
| `CART = Cart(...)` | Keywords only: `fileset=FILESET`, `columns=COLUMNS`, `keep=<hook>`, `prepare=<hook or None>`, `labels=<LABELS or None>`, `title=<text or None>`, `note=<text or None>`. `keep` is required. A missing `prepare`, `labels`, `title` or `note` means `None`. If `LABELS` is written, `labels=LABELS` must be passed. |

The values below are allowed in the recipe region:

- **Text:** plain string literals. Adjacent literals such as `"a" "b"` are joined by Python and
  count as one string.
- **Metadata values:** text, whole numbers, finite floats, `True`, `False`, `None`, or a list
  `[...]` of those.
- **Keys:** keys are text, except label values, which are whole numbers. A key may appear only
  once in each `{...}`.
- **Hooks:** a hook is a name that the hooks region binds. It may be bound by a `def`, by an
  assignment, or by an import such as `from helpers import keep`. `Cart` must be bound too, by
  `from rowver import Cart`.

## What leaves the dialect

Any of these makes `rowver recipe --json` answer `{"dialect": false, "reason": ...}`, and the
app shows the file as text. Each reason names the line.

- **No recipe line, or more than one.** The LPC study's `cart.py` is an example. It is
  hand-written, imports its `FILESET` from `fileset.py`, and names columns as `H.ratio`.
- **An f-string** such as `f"{DAS}/store/..."`. An f-string is not a literal, so the form
  cannot edit it. Write the path out in full.
- **A lambda column.** Give it a `def` in the hooks region and name it in `COLUMNS`.
- **A label key that is not a whole-number literal,** such as `"-1"`, `True` or `0.5`. The
  presser looks labels up by integer, so a string key would never match.
- **Other values:** a name, call or expression where a plain value belongs; a tuple; a dict
  inside metadata; `**` unpacking; or a repeated key.
- **A missing name:** a hook name that the hooks region does not bind, or a `Cart` that is not
  imported.
- **Any other statement** below the recipe line.
- **A syntax error.** The answer then also carries `"line"`.

## The hard cases

- **Numbers with underscores.** `2_400_000` is in the dialect and reads as `2400000`. The
  printer writes plain digits, so the file is not yet in the printed layout
  (`"canonical": false`) until the form saves it.
- **An f-string** leaves the dialect, as above.
- **Label keys** are whole numbers. `-1` is a unary minus to `ast`, and the reader accepts it.
  JSON cannot use an integer as an object key, so labels travel as `[value, label]` pairs with
  `value` a JSON integer. The printer writes them back as integer literals, as in
  `{-1: "opposite sign", 1: "same sign"}`.

## `rowver recipe --json`

```bash
python -m rowver recipe <study folder, or a .py file> --json
```

The command prints one JSON object on one line and exits 0. It exits 1 only when the file
cannot be read, and then prints `{"event": "error", "kind": "input", "message": ...}`.

Every mapping is sent as a list of `[key, value]` pairs in file order. JavaScript objects move
number-like keys such as `"2018"` to the front, so a plain object would reorder a sample named
`2018`. For the same reason, `LABELS` travels as `[[column, [[value, label], ...]], ...]` and
not as the `{column: [[value, label], ...]}` that the plan first sketched.

In the dialect:

```json
{
  "dialect": true,
  "canonical": true,
  "hooks": "\"\"\"The CMS Open Data dimuon spectrum.\"\"\"\n\nimport awkward as ak\n...\n\n\n",
  "bound": ["ak", "np", "Cart", "keep", "mass", "charge"],
  "functions": ["keep", "mass", "charge"],
  "fileset": [["DoubleMuParked 2012B+C", {
    "files": [["https://opendata.cern.ch/.../Run2012BC_DoubleMuParked_Muons.root", {"tree": "Events", "stop": 800000}]],
    "metadata": []
  }]],
  "columns": [["mass", {"hook": "mass"}], ["charge", {"hook": "charge"}], ["Nmuon", {"branch": "nMuon"}]],
  "labels": [["charge", [[-2, "mu- mu-"], [0, "mu+ mu-"], [2, "mu+ mu+"]]]],
  "cart": {"keep": "keep", "prepare": null, "title": "CMS Open Data dimuons", "note": null},
  "siblings": [],
  "missing": []
}
```

The fields:

- **`hooks`** is the file up to the recipe line, exactly as it is on disk, except that line
  ends are `\n`. Python reads files with universal newlines.
- **`bound`** lists every name the hooks region binds at the top level. **`functions`** lists
  its top-level `def`s. Both are in file order.
- **`fileset`, `columns`, `labels`, `cart`** are the four statements. A file is either a tree
  name or `{"tree", "stop"}`. A column is `{"branch": text}` or `{"hook": name}`.
- **`canonical`** is true when the recipe region is already exactly what the printer below
  writes for these values.
- **`siblings`** are the modules the file imports anywhere, inside a `def`, `try` or `if` as
  well, that live in its folder (`<name>.py` or `<name>/__init__.py`), or that this Python
  cannot find, in the order they appear. **`missing`** lists the siblings that
  are not in the folder yet. Both are reported inside and outside the dialect.

Outside the dialect the object is `{"dialect": false, "reason", "siblings", "missing"}`,
plus `"line"` for a syntax error. `reason` is one plain sentence and ends with the line it
concerns, when it has one.

A hooks region that imports a sibling needs the line
`sys.path.insert(0, str(Path(__file__).resolve().parent))` before that import, because the
presser adds nothing to `sys.path`. When the form adds a sibling import, it writes that line
too.

## How the form prints the recipe region

The form writes the recipe region from the values alone. The page's printer
(`web/src/core/recipe.ts`) and the presser's reference printer (`recipe.region`) must give the
same bytes. For each shipped recipe, both printers must give back the file exactly.

1. **Layout.** The region is the recipe line, then `FILESET`, `COLUMNS`, `LABELS` (only when
   it has an entry) and `CART`. One blank line separates statements. The file ends with one
   `\n`.
2. **Blocks.** `FILESET`, `COLUMNS` and `LABELS` are blocks: `NAME = {`, one entry per line
   indented four spaces and followed by a comma, then `}`. An empty block is `NAME = {}`.
3. **`FILESET` entries.** A sample is `"name": {`. Then come `"files": {` at eight spaces, each
   file at twelve spaces as `"path": tree,`, the closing `},` at eight spaces, and
   `"metadata": {...},` on one line at eight spaces. The sample closes with `},` at four
   spaces. A sample with no files prints `"files": {},`.
4. **Values on one line.** A file's tree is `"Events"` or `{"tree": "Events", "stop": 800000}`.
   Metadata is `{"key": value, ...}`, or `{}` when empty. A label map is
   `{-1: "negative", 1: "positive"}`. Items are separated by `, `.
5. **`COLUMNS` entries.** An entry is `"name": hook` or `"name": "branch"`.
6. **`CART`.** It is printed as `CART = Cart(` followed by all seven keywords, one per line at
   four spaces, each ending in a comma, then `)`. The order is `fileset=FILESET`,
   `columns=COLUMNS`, `keep=<hook>`, `prepare=<hook or None>`, `labels=<LABELS or None>`,
   `title=<text or None>`, `note=<text or None>`.
7. **Text.** Text is written in double quotes. `\` and `"` are escaped with a backslash, and
   newline, carriage return and tab become `\n`, `\r` and `\t`. Any other character below
   U+0020, or from U+007F to U+009F, becomes `\xNN` with two lowercase hex digits. Every other
   character is written as itself, non-ASCII included.
8. **Numbers.** Whole numbers are written as plain decimal digits, with `-` when negative and
   no underscores. Floats use Python's `repr`, such as `6077.22`, `1.0` or `1e-05`.
   `True`, `False` and `None` are written as they are. A list is `[a, b]`.

One caution for the page's printer: JSON has one number type. A metadata float with a whole
value, such as `1.0`, arrives in JavaScript as `1`. A printer that writes it back as `1`
changes its type in Python. Keep such a value's text unless the person edits it.
