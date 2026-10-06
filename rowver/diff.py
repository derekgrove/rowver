"""Say where two .rvr files differ, from their headers alone: no array is decompressed.

    rowver diff <a.rvr> <b.rvr>

Every per-row array carries one sha256 per sample (by_sample), so two headers are enough to
name the column and the sample that changed between one press and the next.
"""
import struct

from . import rvr


def parts(column):
    """A column's arrays that carry by_sample, by part: values, or offsets and content."""
    if column["kind"] != "jagged":
        return {"values": column}
    return {"offsets": column["offsets"], "content": column["content"]}


def kind(column):
    """A column's kind as text: f4, or jagged f4."""
    return column["kind"] if column["kind"] != "jagged" else f"jagged {column['content']['kind']}"


def compare(a, b):
    """What differs between the files at two paths, reading only their headers: rows, the samples
    and columns only one holds, samples whose rows differ, columns whose kind differs, and for the
    samples and columns both hold, each (column, sample) whose by_sample differs, with the parts."""
    first, second = rvr.read_header(a)["data"], rvr.read_header(b)["data"]
    places = [{s["name"]: (i, s["rows"]) for i, s in enumerate(data["samples"])} for data in (first, second)]
    columns = [{c["name"]: c for c in data["cols"]} for data in (first, second)]
    shared = [name for name in places[0] if name in places[1]]
    both = [name for name in columns[0] if name in columns[1]]

    def differing(one, other):
        """The shared samples in which two arrays' by_sample differ."""
        return [name for name in shared
                if one["by_sample"][places[0][name][0]] != other["by_sample"][places[1][name][0]]]

    changed = []
    for name in both:
        if kind(columns[0][name]) != kind(columns[1][name]):
            continue
        found = {part: differing(record, parts(columns[1][name])[part])
                 for part, record in parts(columns[0][name]).items()}
        changed += [{"column": name, "sample": sample, "parts": [part for part in found if sample in found[part]]}
                    for sample in shared if any(sample in samples for samples in found.values())]
    return {
        "rows": [first["rows"], second["rows"]],
        "samples": {"only_a": [name for name in places[0] if name not in places[1]],
                    "only_b": [name for name in places[1] if name not in places[0]],
                    "rows": {name: [places[0][name][1], places[1][name][1]] for name in shared
                             if places[0][name][1] != places[1][name][1]}},
        "columns": {"only_a": [name for name in columns[0] if name not in columns[1]],
                    "only_b": [name for name in columns[1] if name not in columns[0]],
                    "kind": {name: [kind(columns[0][name]), kind(columns[1][name])] for name in both
                             if kind(columns[0][name]) != kind(columns[1][name])}},
        "entry": differing(first["entry"], second["entry"]),
        "changed": changed,
    }


def main(a, b):
    """Print one line per difference; the exit status, 0 when the headers show none."""
    try:
        found = compare(a, b)
    except (OSError, ValueError, KeyError, IndexError, TypeError, struct.error) as error:
        print(f"cannot compare: {type(error).__name__}: {error}")
        return 2
    said = []
    if found["rows"][0] != found["rows"][1]:
        said.append(f"rows: {found['rows'][0]:,} in {a}, {found['rows'][1]:,} in {b}")
    for what in ("samples", "columns"):
        said += [f"{what[:-1]} {name}: only in {a}" for name in found[what]["only_a"]]
        said += [f"{what[:-1]} {name}: only in {b}" for name in found[what]["only_b"]]
    said += [f"sample {name}: {rows[0]:,} rows in {a}, {rows[1]:,} in {b}"
             for name, rows in found["samples"]["rows"].items()]
    said += [f"column {name}: {kinds[0]} in {a}, {kinds[1]} in {b}" for name, kinds in found["columns"]["kind"].items()]
    said += [f"entry: differs in sample {sample}" for sample in found["entry"]]
    said += [f"column {c['column']}: differs in sample {c['sample']}"
             + ("" if c["parts"] == ["values"] else f" ({', '.join(c['parts'])})") for c in found["changed"]]
    print("\n".join(said) if said else "The headers show no difference in rows, samples, columns or by_sample.")
    return 1 if said else 0
