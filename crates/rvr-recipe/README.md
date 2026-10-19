# The recipe language

A `recipe.toml` says which events `rvr press` keeps and what it stores of them. Every value in it
is one **expression**: a short formula over the event's branches, like `MET_pt > 30` or
`count(good_muon) >= 1`. This page lists everything a recipe can say, each with an example and
what it gives.

Expressions are not Python. `rvr` parses them when it reads the recipe, so a mistake is reported
before any file is opened, and evaluates them over whole columns of 50,000 events at a time, so
nothing loops over events one by one. Their numbers follow NumPy's rules, so a selection keeps
the same events the same formula would in NumPy or Awkward. Only the branches an expression
names are read from the file.

## The example events

Every result on this page comes from evaluating the expression with `rvr`'s recipe code on
these three events. This page calls a branch with one value per event, such as `MET_pt`, **flat**,
and a branch with a list of values per event, such as `Jet_pt` (one entry per jet), **jagged**: the
lists have different lengths. A collection such as `Jet` is the jagged branches that share a
name and a length per event (`Jet_pt`, `Jet_eta`, …).

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

The sample being read has the metadata `era = 3`, `kind = "signal"` and `xsec = 0.386` (from its
entry in `samples.toml`).

Results are written one per event: `[31.5, 85, 16]` is a flat result, one value for each of the
three events, and `[[65, 41], [], [120, 35]]` a jagged one, a list for each.

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
| `[object.NAME]` | `from = "COLLECTION"` and `where = "CONDITION"`: the members of the collection that pass the condition, under a new name. Inside `where`, bare names are the collection's attributes (`pt` is `Jet_pt`). |
| `[keep]` | the cuts, in order. Each is `name = "condition"`, true or false per event. An event is kept when every cut is true; the counts after each cut are the cutflow `rvr press` prints. |
| `[copy]` | `branches = [...]`: branches stored as they are, with their own types. `NAME = "BRANCH"` stores a branch under another name, and `NAME = "branch(sample.FIELD)"` a branch that each sample names in its metadata. |
| `[column]` | `NAME = "expression"`: a computed column, flat or jagged. `NAME = { value = "expression", stored = "f4" }` also sets the type it is stored as. |
| `[helper.NAME]` | a named expression to reuse: `value = "…"` (or `pass = "…"` for a condition), with `takes = ["a", "b"]` when it takes values, and an optional `about` note. |
| `[collection.NAME]` | a collection made of branches with any names: `pt = "PT_lep"`, `eta = "Eta_lep"`. |

What is stored is only what `[copy]` and `[column]` name. Objects and helpers are working names;
a cut on `good_jet` keeps or drops events, but `Jet_pt` in `[copy]` still holds every jet of each
kept event. To store only the good jets, add a column: `good_jet_pt = "good_jet.pt"`.

### Cuts for some samples only

A cut written as a table applies only where its `if` is true, and lets every other sample's
events through:

```toml
[keep]
hem = { if = "sample.era == 3", apply = "hem_veto()" }
```

A sample whose metadata has no `era` is not cut either.

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

Every comparison with a missing field is false (`!=` too), and arithmetic with it is an error.

Values written into an expression are numbers (`30`, `2.5`, `1e3`, `1_000`), text in single or
double quotes (`'signal'`), and `true` or `false`.

## Operators

From loosest to tightest binding: `where`; `or`; `and`; `not`; comparisons and `in`; `+ -`;
`* / %`; unary `-`; `**`; then `.name`, `[...]` and calls. Parentheses group as usual. A flat value
and a jagged one combine member by member: `Jet_pt - MET_pt` subtracts each event's MET from each
of its jets.

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
| `count(lep where pt > 20)` | a `[collection]` (`pt = "PT_lep"`) works the same | `[1, 1, 0]` |

A collection or object is not a value on its own: use one of its attributes (`good_jet.pt`) or
count it (`count(good_jet)`). One `where` takes one condition; join conditions with `and`.

## Functions

Where a function takes `otherwise`, that value is given for an event whose list is empty (event
1 has no jets).

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

The leading good jet's pt is `nth(good_jet.pt, 0, 0)`: `[65, 0, 120]`.

### Positions and order

| function | gives | example | result |
|---|---|---|---|
| `index(list)` | each member's position | `index(Jet_pt)` | `[[0, 1, 2], [], [0, 1]]` |
| `sorted(list)` | the members in ascending order | `sorted(Jet_pt)` | `[[22, 41, 65], [], [35, 120]]` |
| `sorted(list, 'descending')` | in descending order | `sorted(Jet_pt, 'descending')` | `[[65, 41, 22], [], [120, 35]]` |
| `order(list)`, `order(list, 'descending')` | the positions that would sort the list | `order(Jet_pt, 'descending')` | `[[0, 2, 1], [], [0, 1]]` |
| `take(list, positions, otherwise)` | the members at those positions; `otherwise` where a position is -1 or past the end | `take(Jet_eta, order(Jet_pt, 'descending'), -9)` | `[[0.5, 1.2, -2.1], [], [-0.2, 0.9]]` |
| `listed(list, positions)` | for each member, whether its position is among `positions` | `listed(Jet_pt, nearest_index(Muon_eta, Muon_phi, Jet_eta, Jet_phi))` | `[[true, false, true], [], [false, false]]` |

The `take` example gives the jets' eta in order of descending pt; the `listed` example marks the
jets that are the nearest jet to some muon.

### Math, value by value

These work on flat and jagged values alike, and keep the float type they are given.

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

`min` and `max` compare two values; the smallest member of one list is `least`.

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

For each member of one list, its ΔR to a member of another list of the same event, or that
member's position. They take `eta1, phi1, eta2, phi2`, and an optional fifth value for a member
with no such partner (otherwise infinity for a ΔR, -1 for a position):

| function | example | result |
|---|---|---|
| `dr_nearest` | `dr_nearest(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[0.14142136, 1.9313208], [inf], []]` |
| `dr_second_nearest` | `dr_second_nearest(Muon_eta, Muon_phi, Jet_eta, Jet_phi, -1)` | `[[1.4422206, 3.2202485], [-1], []]` |
| `dr_farthest` | `dr_farthest(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[3.679674, 4.0422897], [inf], []]` |
| `nearest_index` | `nearest_index(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[0, 2], [-1], []]` |
| `farthest_index` | `farthest_index(Muon_eta, Muon_phi, Jet_eta, Jet_phi)` | `[[1, 1], [-1], []]` |

`dr_second_farthest`, `second_nearest_index` and `second_farthest_index` work the same way.
Event 1's muon has no jet to measure against, so it gets infinity or -1.

### Pairs within a list

Each pair of members of an event's list, in the order (0, 1), (0, 2), …, (1, 2), …: event 0's
three jets make three pairs.

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

A mistake stops `rvr press` before anything is read, with a sentence saying what is wrong:

| expression | `rvr` says |
|---|---|
| `Jet_pt >` | Expected a value at the end (column 10) |
| `count(Jet_pt, 3)` | count takes 1 value, not 2 |
| `Jet where nJet > 1` | The condition after where says nothing about each Jet |
| `MET` | This expression needs values, but MET is a collection; name one of its attributes, such as MET.pt |
| `select = "…"` at the top of the recipe | A recipe has no setting named select; it takes format, version, title, collection, object, helper, keep, copy, column |
