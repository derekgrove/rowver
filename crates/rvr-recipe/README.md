# Recipe language

Your `recipe.toml` tells `rvr press` two things: which events to keep, and what to store for each
one. You will write **expressions**, a short formula over the event's branches,
like `MET_pt > 30` or `count(good_muon) >= 1`. This page is the lookup for all of it: every word,
operator and function you can use, each with an example and what it actually gives you.

A quick note before we start: expressions look a lot like Python, but they aren't Python. `rvr`
reads every one of them before it opens a single file, so a typo, a function that does not exist
or a wrong number of values shows up right away, every one of them at once, and not an hour into a
press. It then works on whole columns of events at a time (there is no loop over events, and no
result depends on how many are worked on at once). The kinds of number follow NumPy's type
promotion, with the differences [stated below](#kinds-exactly), and every result is defined here to
the bit, the same on every platform: [Kinds, exactly](#kinds-exactly) and
[Functions, exactly](#functions-exactly) are the rules a second implementation would follow. Only the
branches you mention are read from the file.

If you already write your selections in Python with Awkward, [a later section](#from-python-to-rvr)
puts some common ones side by side with how you'd write them here.

## The example events

Every result on this page is what `rvr` really gives on the three made-up events below, and a
test checks each one, so they can't quietly go out of date. Two words we'll use a lot: a **flat**
branch has one value per event, like `MET_pt`, and a **jagged** branch has a list per event, like
`Jet_pt` with one entry per jet, so the lists have different lengths. A branch is always written
with its full name, as the file spells it: `Jet_pt`, `lep_pt` in an ATLAS ntuple, or `H1_PX` in an
LHCb one.

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
| `Electron_pt` | `[]` | `[30]` | `[50, 9]` |
| `Electron_eta` | `[]` | `[1.1]` | `[-0.6, 2.0]` |
| `Electron_phi` | `[]` | `[-1.2]` | `[0.4, -2.2]` |
| `Electron_charge` | `[]` | `[1]` | `[-1, -1]` |
| `MET_pt` | `31.5` | `85` | `16` |
| `MET_phi` | `-0.5` | `2.0` | `1.0` |
| `Flags` | `11` (binary 1011) | `4` (0100) | `1` (0001) |

`Muon_tightId` holds true-or-false values (`b1`), `nJet` unsigned 32-bit whole numbers (`u4`),
`Muon_charge`, `Electron_charge` and `Flags` signed ones (`i4`), and every other branch 32-bit
floats (`f4`), as NanoAOD stores them. The sample these events come from has `era = 3`,
`kind = "signal"` and `xsec = 0.386` in its `samples.toml` entry.

Results are written one value per event: `[31.5, 85, 16]` is a flat result (a number for each of
the three events), and `[[65, 41], [], [120, 35]]` is a jagged one (a list for each).

## The parts of a recipe

```toml
format = "rvr-recipe"
version = 1
title = "my study"                  # optional

[define]                            # names for expressions, used by the lines below them
good_jet = "Jet_pt > 30 and abs(Jet_eta) < 2.4"

[keep]                              # the cuts: an event is kept when it passes every one
two_jets = "count(good_jet) >= 2"

[columns]                           # what the file stores
branches = ["MET_pt", "Jet_pt", "genWeight"]   # branches stored as they are
ht = "sum(Jet_pt[good_jet])"        # a column computed while pressing

[totals]                            # sums over every event read, before the cuts, per sample
sumw = "genWeight"

[derived]                           # columns kept as expressions, worked out when read
met_over_ht = "MET_pt / ht"
weight = "genWeight / sample.sumw"  # a total, read as sample.NAME
```

| table | what it holds |
|---|---|
| `[define]` | `NAME = "expression"`: a name for an expression, which the lines below it can use, like `good_jet` above: the jets of each event that pass, as a true-or-false list beside `Jet_pt`. A name may use the names defined above it, and may not be the name of a branch. Nothing here is stored. |
| `[keep]` | Your cuts, in order. Each one is `name = "condition"`, true or false for each event, and an event is kept only if all of them are true. `rvr press` prints how many events are left after each cut, so this is your cutflow too. |
| `[columns]` | What the file stores. `branches = [...]` stores branches just as the ROOT file has them, types and all. `NAME = "expression"` stores a column you compute, flat or jagged; a branch's name alone (`npv = "PV_npvs"`) stores that branch under a new name, and `NAME = "branch(sample.FIELD)"` a branch whose name each sample gives in its metadata. Write `NAME = { value = "expression", stored = "f4" }` to choose the type it's stored as. |
| `[totals]` | `NAME = "expression"`: the expression summed over every event read, before the cuts, one number for each sample, such as `sumw = "genWeight"` to normalize simulation, which needs the sum over every event, not only those kept. `NAME = { value = "expression", tree = "Runs" }` sums over every entry of another tree of each file instead (NanoAOD's `genEventSumw`, for a skim whose events were filtered before), for a sample that reads its files whole. A total is stored as a sample column, and only `[derived]` can use it, as `sample.NAME`: it is known once every event is read. |
| `[derived]` | `NAME = "expression"`: a column the file keeps as its expression, not as an array, worked out when it is read. It uses only what the file holds: the columns under `[columns]`, the derived columns above it, and `sample.KEY`; `branch(sample.KEY)`, a column that each sample names, is refused there (name the column). A stored column costs space in the file; a derived one costs a moment each time it is worked out. |

### The keys, exactly

A recipe holds these keys and no others; any other, at the top or in a table, is refused, with the
known key nearest to it when there is one ("Did you mean: keep?"):

| where | keys |
|---|---|
| the top | `format = "rvr-recipe"` and `version = 1`, both required; `title`, text, optional; the five tables |
| `[define]` | `NAME = "expression"`, as many as wanted |
| `[keep]` | `NAME = "condition"`, or `NAME = { if = "condition", apply = "condition" }`: `apply` required, `if` optional, no other key |
| `[columns]` | `branches = ["…", …]`, each a branch's name as the file spells it (nonempty, not a defined name); `NAME = "expression"`, or `NAME = { value = "expression", stored = "KIND" }`: `value` required, `stored` optional (one of the [kinds](#kinds-exactly)) |
| `[totals]` | `NAME = "expression"`, or `NAME = { value = "expression", tree = "TREE" }`: `value` required, `tree` optional (a tree's name, nonempty) |
| `[derived]` | `NAME = "expression"` |

Every `NAME` is a **plain name**: a letter or `_`, then letters, digits and `_`, and not one of the
words `and`, `or`, `not`, `in`, `true`, `false`, `pi`, `inf`. A defined name is not `sample`; a
column, total or derived column is not `sample`, `entry` or `file`, and a total is not
`events_read`, `events_total` or `events_missing`. No two columns share a name (a branch listed
twice is two), and a total or derived column has a name no column, defined name, total or earlier
derived column has. A defined name uses only the names defined above it; a derived column uses only
the stored columns and the derived columns above it. Every expression is written as text in quotes.

`rvr` reads the whole recipe before it opens a file, and refuses it with every problem it finds,
one a line, each naming where it is (`[columns] ht: sum takes 1 value, not 2`). Two checks need the
files' branches, and are made against the branch lists of `samples.lock` before anything is read: a
defined name is never the name of a branch of any sample, so that each name means one thing; and
each column has one kind in every sample: `if(sample.is_data, 1, genWeight)` is `i8` for data and
`f4` for simulation, and is refused; store it as one kind, as
`weight = { value = "if(sample.is_data, 1, genWeight)", stored = "f4" }`.

A total is, for each file, its expression's value at each entry the press chose, before any cut,
as an `f8`, added in entry order from +0.0; a sample's total is its files' added from +0.0 in the
order `samples.lock` lists them. A total of `b1` values counts the true ones.

Every recipe writes all five tables, even the ones it leaves empty, so that it says at a glance
what it does and does not use: an empty `[define]` defines nothing, an empty `[keep]` keeps every
event, an empty `[totals]` sums nothing, and an empty `[derived]` adds nothing; `[columns]` stores
at least one column. A recipe that leaves a table out is refused, with the line to add, and so is
one whose `[columns]` stores nothing; at a terminal, `rvr press` and `rvr try` first ask whether to
write the missing tables in, empty and in their places, and go on (unless `[columns]` stores
nothing, which only the recipe can fix).

Only what `[columns]` names ends up in the file; a defined name is a working name. A cut on
`good_jet` decides which *events* you keep, but `Jet_pt` in `branches` still stores every jet of
those events. If you want only the good jets, store them as a column: `good_jet_pt =
"Jet_pt[good_jet]"`.

### Cuts for some samples only

Sometimes a cut only makes sense for some samples, like a veto for one era. Write the cut as a
table with an `if`, and it applies only to the samples where the `if` is true; every other
sample's events go straight through:

```toml
[keep]
hem = { if = "sample.era == 3", apply = "hem_veto" }
```

A sample that has no `era` at all isn't cut either: a comparison with a field the sample doesn't
have is false. An `if` that is a field alone, as `if = "sample.vbf"`, does not apply to a sample
without that field. An `if` may also be one true-or-false value per event, as
`if = "MET_pt > 100"`: the cut then applies to those events only.

## Values

| expression | gives | result |
|---|---|---|
| `MET_pt` | a flat branch: one value per event | `[31.5, 85, 16]` |
| `Jet_pt` | a jagged branch: a list per event, one entry per jet | `[[65, 22, 41], [], [120, 35]]` |
| `Muon_charge` | another, with whole numbers | `[[1, -1], [-1], []]` |
| `good_jet` | a defined name: here a true-or-false list beside `Jet_pt` | `[[true, false, true], [], [true, true]]` |
| `sample.era` | a field of the sample's metadata: one value, the same for every event | `3` |
| `sample.kind` | a text field | `"signal"` |
| `sample.missing` | a field this sample's metadata does not have | missing |

A field the sample doesn't have is **missing**, and has one rule: any comparison with it is false,
`!=` included, against a fixed value or a column alike, and anything else with it is refused.

| a missing field used | gives |
|---|---|
| in a comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`), or `in` | false, value by value: `MET_pt > sample.cut` is false for every event |
| in `contains` | false |
| in arithmetic, `-`, `and`, `or`, `not`, or a function that takes a number | refused, saying which sample lacks it |
| as the condition of `if(…)` or of a cut | refused: `if(sample.isData, …)` needs every sample to have `isData` (give it to all of them in `[defaults]` of `samples.toml`), while `if(sample.isData == true, …)` takes a sample without it as false |
| as a cut's own `if` | the cut does not apply to that sample |

A field's value is what `samples.toml` writes: a whole number (`xsec = 2`) is whole, a number with
a `.` or an exponent (`xsec = 2.0`) a float, `true` or `false` a truth value, and text text; a list
or table is missing. So `xsec = 2` and `xsec = 2.0` can give a column two kinds. `sample.file` is
the path of the row's file, as `samples.lock` names it; `events_read`, `events_total` and
`events_missing`, which a press records of each sample, are no expression's to read.

Written into an expression:

| you write | it is |
|---|---|
| `30`, `1_000` | a whole number: digits, up to 9223372036854775807 |
| `2.5`, `.5`, `5.`, `1e3`, `1.5E-3` | a float: digits with a `.` or an exponent (`e` or `E`, an optional sign, then digits), read as the nearest double |
| `'signal'`, `"signal"` | text, which holds no quote of the kind around it (there are no escapes) |
| `true`, `false` | the two truth values |
| `pi`, `inf` | π and infinity, as doubles (NaN has no name: `inf - inf` is one) |
| `Jet_pt`, `good_jet` | a name: a letter or `_`, then letters, digits and `_` (any alphabet), a branch or a defined name; `and`, `or`, `not`, `in`, `true`, `false`, `pi` and `inf` are words of the language, so a branch named so is read as `branch('in')` |

In any number, whole or float, a `_` after its first character is ignored (`1_000`, `0.000_1`).
There is no unary `+`, and no hexadecimal. Spaces of any kind may stand between the parts.

## Operators

These work the way you'd expect. From loosest to tightest binding they are: `or`; `and`; `not`;
the comparisons and `in`; `+ -`; `* / %`; unary `-`; `**`; and then `sample.name`, `[...]` and
function calls. Each level works left to right, but `**` right to left (`2 ** 3 ** 2` is `2 ** 9`).
When in doubt, use parentheses. When you combine a flat value with a jagged one, the flat value is
used for every member: `Jet_pt - MET_pt` subtracts the event's MET from each of its jets.

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

The comparisons are `==`, `!=`, `<`, `<=`, `>` and `>=`; one compares two values, and they do not
chain (`a < b < c` is refused: write `a < b and b < c`). `x in [a, b, …]` is
`(x == a) or (x == b) or …`, and `x in []` is false; a list, like a call's values, may end with a
comma. `not`, `and` and `or` take only true-or-false values: a number is refused (write `x != 0`).
`and` whose left side is a fixed `false` (written, a sample's field, or worked out from fixed
values), or `or` whose left side is a fixed `true`, gives it without working out its right side. `not` comes at the start of what it turns over, never straight after an operator
(`a == not b` is refused: write `a == (not b)`).

Text compares only with `==` and `!=`: two texts are equal when they are the same characters, and
text against a number or a truth value is never equal. Any other operator refuses text.

`-x` turns a float's sign over (`-0.0` from `0.0`), and takes a whole number from 0 in its own kind,
wrapping as every whole-number operation does (`-nJet` of a `u4` 3 is 4294967293); a true-or-false
value is refused. Arithmetic on two true-or-false values is refused (use `and`, `or` or `count`); a
true-or-false value beside a number takes the number's kind, true being 1.

## Choosing members of a list

This is where `[...]` comes in: a true-or-false list of the same shape, a **mask**, picks out the
members of a list you care about, event by event.

| expression | gives | result |
|---|---|---|
| `Jet_pt[Jet_pt > 30]` | the members a true-or-false list of the same shape marks | `[[65, 41], [], [120, 35]]` |
| `Jet_pt[Jet_pt > 30 and Jet_eta > 0]` | the same, with a combined condition | `[[65, 41], [], [35]]` |
| `Muon_pt[Muon_tightId]` | marked by a true-or-false branch | `[[44], [25], []]` |
| `count(Jet_pt > 30 and abs(Jet_eta) < 2.4)` | how many members pass | `[2, 0, 2]` |
| `Jet_pt[good_jet]` | a defined mask (`good_jet = "Jet_pt > 30 and abs(Jet_eta) < 2.4"`) | `[[65, 41], [], [120, 35]]` |
| `Jet_eta[good_jet]` | the same jets, another branch | `[[0.5, 1.2], [], [-0.2, 0.9]]` |
| `count(good_jet)` | how many members passed | `[2, 0, 2]` |
| `Muon_pt[good_muon]` | `good_muon = "Muon_tightId and Muon_pt > 20"` | `[[44], [25], []]` |
| `count(good_muon) >= 1` | a cut on it | `[true, true, false]` |

One mask picks the same members of every list it is the same shape as: `good_jet` works for
`Jet_pt`, `Jet_eta`, `Jet_phi` and every other jet branch. A mask is a true-or-false list of the
same length as the list it picks from, event by event: one of another length in an event stops the
press there, naming the event (a [data error](#values-the-recipe-cannot-take)). That is all rvr can
check: in an event with as many muons as jets, a muon mask would pick jets without complaint, so
pick each particle's branches with a mask made from that particle's branches.

## Functions

Some functions take an `otherwise`: that's what you get back where there is nothing to give, as for
an event whose list is empty. It is a fixed value: a number or truth written in, or a sample's
field. Event 1 has no jets, so keep an eye on it in the examples. Every function's exact
definition, its kinds, its order of work and what it does with NaN, is in
[Functions, exactly](#functions-exactly).

### Reducing a jagged list to a flat value

| function | gives | example | result |
|---|---|---|---|
| `count(list)` | how many members; of a true-or-false list, how many are true | `count(Jet_pt > 30)` | `[2, 0, 2]` |
| | of a list of numbers, how many there are | `count(Jet_pt)` | `[3, 0, 2]` |
| `sum(list)` | the sum (0 for an empty list) | `sum(Jet_pt)` | `[128, 0, 155]` |
| `any(list)` | whether any member is true | `any(Jet_pt > 100)` | `[false, false, true]` |
| `all(list)` | whether every member is true (true for an empty list) | `all(Jet_pt > 30)` | `[false, true, true]` |
| `least(list, otherwise)` | the smallest member | `least(Jet_pt, -1)` | `[22, -1, 35]` |
| `greatest(list, otherwise)` | the largest member | `greatest(Jet_pt, -1)` | `[65, -1, 120]` |
| `least_index(list, otherwise)` | the position of the smallest, counting from 0 | `least_index(Jet_pt, -1)` | `[1, -1, 1]` |
| `greatest_index(list, otherwise)` | the position of the largest | `greatest_index(Jet_pt, -1)` | `[0, -1, 0]` |
| `first(list, otherwise)` | the first member | `first(Jet_pt, -1)` | `[65, -1, 120]` |
| `nth(list, n, otherwise)` | the member at position `n`, counting from 0 | `nth(Jet_pt, 1, -1)` | `[22, -1, 35]` |

For example, the leading good jet's pt is `nth(Jet_pt[good_jet], 0, 0)`, which gives `[65, 0, 120]`.

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

### Joining lists

`join` lays each event's lists one after another, all of the first list, then all of the second,
and so on: the way to make one list of leptons from electrons and muons. The lists may hold
different kinds of number, and the result is the kind they combine in, as with `+`.

| function | gives | example | result |
|---|---|---|---|
| `join(list, list, …)` | each event's lists, one after another | `join(Electron_pt, Muon_pt)` | `[[44, 12], [30, 25], [50, 9]]` |
| | the same leptons' charges | `join(Electron_charge, Muon_charge)` | `[[1, -1], [1, -1], [-1, -1]]` |
| | the leading lepton's pt | `greatest(join(Electron_pt, Muon_pt), -1)` | `[44, 30, 50]` |
| | the mass of all the leptons together | `mass(join(Electron_pt, Muon_pt), join(Electron_eta, Muon_eta), join(Electron_phi, Muon_phi))` | `[59.41024, 67.69384, 82.84507]` |
| | which lepton pairs have opposite charges | `pair_first(join(Electron_charge, Muon_charge)) != pair_second(join(Electron_charge, Muon_charge))` | `[[true], [true], [false]]` |

Join every branch of the leptons in the same order and they stay lined up, member by member. In
a recipe, name them once under `[define]` (`lep_pt = "join(Electron_pt, Muon_pt)"`, and the same
for `lep_eta`, `lep_phi` and `lep_charge`), and use the names after. To list them by pt, take each
in `order(lep_pt, 'descending')`.

### Math, value by value

These work on flat and jagged values alike, value by value. A float keeps its kind, and a whole
number gives a float (`f4` or `f8`, as [Functions, exactly](#functions-exactly) says). Each gives
the same bits on every platform: `sqrt` and the remainder of `%` are exact in IEEE arithmetic, and
the rest that a math library computes (logarithms, trigonometry, `hypot`, `**` and the others) come
from the `libm` crate, at exactly version 0.2.16, not from the platform's own, which differs from
one system to another in the last bit.

| function | example | result |
|---|---|---|
| `abs(x)` | `abs(Jet_eta)` | `[[0.5, 2.1, 1.2], [], [0.2, 0.9]]` |
| `sign(x)` | `sign(Jet_eta)` | `[[1, -1, 1], [], [-1, 1]]` |
| `sqrt(x)` | `sqrt(MET_pt)` | `[5.612486, 9.219544, 4]` |
| `exp(x)`, `log(x)` (natural) | `log(MET_pt)` | `[3.4499876, 4.4426513, 2.7725887]` |
| `log10(x)` | `log10(MET_pt)` | `[1.4983106, 1.9294189, 1.20412]` |
| `cos(x)`, `sin(x)`, `tan(x)` | `cos(MET_phi)` | `[0.87758255, -0.41614684, 0.5403023]` |
| `acos(x)`, `asin(x)`, `atan(x)` | `acos(cos(MET_phi))` | `[0.5, 2, 1]` |
| | `atan(MET_phi)` | `[-0.4636476, 1.1071488, 0.7853982]` |
| `atan2(y, x)`, the angle of the point (x, y) | `atan2(MET_pt * sin(MET_phi), MET_pt * cos(MET_phi))` | `[-0.5, 2, 1]` |
| `cosh(x)`, `sinh(x)`, `tanh(x)` | `cosh(Jet_eta)` | `[[1.127626, 4.1443133, 1.8106556], [], [1.0200667, 1.4330864]]` |
| | `tanh(Jet_eta)` | `[[0.46211717, -0.97045195, 0.83365464], [], [-0.19737533, 0.71629786]]` |
| `hypot(x, y)` | `hypot(MET_pt, MET_pt)` | `[44.547726, 120.20815, 22.627417]` |
| `floor(x)`, `ceil(x)` | `floor(MET_pt)` | `[31, 85, 16]` |
| `round(x)`, halves to the even number, as NumPy | `round(Jet_pt / 10)` | `[[6, 2, 4], [], [12, 4]]` |
| `min(x, y)`, `max(x, y)` | `min(MET_pt, 50)` | `[31.5, 50, 16]` |
| `clip(x, low, high)` | `clip(MET_pt, 20, 50)` | `[31.5, 50, 20]` |
| `isfinite(x)` | `isfinite(MET_pt / 0)` | `[false, false, false]` |
| `if(condition, yes, no)` | `if(MET_pt > 30, MET_pt, 0)` | `[31.5, 85, 0]` |

`tanh(Jet_eta)` is each jet's cos θ. `floor`, `ceil` and `round` leave whole numbers as they
are and keep a float's kind; `round` takes no true-or-false values. `round` takes 6.5 to 6 and 3.5
to 4, as NumPy does, not up every time.

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
| `pt(pt, eta, phi)`, `pt(pt, eta, phi, mass)` | the pt of the same sum: of two muons, the dimuon's | `pt(Muon_pt, Muon_eta, Muon_phi)` | `[32.1647, 25, 0]` |
| `eta(…)`, `phi(…)` | its pseudorapidity η and its φ, each 0 for an event with nothing in the list | `eta(Muon_pt, Muon_eta, Muon_phi)` | `[1.3412633, -0.3, 0]` |
| | | `phi(Muon_pt, Muon_eta, Muon_phi)` | `[0.1473266, 1.5, 0]` |
| `rapidity(…)` | its rapidity, ½ ln((E + pz) / (E − pz)), NaN for an event with nothing in the list | `rapidity(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[0.030402305, nan, 0.067968756]` |

`eta` is the pseudorapidity, −ln tan(θ/2), which depends on the direction alone; `rapidity` uses
the energy too, and the two agree only where the sum is massless (a sum of massless particles
generally is not). `pt`, `eta`, `phi` and `rapidity`
take the same values as `mass`, and give what the `vector` library gives for `ak.sum` of the
event's four-vectors: an event with none has a sum of zero,
whose η and φ are 0 and whose rapidity is NaN, as there. A file holds any float, NaN and
infinities included, so such a rapidity is stored as NaN; to store another value for those events,
write it, as `if(count(Jet_pt) > 0, rapidity(Jet_pt, Jet_eta, Jet_phi, Jet_mass), -9)`. In a cut, NaN is
neither greater nor less than anything and `!=` to everything, as in NumPy.

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
…, so event 0's three jets make three pairs. A list of n members makes n(n − 1)/2 pairs, and one
event may make at most 65,536 (a list of 362 members makes 65,341): an event past that stops the press,
naming it, so select the members first, as `pair_mass(Jet_pt[good_jet], …)`.

| function | gives | example | result |
|---|---|---|---|
| `pair_first(list)` | the first member of each pair | `pair_first(Jet_pt)` | `[[65, 65, 22], [], [120]]` |
| `pair_second(list)` | the second member | `pair_second(Jet_pt)` | `[[22, 41, 41], [], [35]]` |
| `pair_dr(eta, phi)` | ΔR between the two | `pair_dr(Jet_eta, Jet_phi)` | `[[3.8209946, 1.3038405, 4.070574], [], [1.135867]]` |
| `pair_mass(pt, eta, phi)`, `pair_mass(pt, eta, phi, mass)` | the invariant mass of each pair | `pair_mass(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[[149.08528, 66.87832, 160.95856], [], [78.98824]]` |
| `pair_pt(…)` | the pt of each pair, as `pt` of the two | `pair_pt(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[[44.88033, 91.23413, 29.247534], [], [153.91696]]` |
| `pair_eta(…)`, `pair_phi(…)`, `pair_rapidity(…)` | its pseudorapidity η, φ and rapidity | `pair_eta(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[[-1.0266734, 0.91601306, -0.8157165], [], [0.07638098]]` |
| | | `pair_phi(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[[0.2649557, -0.3120652, -1.5437373], [], [3.0635805]]` |
| | | `pair_rapidity(Jet_pt, Jet_eta, Jet_phi, Jet_mass)` | `[[-0.34392893, 0.7685827, -0.16184664], [], [0.067968756]]` |

The pair functions give what `vector` gives for `pairs.a + pairs.b`, but for a pair with no
transverse momentum at all: its φ is 0 here, as for an event's sum, where `vector` keeps the first
member's.

### Text, branches and stored types

| function | gives | example | result |
|---|---|---|---|
| `contains(text, part)` | whether a text holds another; false when the sample lacks the field | `contains(sample.kind, 'sig')` | `true` |
| `branch(name)` | the branch of that name, often a name from the sample's metadata | `branch(sample.rho)`, with `rho = "MET_pt"` | `[31.5, 85, 16]` |
| `stored(x, kind)` | `x` converted to another kind, by [the conversion rule](#conversions) | `stored(nJet, 'u1')` | `[3, 0, 2]`, as `u1` |

The kinds are `f4`, `f8` (floats); `i1`, `i2`, `i4`, `i8` (signed whole numbers); `u1`, `u2`,
`u4`, `u8` (unsigned); and `b1` (true or false). A comparison gives `b1`; `count` of a list gives
`i8`; `sum` of true-or-false values and signed whole numbers gives `i8`, of unsigned ones `u8`, and
of floats the float kind it was given. [Kinds, exactly](#kinds-exactly) gives the rest.

## Defined names

When you find yourself writing the same thing twice, give it a name under `[define]`, and use the
name anywhere below it, as if it were a branch. A defined name may have a function's name, but that
function is then refused where it is called, so name a list of leptons' η `lep_eta`, not `eta`, to
keep `eta(…)`:

```toml
[define]
good_jet = "Jet_pt > 30 and abs(Jet_eta) < 2.4"
hard_central = "good_jet and Jet_pt > 100"    # a name may use the names above it
hem_veto = "count(Jet_eta < -1.3 and Jet_phi > 2.5) == 0"
```

| expression | result |
|---|---|
| `count(hard_central)` | `[0, 0, 1]` |
| `hem_veto` | `[false, true, true]` |
| `sum(Jet_pt[hard_central])` | `[0, 0, 120]` |

A defined name is worked out once for each run of events, however many times it is used, so
writing `Muon_pt[good_muon]`, `Muon_eta[good_muon]` and `Muon_phi[good_muon]` costs no more than
writing the condition once. A name is never a branch's name, so each name means one thing: `rvr
press` refuses a defined name that is a branch of any sample, as `samples.lock` lists them, before
it reads anything. Long branch names can be given a short one the same way:
`trig = "HLT_IsoMu17_eta2p1_LooseIsoPFTau20"`.

## Mistakes

If something in the recipe doesn't make sense, `rvr press` stops before it reads anything and
tells you what's wrong, in plain words, every problem at once, one a line:

| expression | `rvr` says |
|---|---|
| `Jet_pt >` | Expected a value at the end (column 9) |
| `count(Jet_pt, 3)` | count takes 1 value, not 2 |
| `cuont(Jet_pt)` | There is no function named cuont. Did you mean: count? |
| `Jet.pt` | Only sample has a field after a dot, as in sample.era (the dot at column 4). Name a branch in full, such as Jet_pt, and keep some members of it with a mask, such as Jet_pt[good_jet]; a branch whose name holds a dot is written branch('Jet.pt') |
| `good_jet()` | good_jet is a defined name, not a function: write it without brackets, as good_jet |
| `not nJet` | not needs a true-or-false value, not u4: to test a number, compare it, as x != 0 |
| `nJet > -1` | -1 does not fit beside a u4 column |
| `MET_pt < 'high'` | Text can only be compared with == and !=, not < |
| `select = "…"` at the top of the recipe | A recipe has no key "select"; it takes format, version, title, define, keep, columns, totals, derived. |

A name that is not a branch (`Jet_ptt`) is answered with the nearest branches the samples have,
before anything is read.

### How deep

An expression is at most 100 levels deep. A value (a number, text, a name, `sample.FIELD`) is one
level, a defined name as many as its own expression, and each operator, call, pair of brackets, mask
and `in` list one more than the deepest part it holds: `a + b + c` is 3 levels, `abs(-x)` 3, and
`(x)` 2. A derived column is as deep as its expression, counting each derived column it uses as deep
as that one's. A deeper expression is refused as it is read.

### Values the recipe cannot take

A few mistakes lie in the events, not in the recipe's text, so `rvr` meets them only as it works an
expression out: the press stops there, naming the file, the entry, the column and the value.

- a value [converted](#conversions) to a kind that cannot hold it, or NaN made whole or `b1`;
- a mask, or two lists taken member by member (in an operator, `if`, or the lists of a kinematic or
  matching function), whose lengths differ in an event;
- an event whose list would make more than 65,536 pairs in a pair function;
- a whole number raised to a negative power;
- a total whose sum is not a finite number.

NaN and the infinities are values, never mistakes: a float column may hold them, and every function
says what it gives for them.

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
| `ak.num(ev.Jet_pt)` | `count(Jet_pt)` | `[3, 0, 2]` |
| `ak.sum(ev.Jet_pt > 30, axis=1)` | `count(Jet_pt > 30)` | `[2, 0, 2]` |
| `ak.sum(ev.Jet_pt, axis=1)` | `sum(Jet_pt)` | `[128, 0, 155]` |
| `ev.Jet_pt[(ev.Jet_pt > 30) & (abs(ev.Jet_eta) < 2.4)]` | `Jet_pt[Jet_pt > 30 and abs(Jet_eta) < 2.4]` | `[[65, 41], [], [120, 35]]` |
| `ak.sum(ev.Jet_pt[(ev.Jet_pt > 30) & (abs(ev.Jet_eta) < 2.4)], axis=1)` | `sum(Jet_pt[Jet_pt > 30 and abs(Jet_eta) < 2.4])` | `[106, 0, 155]` |
| `ak.fill_none(ak.max(ev.Jet_pt, axis=1), -1)` | `greatest(Jet_pt, -1)` | `[65, -1, 120]` |
| `ak.fill_none(ak.pad_none(ak.sort(ev.Jet_pt, ascending=False), 2)[:, 1], -1)` | `nth(sorted(Jet_pt, 'descending'), 1, -1)` | `[41, -1, 35]` |
| `(ak.sum((ev.Jet_pt > 30) & (abs(ev.Jet_eta) < 2.4), axis=1) >= 2) & (ev.MET_pt > 30)` | `count(Jet_pt > 30 and abs(Jet_eta) < 2.4) >= 2 and MET_pt > 30` | `[true, false, false]` |
| `ak.sum((ev.Muon_pt > 20) & ev.Muon_tightId & (ev.Muon_charge < 0), axis=1)` | `count(Muon_pt > 20 and Muon_tightId and Muon_charge < 0)` | `[0, 1, 0]` |
| `ev.Jet_eta[ak.argsort(ev.Jet_pt, ascending=False)]` | `take(Jet_eta, order(Jet_pt, 'descending'), -9)` | `[[0.5, 1.2, -2.1], [], [-0.2, 0.9]]` |
| `ak.concatenate([ev.Electron_pt, ev.Muon_pt], axis=1)` | `join(Electron_pt, Muon_pt)` | `[[44, 12], [30, 25], [50, 9]]` |
| `np.floor(ev.MET_pt)` | `floor(MET_pt)` | `[31, 85, 16]` |

### Longer ones

Here the Python takes a few lines, and the recipe still takes one. Each is written as it would sit
in `[columns]`.

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

**The pt of the leptons together**, electrons and muons joined into one list:

<table>
<tr><th>Python (Awkward)</th><th>rvr</th></tr>
<tr><td>

```python
pt = ak.concatenate([ev.Electron_pt, ev.Muon_pt], axis=1)
phi = ak.concatenate([ev.Electron_phi, ev.Muon_phi], axis=1)
px, py = ak.sum(pt * np.cos(phi), axis=1), ak.sum(pt * np.sin(phi), axis=1)
np.hypot(px, py)
```

</td><td>

```toml
lepton_pt = "pt(join(Electron_pt, Muon_pt), join(Electron_eta, Muon_eta), join(Electron_phi, Muon_phi))"
```

</td></tr>
</table>

Both give `[32.1647, 12.995836, 42.541744]`.

## Picking the jets you want

Say you only want the jets with pt above 30 GeV and |η| below 2.4. There are two ways to write
that, and both pick exactly the same jets. Which one you use depends on how often you need them.

**1. A defined name**

```toml
[define]
good_jet = "Jet_pt > 30 and abs(Jet_eta) < 2.4"
```

`good_jet` is a true-or-false list beside the jets, and picks them from any jet branch:
`Jet_pt[good_jet]` is their pt, `Jet_eta[good_jet]` their eta, and `count(good_jet)` how many
there are.

**2. A mask written where it is used**

```toml
[columns]
good_jet_eta = "Jet_eta[Jet_pt > 30 and abs(Jet_eta) < 2.4]"
```

The brackets keep the members of `Jet_eta` where the condition is true. But the condition now
lives in this one line only: if you also want the good jets' pt, or how many there are, you write
it out again, and if you change the cut you change it everywhere you wrote it.

**So which one?** Define a name for anything you'll use more than once: good jets, good muons,
good electrons. Then the cut lives in one place, and every cut and column that says `good_jet`
means the same jets. A mask written in place is fine for a one-off. On the example events:

| expression | written as | result |
|---|---|---|
| `Jet_eta[good_jet]` | 1 | `[[0.5, 1.2], [], [-0.2, 0.9]]` |
| `Jet_eta[Jet_pt > 30 and abs(Jet_eta) < 2.4]` | 2 | `[[0.5, 1.2], [], [-0.2, 0.9]]` |
| `count(good_jet)` | 1 | `[2, 0, 2]` |
| `count(Jet_pt > 30 and abs(Jet_eta) < 2.4)` | 2 | `[2, 0, 2]` |

### More than one kind of particle

An expression can use any branches together, whatever particles they are about: each is named in
full, so there is never a question of which one a name means. Here `good_muon` is a second defined
name: `good_muon = "Muon_tightId and Muon_pt > 20"`.

| expression | gives | result |
|---|---|---|
| `count(good_jet) + count(good_muon)` | the good jets and good muons of each event, together | `[3, 1, 2]` |
| `Jet_pt[good_jet and dr_nearest(Jet_eta, Jet_phi, Muon_eta[good_muon], Muon_phi[good_muon]) > 0.4]` | the good jets that aren't within ΔR 0.4 of a good muon | `[[41], [], [120, 35]]` |

Event 0 shows the cleaning at work: its first jet, at η 0.5 and φ 0.1, sits 0.14 away from the good
muon at η 0.4 and φ 0.2, so it goes.

The cleaning can itself be a defined name, using the names above it:

```toml
[define]
good_muon = "Muon_tightId and Muon_pt > 20"
clean_jet = "Jet_pt > 30 and dr_nearest(Jet_eta, Jet_phi, Muon_eta[good_muon], Muon_phi[good_muon]) > 0.4"
```

| expression | gives | result |
|---|---|---|
| `count(clean_jet)` | how many clean jets each event has | `[1, 0, 2]` |

## Kinds, exactly

This section and the next are the language's arithmetic in full: with them, a reader can say what
any expression gives, kind and bits, and which it refuses.

### Two kinds together

Every column holds one of the eleven kinds: `b1`; the whole-number kinds `i1`, `i2`, `i4`, `i8`
(signed, 8 to 64 bits) and `u1`, `u2`, `u4`, `u8` (unsigned); and the floats `f4` and `f8`
(IEEE single and double). Where two values meet, in an operator, `min`, `max`, `clip`, `if`, `join`
or a function's `otherwise`, both are brought to the kind they combine in, `numpy.result_type` of
NumPy 2.5.1:

| combined with | b1 | i1 | i2 | i4 | i8 | u1 | u2 | u4 | u8 | f4 | f8 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| b1 | b1 | i1 | i2 | i4 | i8 | u1 | u2 | u4 | u8 | f4 | f8 |
| i1 | i1 | i1 | i2 | i4 | i8 | i2 | i4 | i8 | f8 | f4 | f8 |
| i2 | i2 | i2 | i2 | i4 | i8 | i2 | i4 | i8 | f8 | f4 | f8 |
| i4 | i4 | i4 | i4 | i4 | i8 | i4 | i4 | i8 | f8 | f8 | f8 |
| i8 | i8 | i8 | i8 | i8 | i8 | i8 | i8 | i8 | f8 | f8 | f8 |
| u1 | u1 | i2 | i2 | i4 | i8 | u1 | u2 | u4 | u8 | f4 | f8 |
| u2 | u2 | i4 | i4 | i4 | i8 | u2 | u2 | u4 | u8 | f4 | f8 |
| u4 | u4 | i8 | i8 | i8 | i8 | u4 | u4 | u4 | u8 | f8 | f8 |
| u8 | u8 | f8 | f8 | f8 | f8 | u8 | u8 | u8 | u8 | f8 | f8 |
| f4 | f4 | f4 | f4 | f8 | f8 | f4 | f4 | f8 | f8 | f4 | f8 |
| f8 | f8 | f8 | f8 | f8 | f8 | f8 | f8 | f8 | f8 | f8 | f8 |

A whole-number value brought to a wider whole kind is unchanged; one brought to a float is rounded
once to the nearest float, ties to even. Then:

- `+ - * % **` give that kind; `/` gives it too when it is a float, and `f8` when it is whole (two
  whole numbers divide as doubles); arithmetic on two `b1` values is refused.
- A comparison gives `b1`. Two whole-number values compare exactly as the numbers they are, each in
  its own kind, never through a float (an `i8` 2^53 + 1 is not a `u8` 2^53, where the table's `f8`
  would make them equal); any other two compare in the kind of the table.
- `and`, `or` and `not` take `b1` values and give `b1`.

### Fixed values beside a column

A number, a truth value or a sample's field written into an expression is a **fixed value**, and is
weak: it takes the kind of the column beside it, rather than combining by the table.

| fixed value | beside a `b1` column | beside a whole-number kind K | beside a float kind K |
|---|---|---|---|
| a whole number | `i8` | K, refused when K cannot hold it | K, rounded once to it |
| a float | `f8` | `f8` | K, rounded once to it (so `MET_pt > 0.15` compares with 0.15 rounded to `f4`) |
| a truth value | `b1` | K, true being 1 | K, true being 1 |

Fixed values together are worked out as `i8` (whole numbers), `f8` (floats) or `b1`, and the result
is fixed and weak again; so is a function of fixed values only: `pi / 2`, `sqrt(2)`, `cos(pi)` and
`delta_r(0, 0, 1, 1)` are weak floats, and `MET_pt * sqrt(2)` is `f4`. Where a column is needed
(a column of `[columns]`, a total, a derived column) a fixed value alone becomes one value per event
of `i8`, `f8` or `b1`; text is refused.

| expression | kind | result |
|---|---|---|
| `nJet + 1` | `u4` | `[4, 1, 3]` |
| `nJet - 4` | `u4` | `[4294967295, 4294967292, 4294967294]` |
| `nJet * 0.5` | `f8` | `[1.5, 0, 1]` |
| `nJet / 2` | `f8` | `[1.5, 0, 1]` |
| `nJet * sample.era` | `u4` | `[9, 0, 6]` |
| `MET_pt * 0.5` | `f4` | `[15.75, 42.5, 8]` |
| `MET_pt * sqrt(2)` | `f4` | `[44.547726, 120.20815, 22.627417]` |
| `MET_pt * nJet` | `f8` | `[94.5, 0, 32]` |
| `Flags + 0.5` | `f8` | `[11.5, 4.5, 1.5]` |
| `Muon_charge * Muon_tightId` | `i4` | `[[1, 0], [-1], []]` |
| `if(MET_pt > 30, 1, 0.5)` | `f8` | `[1, 1, 0.5]` |
| `first(Muon_charge, 0.5)` | `f8` | `[1, -1, 0.5]` |
| `least(Jet_pt, -1)` | `f4` | `[22, -1, 35]` |
| `2 ** 3 ** 2` | | `512` |
| `-7 % 3` | | `2` |
| `7 % -3` | | `-2` |

### Numbers

**Whole numbers** are worked in 64 bits and brought back to their kind after each operation, by
wrapping (a `u1` 250 + 10 is 4). `%` is Python's: the remainder takes the divisor's sign, and
`x % 0` is 0. `**` with a negative exponent is a [data error](#values-the-recipe-cannot-take).

**Floats** are worked as doubles. An `f4` operation is done in `f8` and rounded once to `f4` after
it, which is exactly IEEE single arithmetic for `+ - * /` and `sqrt`. `%` is Python's: r is the
exact `fmod(x, y)`; a nonzero r whose sign is not y's has y added; a zero r takes y's sign; y = 0
gives NaN. `x ** y` is `x * x` for y exactly 2, else libm's `pow(x, y)`. NaN is not equal to
anything, itself included (`!=` is true), nor less or greater than anything; arithmetic on it gives
NaN.

### Conversions

`stored(x, 'K')` and `stored = "K"` convert each value to K:

| from | to | the value |
|---|---|---|
| a float | a whole-number kind | truncated toward zero (2.9 is 2, -2.9 is -2); NaN, or a truncated value K cannot hold, is a data error |
| a whole number | a whole-number kind | unchanged; one K cannot hold is a data error |
| a whole number | `f4` or `f8` | rounded once to the nearest, ties to even (straight to `f4`, not through `f8`) |
| `f8` | `f4` | rounded to the nearest `f4`, ties to even; past the largest `f4`, an infinity |
| `f4` | `f8` | exact |
| any number | `b1` | whether it is not zero (-0.0 is zero); NaN is a data error |
| `b1` | any kind | 0 or 1 |

| expression | kind | result |
|---|---|---|
| `stored(MET_pt, 'i4')` | `i4` | `[31, 85, 16]` |
| `stored(-MET_pt / 10, 'i1')` | `i1` | `[-3, -8, -1]` |
| `stored(nJet, 'f4')` | `f4` | `[3, 0, 2]` |

### Signed zeros

`-x` of 0.0 is -0.0; a zero remainder takes the divisor's sign (`-4.0 % 2` is +0.0); `sign` of
either zero is +0.0; `abs` of either is +0.0; and of two equal values `min` and `max` give the right
one, so `min(0.0, -0.0)` is -0.0.

### What differs from NumPy

The kinds above are NumPy's type promotion, but for these differences:

- a fixed value that a whole-number column's kind cannot hold is refused, in a comparison too
  (NumPy takes `u1 < 300` as true); a function of fixed values, and a sample's field, stays weak
  (NumPy's `np.sqrt(2)` is a strong `f8`, so `MET_pt * np.sqrt(2)` is `f8` there);
- there is no `f2`: a float function of `b1`, `i1` or `u1` gives `f4` (NumPy `f2`); `floor` and
  `ceil` of `b1` keep `b1`, and `round` refuses it (NumPy gives `f2`);
- arithmetic on two `b1` values is refused (NumPy's `+` is or, `*` and, `/` gives `f8`);
- a float function of two values takes the float of the kind they combine in: `atan2` of an `i1` and
  a `u2` is `f8` (they combine in `i4`), where NumPy gives `f4`;
- an `f4` function works in `f8` and rounds once, where NumPy's single-precision routines can differ
  in the last bit; `x ** 0.5` is `pow` here and `sqrt` there (they differ only for -0 and -inf);
- `sum` of `f4` adds in list order (as Awkward does; NumPy adds pairwise);
- a conversion of a value the kind cannot hold is refused (NumPy wraps it, or leaves it undefined).

## Functions, exactly

The float kind of a function's result is its input's kind, made a float: `f4` for `b1`, `i1`, `u1`,
`i2`, `u2` and `f4`, `f8` for `i4`, `u4`, `i8`, `u8` and `f8`. For a function of several values it
is that of the kind they combine in (the table), a fixed one weak beside it. Every float function
works on its values as doubles and rounds its double result once to that kind. "libm" is the `libm`
crate at 0.2.16; `sqrt` is IEEE's.

| function | exactly |
|---|---|
| `count(list)` | per event, of a `b1` list the number of true members, of any other the number of members; `i8` |
| `sum(list)` | per event, the members added one at a time from 0 in list order: floats in the list's kind, rounded to it after each addition; whole numbers in 64 bits, wrapping, giving `i8` for `b1` and signed kinds and `u8` for unsigned ones; 0 for an empty list |
| `any(list)`, `all(list)` | of a `b1` list, whether any (every) member is true; `b1`; `any` of an empty list is false, `all` true |
| `least(list, otherwise)`, `greatest` | per event, scanning from the first member, the first that is less (greater) than every one before it: of equal ones the first; a NaN is kept when it comes first and passed over after; whole numbers, `u8` too, compared exactly; `otherwise` for an empty list; the kind of the list with `otherwise` beside it |
| `least_index(list, otherwise)`, `greatest_index` | the position, from 0, of the member `least` (`greatest`) chooses; `i8`; `otherwise` a whole number |
| `first(list, otherwise)` | `nth(list, 0, otherwise)` |
| `nth(list, n, otherwise)` | the member at position n (a fixed whole number from 0), or `otherwise` when the list has n members or fewer; the kind as `least` |
| `index(list)` | each member's position, 0, 1, …; `i8` |
| `sorted(list)`, `sorted(list, 'descending')` | per event, the members in ascending (descending) order by a stable sort: equal members keep their order, and NaN comes first either way; whole numbers compared exactly; the list's kind |
| `order(list)`, `order(list, 'descending')` | the positions, from 0, of that sort's members; `i8` |
| `take(list, positions, otherwise)` | per event, for each of its whole-number positions the list's member there, or `otherwise` for a negative position or one past the end; shaped like the positions; the kind as `least` |
| `listed(list, positions)` | `b1` shaped like the list: whether each member's position is among the event's whole-number positions |
| `join(list, list, …)` | per event, every member of the first list, then of the second, and on; the kind they combine in |
| `abs(x)` | a float's magnitude (its sign bit cleared); a signed whole number's, wrapping (an `i1` -128 stays -128); an unsigned or `b1` value unchanged |
| `sign(x)` | of a float, 1 above 0, -1 below, +0.0 for either zero, NaN for NaN; of a whole number -1, 0 or 1 in its kind (0 or 1 when unsigned); `b1` unchanged |
| `sqrt(x)` | the square root, in the float kind |
| `exp`, `log`, `log10`, `cos`, `sin`, `tan`, `acos`, `asin`, `atan`, `cosh`, `sinh`, `tanh` | libm's function of x, in the float kind |
| `atan2(y, x)`, `hypot(x, y)` | libm's function of the two, in the float kind of the kind they combine in |
| `floor(x)`, `ceil(x)` | libm's `floor`, `ceil`, in the float's kind; whole numbers and `b1` unchanged |
| `round(x)` | libm's `rint`: to the nearest whole number, halves to even, in the float's kind (`round(-0.5)` is -0.0); whole numbers unchanged; `b1` refused |
| `min(x, y)`, `max(x, y)` | value by value, in the kind they combine in: NaN when either is (x when both), else x when it is less (greater) than y, else y; whole numbers compared exactly |
| `clip(x, low, high)` | `min(max(x, low), high)`, so `low` above `high` gives `high` |
| `isfinite(x)` | `b1`: false for NaN and the infinities; true for every whole number and `b1` |
| `if(condition, yes, no)` | the condition a `b1` value (or a fixed truth, which works out only the value it chooses and gives it as it is); value by value, yes where it is true and no elsewhere, in the kind yes and no combine in; shaped like the first list among the three, and a list among them must have the others' members |
| `bit_and(x, mask)` | x & mask, in x's whole-number kind; mask a fixed whole number x's kind can hold |
| `shift_right(x, n)` | x shifted right n bits, n a fixed whole number from 0 that x's kind can hold: arithmetic for a signed kind, logical for an unsigned one; n of 64 or more gives 0 (-1 for a negative signed x) |
| `delta_phi(a, b)` | in doubles, r = `fmod(a - b + π, 2π)`, plus 2π when r is negative, minus π; in the float kind. It lies in [-π, π]: rounding can give +π, and in `f4` 3.1415927 |
| `delta_r(eta1, phi1, eta2, phi2)` | in doubles, d = eta1 - eta2 and p = `delta_phi(phi1, phi2)`, then `sqrt(d * d + p * p)`; in the float kind |
| `mass`, `pt`, `eta`, `phi`, `rapidity` of `(pt, eta, phi)` or `(pt, eta, phi, mass)` | per event, in doubles: each member's four-momentum, with q = pt · cosh(eta), is E = sqrt(q · q + m · \|m\|) (m 0 without a mass), px = pt · cos(phi), py = pt · sin(phi), pz = pt · sinh(eta); the members' four-momenta are added component by component from 0, in list order. Of the sum S: `mass` is copysign(sqrt(\|m2\|), m2) with m2 = ((E · E - px · px) - py · py) - pz · pz; `pt` is sqrt(px · px + py · py); `phi` is atan2(py, px); `eta` is asinh(pz / pt), but 0 where that is NaN and pz is 0; `rapidity` is 0.5 · log((E + pz) / (E - pz)). In the float kind of the lists. An event with no members has the sum 0: mass, pt, eta and phi 0, rapidity NaN. cosh, cos, sin, sinh, atan2, asinh and log are libm's; the lists must have the same members |
| `dr_nearest`, `dr_second_nearest`, `dr_farthest`, `dr_second_farthest` of `(eta1, phi1, eta2, phi2)` or with a fifth value | for each member of the first list, `delta_r` (in doubles) to each member of the second list of the same event, a NaN distance passed over, sorted by a stable sort, nearest first (farthest first): the first of them (the second); in the float kind of the four lists, or where there is none the fifth value (a fixed number or truth), by default inf |
| `nearest_index`, `second_nearest_index`, `farthest_index`, `second_farthest_index` | the position, from 0, in the second list of the member the `dr_` function of the same name chooses; `i8`; where there is none the fifth value (a fixed whole number), by default -1 |
| `pair_first(list)`, `pair_second(list)` | per event, for each pair of members (0, 1), (0, 2), …, (0, n - 1), (1, 2), …, its first (second) member; the list's kind |
| `pair_dr(eta, phi)` | each pair's `delta_r`; in the float kind of the two |
| `pair_mass`, `pair_pt`, `pair_eta`, `pair_phi`, `pair_rapidity` of `(pt, eta, phi)` or `(pt, eta, phi, mass)` | each pair's two four-momenta added from 0, first then second, and measured as `mass`, `pt`, … measure a sum; a pair with no transverse momentum has phi 0 |
| `contains(text, part)` | whether the text holds the part; false when the sample lacks either field |
| `branch(name)` | the branch of that name (text, written in or a sample's text field) |
| `stored(x, kind)` | x, a column, converted to the kind by the [conversion rule](#conversions) |

A pair function (`pair_…`) refuses an event whose list would make more than 65,536 pairs, a
[data error](#values-the-recipe-cannot-take); every list two values are taken member by member from
must have the same number of members in each event, or it is one too.

Every function works event by event, so no result depends on how many events are worked on at
once. That number is only how much is held: a press reads up to 50,000 entries at a time, a reader
works a derived column out a million rows at a time, and either works on fewer at once (halving
until they fit) where the events' lists would make more than 67,108,864 (2^26) pairs together. Only
the limit of one event, 65,536 pairs, ever stops a press.
