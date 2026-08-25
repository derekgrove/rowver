# `.rvr` 6

The only supported container is:

```text
gzip(u32 little-endian header length | UTF-8 JSON header | column buffer)
```

All buffer values are little-endian. Columns are contiguous `f8` or `i4` arrays followed
by one `i4` sample index per row; a mixed data/MC file also has one `f8` weight array.

The header has `rowver: 6` and five drawers:

- `about`: study name and human-facing description.
- `data`: immutable offsets, types, sample counts, and weight policy.
- `view`: settings shared by all tabs.
- `tabs`: ordered page setups. `opens_on: 0` is the blank-page sentinel when this is empty.
- `made`: optional provenance; removing it never changes a plotted number.

Continuous columns use inclusive `[low, high]` cuts (`null` opens one end); categorical
columns use `{ "in": [...] }` with declared values. Luminosity is a positive finite number
or `null`, namespaced as `view.lumis.{sample,sub,group}`; the outermost configured value
wins. Appearance is likewise namespaced as `view.look.{sample,sub,group}` so each tier's
settings stay explicit. Sample, subgroup, and group names are unique across those namespaces
because stage source references are strings. `view.groups.order` may declare an
empty group so a page can create a destination before samples are moved into it.
`weighted: false` means observed counts and is never scaled.

A stage is ordered. Its leaves are `1d`, `2d`, `3d`, `board`, or `map`. A `block` contains
one to six non-nested leaves or interior `null` holes and an explicit layout chosen from:

```text
[6] [3,3] [2,2,2] [3,3,6] [6,3,3] [3,3,3,3] [2,2,2,2,2,2]
```

An effective stage contains at most one `board` and one `map`. Plot and map column names
must resolve in `data.cols`. Boards are at most 4 × 4 and may reference only declared
sources. Map source/background/signal references must likewise resolve; region membership
cuts use declared categorical values, and stored grid recipes are bounded and non-overlapping.

Writers merge edits into the parsed metadata, preserve unknown keys, never rewrite
`data`, and pass the column buffer through byte-for-byte. `rvr6.check` is normative for
validation limits and invariants.
