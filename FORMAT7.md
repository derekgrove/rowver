# `.rvr` 7 — packed boolean columns

Version 7 retains the container, metadata drawers, weighting, sample membership,
and event-index semantics of [version 6](FORMAT.md). This document specifies its
only changes. Readers support both versions; newly pressed cartridges use 7.
Saving an existing cartridge preserves its version, data drawer, and buffer.
The historical `rvr6` Python module name remains a supported import path.

## Boolean encoding

`data.cols[].kind` additionally accepts **`"b1"`**, meaning one bit per event.
`at` is a buffer-relative byte offset. Its span is exactly `ceil(data.rows / 8)`
bytes, with no alignment requirement beyond a byte boundary.

Event `i` is bit `i % 8`, least significant bit first, of byte `at + floor(i / 8)`.
False is 0 and true is 1. Bits continue across sample boundaries; the existing
sample-index array identifies the sample for every event. Event order never changes.
Unused high bits of the final byte **must be zero**. Empty columns occupy zero bytes.

Only columns whose populated arrays are boolean in every sample are pressed as `b1`.
Empty samples do not force a populated boolean column to widen. If all samples
are empty, the declared array types determine the kind.
Integer columns containing 0 and 1 remain integer columns. Mixing boolean arrays
with numeric arrays across samples follows the existing numeric promotion rules.
Weights remain float64 and sample membership remains int32.

`lim`, `nice`, percentiles, optional `vals`, and labels use numeric 0 and 1, keeping
selection and plotting semantics unchanged. For an empty boolean column `lim` is
`[0, 0]`; the presser emits zero percentiles and `vals: []`. When present, `vals`
must exactly enumerate the occurring numeric values in ascending order.

## Buffer geometry

The buffer length equals:

```text
rows × (4 + (8 if weights are present else 0))
+ sum(rows × numeric column width)
+ number_of_boolean_columns × ceil(rows / 8)
```

All spans must be in bounds and non-overlapping, and together fill the buffer.
There are no padding bytes. Existing numeric alignment rules still apply.
The presser emits weights, float64 columns, int32 columns, sample membership,
then packed boolean columns. Column descriptors retain the study's declaration
order; readers use offsets rather than assuming physical block order.

The 1 GiB limit applies to the **packed** buffer. Pressing, including parallel
pressing, accounts for packed boolean spans in the running size check.

## Reading and saving

Framing, validation, hashing, copying, and saving do not require an unpacked boolean
event array. The Python validator checks packed bytes directly. `columns()` and
`read_by_sample()` materialize NumPy boolean arrays, one byte per event, when
callers request event values. Other numeric arrays retain their existing views.
The viewer materializes one-byte values when projecting a cartridge for plotting,
while retaining the original packed buffer for saves.

Readers must reject `b1` in a version 6 cartridge. An older v6-only reader rejects
version 7. A conforming save preserves the packed buffer byte-for-byte, including
its zero tail bits, and preserves unknown metadata as required by version 6.

The cross-implementation examples are in `contract/v7/cases.json`. The unchanged
`contract/v6/cases.json` remains the compatibility contract for existing cartridges.
