# Recipe language

Your `recipe.toml` tells `rvr press` two things: which events to keep, and what to store for each
one. You will write **expressions**, a short formula over the event's branches,
like `MET_pt > 30` or `count(good_muon) >= 1`. This page is the lookup for all of it: every word,
operator and function you can use, each with an example and what it actually gives you.

A quick note before we start: expressions look a lot like Python, but they aren't Python. `rvr`
reads every one of them before it opens a single file, so a typo shows up right away and not an
hour into a press. It then works on whole columns of 50,000 events at a time (there is no loop
over events), and the numbers follow NumPy's rules, so a cut keeps exactly the events the same
formula would keep in NumPy or Awkward. Only the branches you mention are read from the file.

If you already write your selections in Python with Awkward, [the last section](#from-python-to-rvr)
puts some common ones side by side with how you'd write them here.

## The example events

Every result on this page is what `rvr` really gives on the three made-up events below, and a
test checks each one, so they can't quietly go out of date. Two words we'll use a lot: a **flat**
branch has one value per event, like `MET_pt`, and a **jagged** branch has a list per event, like
`Jet_pt` with one entry per jet, so the lists have different lengths. A **collection**, like `Jet`,
is the jagged branches that share a name and a length (`Jet_pt`, `Jet_eta`, …).

| branch | event 0 | event 1 | event 2 |
|---|---|---|---|
| `Jet_pt` | `[65, 22, 41]` | `[]` | `[120, 35]` |
| `Jet_eta` | `[0.5, -2.1, 1.2]` | `[]` | `[-0.2, 0.9]` |
| `Jet_phi` | `[0.1, 2.9, -1.0]` | `[]` | `[3.0, -3.0]` |
| `Jet_mass` | `[8, 4, 6]` | `[]` | `[12, 5]` |
| `nJet` | `3` | `0` | `2` |
| `Muon_pt` | `[44, 12]` | `[25]` | `[]` |
| `Muon_eta` | `[0.4, 1.9]` | `[-0.3]` | `[]` |
| `Muon_phi` | `[0.2, -2.8]` | `[1.5]` | `[]` |
| `Muon_tightId` | `[true, false]` | `[true]` | `[]` |
| `Muon_charge` | `[1, -1]` | `[-1]` | `[]` |
| `MET_pt` | `31.5` | `85` | `16` |
| `MET_phi` | `-0.5` | `2.0` | `1.0` |
| `Flags` | `11` (binary 1011) | `4` (0100) | `1` (0001) |

The sample these events come from has `era = 3`, `kind = "signal"` and `xsec = 0.386` in its
`samples.toml` entry.

Results are written one value per event: `[31.5, 85, 16]` is a flat result (a number for each of
the three events), and `[[65, 41], [], [120, 35]]` is a jagged one (a list for each).

## The parts of a recipe

```toml
format = "rvr-recipe"
version = 1
title = "my study"                  # optional

[object.good_jet]                   # a collection filtered by a condition
from = "Jet"
where = "pt > 30 and abs(eta) < 2.4"

[keep]                              # the cuts: an event is kept when it passes every one
two_jets = "count(good_jet) >= 2"

[copy]                              # branches stored as they are
branches = ["MET_pt", "Jet_pt"]

[column]                            # computed columns, stored after the copies
ht = "sum(good_jet.pt)"
```

| table | what it holds |
|---|---|
| `[object.NAME]` | A new collection: the members of `from` that pass `where`. The two words are borrowed from SQL (`SELECT … FROM Jet WHERE pt > 30`). Inside `where`, a bare name is that member's own branch, so `pt` means `Jet_pt`. It picks jets *inside* each event; it never drops an event. You don't define `Jet`: any branches that share a prefix (`Jet_pt`, `Jet_eta`, …) are a collection, and `rvr branches` lists the ones your file has. |
| `[keep]` | Your cuts, in order. Each one is `name = "condition"`, true or false for each event, and an event is kept only if all of them are true. `rvr press` prints how many events are left after each cut, so this is your cutflow too. |
| `[copy]` | `branches = [...]`: branches stored just as the ROOT file has them, types and all. `NAME = "BRANCH"` stores a branch under a new name, and `NAME = "branch(sample.FIELD)"` stores a branch whose name each sample gives in its metadata. |
| `[column]` | `NAME = "expression"`: a new column you compute, flat or jagged. Write `NAME = { value = "expression", stored = "f4" }` to choose the type it's stored as. |
| `[helper.NAME]` | An expression you want to reuse, like a small function: `value = "…"`, plus `takes = ["a", "b"]` if it takes inputs, and an optional `about` note. (`pass = "…"` means the same as `value`.) |
| `[collection.NAME]` | For branches that don't follow the `Jet_pt` naming: build a collection by hand, `pt = "PT_lep"`, `eta = "Eta_lep"`, and use it like any other. |

One thing that trips people up: only what `[copy]` and `[column]` name ends up in the file.
Objects and helpers are just working names. A cut on `good_jet` decides which *events* you keep,
but `Jet_pt` in `[copy]` still stores every jet of those events. If you want only the good jets,
store them as a column: `good_jet_pt = "good_jet.pt"`.

### Cuts for some samples only

Sometimes a cut only makes sense for some samples, like a veto for one era. Write the cut as a
table with an `if`, and it applies only to the samples where the `if` is true; every other
sample's events go straight through:

```toml
[keep]
hem = { if = "sample.era == 3", apply = "hem_veto()" }
```

A sample that has no `era` at all isn't cut either. (`pass` works in place of `apply` too.)

## Values

| expression | gives | result |
|---|---|---|
| `MET_pt` | a flat branch: one value per event | `[31.5, 85, 16]` |
| `Jet_pt` | a jagged branch: a list per event, one entry per jet | `[[65, 22, 41], [], [120, 35]]` |
| `Jet.pt` | the same jagged branch, named as the attribute `pt` of the collection `Jet` | `[[65, 22, 41], [], [120, 35]]` |
| `Muon.charge` | the branch `Muon_charge`, named the same way | `[[1, -1], [-1], []]` |
| `sample.era` | a field of the sample's metadata: one value, the same for every event | `3` |
| `sample.kind` | a text field | `"signal"` |
| `sample.missing` | a field this sample's metadata does not have | missing |

A comparison with a field the sample doesn't have is always false (even `!=`), and doing
arithmetic with one is an error, as is giving one alone to the function `if`: `if(sample.isData, …)`
needs every sample to have `isData` (give it to all of them in `[defaults]` of `samples.toml`),
while `if(sample.isData == true, …)` takes a sample without it as false. (A cut's own `if`, above,
lets a sample without the field through.)

You can write numbers (`30`, `2.5`, `1e3`, `1_000`), text in single or double quotes
(`'signal'`), and `true` or `false` straight into an expression.

## Operators

These work the way you'd expect. From loosest to tightest binding they are: `where`; `or`; `and`;
`not`; the comparisons and `in`; `+ -`; `* / %`; unary `-`; `**`; and then `.name`, `[...]` and
function calls. When in doubt, use parentheses. When you combine a flat value with a jagged one,
the flat value is used for every member: `Jet_pt - MET_pt` subtracts the event's MET from each of
its jets.

| expression | result |
|---|---|
| `MET_pt * 2` | `[63, 170, 32]` |
| `MET_pt / 1000` | `[0.0315, 0.085, 0.016]` |
| `Jet_pt + 1` | `[[66, 23, 42], [], [121, 36]]` |
| `Jet_pt - MET_pt` | `[[33.5, -9.5, 9.5], [], [104, 19]]` |
| `nJet % 2` | `[1, 0, 0]` |
| `nJet ** 2` | `[9, 0, 4]` |
| `-MET_pt` | `[-31.5, -85, -16]` |
| `MET_pt > 30` | `[true, true, false]` |
| `Jet_pt > 40` | `[[true, false, true], [], [true, false]]` |
| `Jet_pt > MET_pt` | `[[true, false, true], [], [true, true]]` |
| `MET_pt > 30 and nJet >= 2` | `[true, false, false]` |
| `MET_pt > 30 or nJet == 0` | `[true, true, false]` |
| `not (MET_pt > 30)` | `[false, false, true]` |
| `nJet in [0, 2]` | `[false, true, true]` |
| `sample.kind == 'signal'` | `true` |
| `MET_pt * sample.xsec` | `[12.159, 32.81, 6.176]` |

The comparisons are `==`, `!=`, `<`, `<=`, `>` and `>=`.

## Choosing members of a list

This is where `where` and `[...]` come in: they pick out the members of a list you care about,
event by event.

| expression | gives | result |
|---|---|---|
| `Jet_pt[Jet_pt > 30]` | the members a true-or-false list of the same shape marks | `[[65, 41], [], [120, 35]]` |
| `Jet_pt[Jet_pt > 30 and Jet_eta > 0]` | the same, with a combined condition | `[[65, 41], [], [35]]` |
| `Muon_pt[Muon_tightId]` | marked by a true-or-false branch | `[[44], [25], []]` |
| `count(Jet where pt > 30 and abs(eta) < 2.4)` | `where` written inline; bare names are the jet's attributes | `[2, 0, 2]` |
| `good_jet.pt` | an attribute of an object (`from = "Jet"`, `where = "pt > 30 and abs(eta) < 2.4"`) | `[[65, 41], [], [120, 35]]` |
| `good_jet.eta` | the same jets, another attribute | `[[0.5, 1.2], [], [-0.2, 0.9]]` |
| `count(good_jet)` | how many members passed | `[2, 0, 2]` |
| `good_muon.pt` | `from = "Muon"`, `where = "tightId and pt > 20"` | `[[44], [25], []]` |
| `count(good_muon) >= 1` | a cut on it | `[true, true, false]` |
| `count(lep where pt > 20)` | a hand-built `[collection]` works the same way (here `lep` is the muons: `pt = "Muon_pt"`) | `[1, 1, 0]` |

A collection on its own isn't a value you can store or compare: use one of its branches
(`good_jet.pt`) or count it (`count(good_jet)`). One `where` takes one condition, so join several
with `and`.

## Functions

Some functions take an `otherwise`: that's what you get back for an event whose list is empty.
Event 1 has no jets, so keep an eye on it in the examples.

### Reducing a jagged list to a flat value

| function | gives | example | result |
|---|---|---|---|
| `count(list)` | how many members; of a true-or-false list, how many are true | `count(Jet_pt > 30)` | `[2, 0, 2]` |
| `count(collection)` | how many members | `count(Jet)` | `[3, 0, 2]` |
| `sum(list)` | the sum (0 for an empty list) | `sum(Jet_pt)` | `[128, 0, 155]` |
| `any(list)` | whether any member is true | `any(Jet_pt > 100)` | `[false, false, true]` |
| `all(list)` | whether every member is true (true for an empty list) | `all(Jet_pt > 30)` | `[false, true, true]` |
| `least(list, otherwise)` | the smallest member | `least(Jet_pt, -1)` | `[22, -1, 35]` |
| `greatest(list, otherwise)` | the largest member | `greatest(Jet_pt, -1)` | `[65, -1, 120]` |
| `least_index(list, otherwise)` | the position of the smallest, counting from 0 | `least_index(Jet_pt, -1)` | `[1, -1, 1]` |
| `greatest_index(list, otherwise)` | the position of the largest | `greatest_index(Jet_pt, -1)` | `[0, -1, 0]` |
| `first(list, otherwise)` | the first member | `first(Jet_pt, -1)` | `[65, -1, 120]` |
| `nth(list, n, otherwise)` | the member at position `n`, counting from 0 | `nth(Jet_pt, 1, -1)` | `[22, -1, 35]` |

For example, the leading good jet's pt is `nth(good_jet.pt, 0, 0)`, which gives `[65, 0, 120]`.

### Positions and order

| function | gives | example | result |
|---|---|---|---|
| `index(list)` | each member's position | `index(Jet_pt)` | `[[0, 1, 2], [], [0, 1]]` |
| `sorted(list)` | the members in ascending order | `sorted(Jet_pt)` | `[[22, 41, 65], [], [35, 120]]` |
| `sorted(list, 'descending')` | in descending order | `sorted(Jet_pt, 'descending')` | `[[65, 41, 22], [], [120, 35]]` |
| `order(list)`, `order(list, 'descending')` | the positions that would sort the list | `order(Jet_pt, 'descending')` | `[[0, 2, 1], [], [0, 1]]` |
| `take(list, positions, otherwise)` | the members at those positions; `otherwise` where a position is -1 or past the end | `take(Jet_eta, order(Jet_pt, 'descending'), -9)` | `[[0.5, 1.2, -2.1], [], [-0.2, 0.9]]` |
| `listed(list, positions)` | for each member, whether its position is among `positions` | `listed(Jet_pt, nearest_index(Muon_eta, Muon_phi, Jet_eta, Jet_phi))` | `[[true, false, true], [], [false, false]]` |

In words: the `take` example lists the jets' eta from the highest-pt jet down, and the `listed`
example marks each jet that is the closest jet to some muon.

### Math, value by value

These work on flat and jagged values alike, value by value, and keep whatever float type you
give them.

| function | example | result |
|---|---|---|
| `abs(x)` | `abs(Jet_eta)` | `[[0.5, 2.1, 1.2], [], [0.2, 0.9]]` |
| `sign(x)` | `sign(Jet_eta)` | `[[1, -1, 1], [], [-1, 1]]` |
| `sqrt(x)` | `sqrt(MET_pt)` | `[5.612486, 9.219544, 4]` |
| `exp(x)`, `log(x)` (natural) | `log(MET_pt)` | `[3.4499876, 4.4426513, 2.7725887]` |
| `cos(x)`, `sin(x)` | `cos(MET_phi)` | `[0.87758255, -0.41614684, 0.5403023]` |
| `cosh(x)`, `sinh(x)` | `cosh(Jet_eta)` | `[[1.127626, 4.1443133, 1.8106556], [], [1.0200667, 1.4330864]]` |
| `hypot(x, y)` | `hypot(MET_pt, MET_pt)` | `[44.547726, 120.20815, 22.627417]` |
| `min(x, y)`, `max(x, y)` | `min(MET_pt, 50)` | `[31.5, 50, 16]` |
| `clip(x, low, high)` | `clip(MET_pt, 20, 50)` | `[31.5, 50, 20]` |
| `isfinite(x)` | `isfinite(MET_pt / 0)` | `[false, false, false]` |
| `if(condition, yes, no)` | `if(MET_pt > 30, MET_pt, 0)` | `[31.5, 85, 0]` |

Careful with `min` and `max`: they compare two values, member by member. To find the smallest
member *within* a list, use `least` (and `greatest` for the largest).

### Bits

| function | gives | example | result |
|---|---|---|---|
| `bit_and(x, mask)` | the bits `x` and `mask` share | `bit_and(Flags, 2)` | `[2, 0, 0]` |
| | whether bit 1 is set | `bit_and(Flags, 2) != 0` | `[true, false, false]` |
| `shift_right(x, n)` | `x` shifted right by `n` bits | `shift_right(Flags, 1)` | `[5, 2, 0]` |

### Angles and kinematics

| function | gives | example | result |
|---|---|---|---|
| `delta_phi(phi1, phi2)` | the difference in phi, wrapped into [-π, π] | `delta_phi(MET_phi, 3.0)` | `[2.7831852, -1, -2]` |
| `delta_r(eta1, phi1, eta2, phi2)` | ΔR = √(Δη² + Δφ²) | `delta_r(Jet_eta, Jet_phi, 0.0, 0.0)` | `[[0.50990194, 3.5805027, 1.56205], [], [3.0066593, 3.132092]]` |
| `mass(pt, eta, phi)`, `mass(pt, eta, phi, mass)` | the invariant mass of each event's list added together (massless without a mass) | `mass(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[229.10867, 0, 78.98824]` |

The next ones are for matching. For each member of one list, they find a member of another list
in the same event (the nearest, the farthest, …) and give you either the ΔR to it or its
position. They take `eta1, phi1, eta2, phi2`, plus an optional fifth value for a member with
nothing to match to (by default infinity for a ΔR and -1 for a position):

| function | example | result |
|---|---|---|
| `dr_nearest` | `dr_nearest(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[0.14142136, 1.9313208], [inf], []]` |
| `dr_second_nearest` | `dr_second_nearest(Muon_eta, Muon_phi, Jet_eta, Jet_phi, -1)` | `[[1.4422206, 3.2202485], [-1], []]` |
| `dr_farthest` | `dr_farthest(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[3.679674, 4.0422897], [inf], []]` |
| `nearest_index` | `nearest_index(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[0, 2], [-1], []]` |
| `farthest_index` | `farthest_index(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[1, 1], [-1], []]` |

`dr_second_farthest`, `second_nearest_index` and `second_farthest_index` work the same way.
Notice event 1: its muon has no jet to match to, so it gets infinity or -1.

### Pairs within a list

These go over every pair of members in each event's list, in the order (0, 1), (0, 2), …, (1, 2),
…, so event 0's three jets make three pairs.

| function | gives | example | result |
|---|---|---|---|
| `pair_first(list)` | the first member of each pair | `pair_first(Jet_pt)` | `[[65, 65, 22], [], [120]]` |
| `pair_second(list)` | the second member | `pair_second(Jet_pt)` | `[[22, 41, 41], [], [35]]` |
| `pair_dr(eta, phi)` | ΔR between the two | `pair_dr(Jet_eta, Jet_phi)` | `[[3.8209946, 1.3038405, 4.070574], [], [1.135867]]` |
| `pair_mass(pt, eta, phi)`, `pair_mass(pt, eta, phi, mass)` | the invariant mass of each pair | `pair_mass(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[[149.08528, 66.87832, 160.95856], [], [78.98824]]` |

### Text, branches and stored types

| function | gives | example | result |
|---|---|---|---|
| `contains(text, part)` | whether a text holds another; false when the sample lacks the field | `contains(sample.kind, 'sig')` | `true` |
| `branch(name)` | the branch of that name, often a name from the sample's metadata | `branch(sample.rho)`, with `rho = "MET_pt"` | `[31.5, 85, 16]` |
| `stored(x, kind)` | `x` converted to another type | `stored(nJet, 'u1')` | `[3, 0, 2]`, as `u1` |

The types are `f4`, `f8` (floats); `i1`, `i2`, `i4`, `i8` (signed whole numbers); `u1`, `u2`,
`u4`, `u8` (unsigned); and `b1` (true or false). A comparison gives `b1`; `count` of a list gives `i8`;
`sum` of whole numbers gives `i8`, and of floats the float type it was given.

## Helpers

When you find yourself writing the same thing twice, make it a helper. It's like a small function:
give it a name, an expression and, if it needs them, the names of its inputs, then call it
anywhere in the recipe:

```toml
[helper.jets_above]
takes = ["threshold"]
value = "count(Jet where pt > threshold)"

[helper.hem_veto]
about = "no jet in the HEM 15/16 hole"
pass = "count(Jet where eta < -1.3 and phi > 2.5) == 0"
```

| expression | result |
|---|---|
| `jets_above(40)` | `[2, 0, 1]` |
| `jets_above(100)` | `[0, 0, 1]` |
| `hem_veto()` | `[false, true, true]` |

## Mistakes

If something in the recipe doesn't make sense, `rvr press` stops before it reads anything and
tells you what's wrong, in plain words:

| expression | `rvr` says |
|---|---|
| `Jet_pt >` | Expected a value at the end (column 9) |
| `count(Jet_pt, 3)` | count takes 1 value, not 2 |
| `Jet where nJet > 1` | The condition after where says nothing about each Jet |
| `MET` | This expression needs values, but MET is a collection; name one of its attributes, such as MET.pt |
| `select = "…"` at the top of the recipe | A recipe has no setting named select; it takes format, version, title, collection, object, helper, keep, copy, column |

## From Python to rvr

If you already make your selections in Python, here are some of the usual ones next to how you'd
write them in a recipe, starting simple and getting less simple as we go. The Python side is
Awkward, with the branches read the way uproot reads NanoAOD (`ev = tree.arrays()`, so `ev.Jet_pt`
and so on). Both sides were run on the example events above and give the result shown; a test
runs them again every time, so if one ever stopped matching we'd know.

A few things you'll notice: Awkward's `&` becomes `and` (and you don't need the parentheses
around each condition), `axis=1` goes away because a function on a jagged list always works per
event, and where Awkward gives you `None` for an empty list, an rvr function asks for the value
you want instead.

| Python (Awkward) | rvr | result |
|---|---|---|
| `ev.MET_pt * 2` | `MET_pt * 2` | `[63, 170, 32]` |
| `ev.MET_pt > 30` | `MET_pt > 30` | `[true, true, false]` |
| `ak.num(ev.Jet_pt)` | `count(Jet)` | `[3, 0, 2]` |
| `ak.sum(ev.Jet_pt > 30, axis=1)` | `count(Jet_pt > 30)` | `[2, 0, 2]` |
| `ak.sum(ev.Jet_pt, axis=1)` | `sum(Jet_pt)` | `[128, 0, 155]` |
| `ev.Jet_pt[(ev.Jet_pt > 30) & (abs(ev.Jet_eta) < 2.4)]` | `Jet_pt[Jet_pt > 30 and abs(Jet_eta) < 2.4]` | `[[65, 41], [], [120, 35]]` |
| `ak.sum(ev.Jet_pt[(ev.Jet_pt > 30) & (abs(ev.Jet_eta) < 2.4)], axis=1)` | `sum(Jet_pt[Jet_pt > 30 and abs(Jet_eta) < 2.4])` | `[106, 0, 155]` |
| `ak.fill_none(ak.max(ev.Jet_pt, axis=1), -1)` | `greatest(Jet_pt, -1)` | `[65, -1, 120]` |
| `ak.fill_none(ak.pad_none(ak.sort(ev.Jet_pt, ascending=False), 2)[:, 1], -1)` | `nth(sorted(Jet_pt, 'descending'), 1, -1)` | `[41, -1, 35]` |
| `(ak.sum((ev.Jet_pt > 30) & (abs(ev.Jet_eta) < 2.4), axis=1) >= 2) & (ev.MET_pt > 30)` | `count(Jet where pt > 30 and abs(eta) < 2.4) >= 2 and MET_pt > 30` | `[true, false, false]` |
| `ak.sum((ev.Muon_pt > 20) & ev.Muon_tightId & (ev.Muon_charge < 0), axis=1)` | `count(Muon where pt > 20 and tightId and charge < 0)` | `[0, 1, 0]` |
| `ev.Jet_eta[ak.argsort(ev.Jet_pt, ascending=False)]` | `take(Jet_eta, order(Jet_pt, 'descending'), -9)` | `[[0.5, 1.2, -2.1], [], [-0.2, 0.9]]` |

### Longer ones

Here the Python takes a few lines, and the recipe still takes one. Each is written as it would sit
in `[column]`.

**Jets that are not near any muon** (ΔR > 0.4), the usual cleaning step. A jet with no muon
around has an infinite ΔR to the nearest one, so it's kept:

<table>
<tr><th>Python (Awkward)</th><th>rvr</th></tr>
<tr><td>

```python
j_eta, m_eta = ak.unzip(ak.cartesian([ev.Jet_eta, ev.Muon_eta], nested=True))
j_phi, m_phi = ak.unzip(ak.cartesian([ev.Jet_phi, ev.Muon_phi], nested=True))
dphi = (j_phi - m_phi + np.pi) % (2 * np.pi) - np.pi
dr = np.sqrt((j_eta - m_eta) ** 2 + dphi ** 2)
ev.Jet_pt[ak.all(dr > 0.4, axis=2)]
```

</td><td>

```toml
clean_jet_pt = "Jet_pt[dr_nearest(Jet_eta, Jet_phi, Muon_eta, Muon_phi) > 0.4]"
```

</td></tr>
</table>

Both give `[[22, 41], [], [120, 35]]`: event 0's first jet sits right on a muon, so it goes.

**The transverse mass of the leading muon and the MET**, with 0 where there is no muon:

<table>
<tr><th>Python (Awkward)</th><th>rvr</th></tr>
<tr><td>

```python
mu_pt = ak.fill_none(ak.firsts(ev.Muon_pt), 0)
mu_phi = ak.fill_none(ak.firsts(ev.Muon_phi), 0)
np.sqrt(2 * mu_pt * ev.MET_pt * (1 - np.cos(mu_phi - ev.MET_phi)))
```

</td><td>

```toml
mt = "sqrt(2 * first(Muon_pt, 0) * MET_pt * (1 - cos(first(Muon_phi, 0) - MET_phi)))"
```

</td></tr>
</table>

Both give `[25.5315, 22.80952, 0]`.

**The largest mass of any two jets**, with -1 for an event with fewer than two:

<table>
<tr><th>Python (Awkward)</th><th>rvr</th></tr>
<tr><td>

```python
jet = ak.zip({"pt": ev.Jet_pt, "eta": ev.Jet_eta, "phi": ev.Jet_phi, "m": ev.Jet_mass})
px, py = jet.pt * np.cos(jet.phi), jet.pt * np.sin(jet.phi)
pz, e = jet.pt * np.sinh(jet.eta), np.sqrt((jet.pt * np.cosh(jet.eta)) ** 2 + jet.m ** 2)
a, b = ak.unzip(ak.combinations(ak.zip({"px": px, "py": py, "pz": pz, "e": e}), 2))
m = np.sqrt((a.e + b.e) ** 2 - (a.px + b.px) ** 2 - (a.py + b.py) ** 2 - (a.pz + b.pz) ** 2)
ak.fill_none(ak.max(m, axis=1), -1)
```

</td><td>

```toml
max_mjj = "greatest(pair_mass(Jet_pt, Jet_eta, Jet_phi, Jet_mass), -1)"
```

</td></tr>
</table>

Both give `[160.95856, -1, 78.98824]`.

## Three ways to pick the jets you want

Say you only want the jets with pt above 30 GeV and |η| below 2.4. There are three ways to write
that, and all three pick exactly the same jets. Which one you use depends on how often you need
those jets.

**1. An object, with bare names**

```toml
[object.good_jet]
from = "Jet"
where = "pt > 30 and abs(eta) < 2.4"
```

`from = "Jet"` says which collection you're picking from, and it's what makes `good_jet` a kind of
jet: afterwards `good_jet.eta` is the good jets' `Jet_eta`, `good_jet.pt` their `Jet_pt`, and
`count(good_jet)` counts them. Inside `where`, a bare name like `pt` is the jet's own branch,
`Jet_pt`.

**2. An object, with full branch names**

```toml
[object.good_jet]
from = "Jet"
where = "Jet_pt > 30 and abs(Jet_eta) < 2.4"
```

The same jets again. Some people find this easier to read, because you can see exactly which branch
each cut is on; use whichever you like (you can even mix the two). You still need `from`, though:
the condition says which jets pass, but `from` is what says `good_jet` *is* jets.

**3. No object at all: a mask**

```toml
[column]
good_jet_eta = "Jet_eta[Jet_pt > 30 and abs(Jet_eta) < 2.4]"
```

The brackets keep the members of `Jet_eta` where the condition is true. No object and no `from`,
but the condition now lives in this one line only. If you also want the good jets' pt, or how many
there are, you have to write it out again, and if you ever change the cut you have to change it
everywhere you wrote it.

**So which one?** Use an object (1 or 2) for anything you'll use more than once: good jets, good
muons, good electrons. Then the cut lives in one place, and every cut, column and count that says
`good_jet` means the same jets. A mask (3) is fine for a one-off. On the example events:

| expression | written as | result |
|---|---|---|
| `good_jet.eta` | 1 or 2 | `[[0.5, 1.2], [], [-0.2, 0.9]]` |
| `Jet_eta[Jet_pt > 30 and abs(Jet_eta) < 2.4]` | 3 | `[[0.5, 1.2], [], [-0.2, 0.9]]` |
| `count(good_jet)` | 1 or 2 | `[2, 0, 2]` |
| `count(Jet where Jet_pt > 30 and abs(Jet_eta) < 2.4)` | the same count, with no object | `[2, 0, 2]` |

### More than one collection

An object picks from one collection, but you can have as many objects as you like and use them
together anywhere. Here `good_muon` is a second object: `from = "Muon"`, `where = "tightId and pt > 20"`.

| expression | gives | result |
|---|---|---|
| `count(good_jet) + count(good_muon)` | the good jets and good muons of each event, together | `[3, 1, 2]` |
| `good_jet.pt[dr_nearest(good_jet.eta, good_jet.phi, good_muon.eta, good_muon.phi) > 0.4]` | the good jets that aren't within ΔR 0.4 of a good muon | `[[41], [], [120, 35]]` |

Event 0 shows the cleaning at work: its first jet, at η 0.5 and φ 0.1, sits 0.14 away from the good
muon at η 0.4 and φ 0.2, so it goes.

You can also do the cleaning inside the object itself, by naming the other collection in its
`where`:

```toml
[object.clean_jet]
from = "Jet"
where = "pt > 30 and dr_nearest(eta, phi, good_muon.eta, good_muon.phi) > 0.4"
```

This is where `from` matters most. The condition names both jets and muons, so without `from`
there would be no way to tell whether `clean_jet` is jets or muons. Inside the `where`, the bare
names (`pt`, `eta`, `phi`) are the jet's own, and anything else is written in full
(`good_muon.eta`, or `Muon_eta`).

| expression | gives | result |
|---|---|---|
| `count(Jet where pt > 30 and dr_nearest(eta, phi, good_muon.eta, good_muon.phi) > 0.4)` | how many `clean_jet` each event has | `[1, 0, 2]` |
