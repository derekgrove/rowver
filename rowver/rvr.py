"""Build, write, and read an .rvr file: a JSON header and a buffer of arrays."""
import gzip
import hashlib
import json
import math
import struct

import awkward as ak
import numpy as np

VERSION = 11
FEW = 64


def kind(dtype):
    """The header spelling of a NumPy dtype: f4, i8, u4, b1."""
    dtype = np.dtype(dtype)
    return f"{dtype.kind}{dtype.itemsize}"


def dtype(kind):
    """The little-endian NumPy dtype of a header kind."""
    return np.dtype(bool) if kind == "b1" else np.dtype("<" + kind)


def made_checksum(made):
    """SHA-256 of the complete made section, excluding its own checksum."""
    body = {key: value for key, value in made.items() if key != "sha256"}
    text = json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def join(pieces):
    """Concatenate arrays: NumPy for flat columns, Awkward for jagged ones."""
    if all(isinstance(p, ak.Array) and p.ndim == 1 for p in pieces):
        pieces = [ak.to_numpy(p) for p in pieces]
    return np.concatenate(pieces) if isinstance(pieces[0], np.ndarray) else ak.concatenate(pieces)


def outward(x, up):
    """x rounded away from the range to two significant digits."""
    if x == 0:
        return 0.0
    step = 10.0 ** (math.floor(math.log10(abs(x))) - 1)
    return (math.ceil if up else math.floor)(round(x / step, 6)) * step


def facts(array, labels):
    """lim, nice, and for integer kinds vals and names, of one flat column."""
    integer = array.dtype.kind in "iub"
    if len(array) == 0:
        return {"lim": [0, 0], "nice": [0, 1], **({"vals": []} if integer else {})}
    lo, hi = array.min().item(), array.max().item()
    nice = [outward(lo, False), outward(hi, True)]
    if integer:
        lo, hi, nice = int(lo), int(hi), [math.floor(nice[0]), math.ceil(nice[1])]
    if nice[0] == nice[1]:
        nice[1] += 1
    out = {"lim": [lo, hi], "nice": nice}
    distinct = np.unique(array)
    if integer and len(distinct) <= FEW:
        out["vals"] = [int(v) for v in distinct]
        if labels:
            out["names"] = {str(v): labels[v] for v in out["vals"] if v in labels}
    return out


def pack(samples, labels):
    """The data section and the buffer of a skim: {sample: {"columns": {...}}}."""
    rows = [len(next(iter(s["columns"].values()))) for s in samples.values()]
    data = {"rows": sum(rows), "samples": [{"name": name, "rows": r} for name, r in zip(samples, rows)]}
    buffer = bytearray()

    def put(array):
        """Append one array on an 8-byte boundary; return its offset."""
        buffer.extend(b"\0" * (-len(buffer) % 8))
        at = len(buffer)
        buffer.extend(np.ascontiguousarray(array).astype(array.dtype.newbyteorder("<")).tobytes())
        return at

    data["sample_at"] = put(np.repeat(np.arange(len(samples), dtype="<i4"), rows))
    data["cols"] = []
    for name in next(iter(samples.values()))["columns"]:
        array = join([s["columns"][name] for s in samples.values()])
        if isinstance(array, np.ndarray):
            record = {"name": name, "kind": kind(array.dtype), "at": put(array),
                      **facts(array, labels.get(name))}
        else:
            offsets = np.concatenate([[0], np.cumsum(ak.to_numpy(ak.num(array)))]).astype("<i8")
            content = ak.to_numpy(ak.flatten(array))
            record = {"name": name, "kind": "jagged", "at": put(offsets),
                      "content": {"kind": kind(content.dtype), "at": put(content), "count": len(content)}}
        data["cols"].append(record)
    data["sha256"] = hashlib.sha256(buffer).hexdigest()
    return data, bytes(buffer)


def write(path, header, buffer):
    """Write header and buffer as one .rvr file."""
    text = json.dumps(header).encode()
    with gzip.GzipFile(path, "wb", mtime=0) as f:
        f.write(struct.pack("<I", len(text)) + text + buffer)


def read(path):
    """The header and buffer of an .rvr file."""
    with gzip.open(path, "rb") as f:
        raw = f.read()
    n = struct.unpack("<I", raw[:4])[0]
    return json.loads(raw[4:4 + n]), raw[4 + n:]


def span(buffer, at, kind, count):
    """A count-long array of the given kind at byte offset at."""
    return np.frombuffer(buffer, dtype(kind), count, at)


def column(header, buffer, name):
    """One column's array: NumPy for flat, Awkward for jagged."""
    data = header["data"]
    record = next(c for c in data["cols"] if c["name"] == name)
    if record["kind"] != "jagged":
        return span(buffer, record["at"], record["kind"], data["rows"])
    offsets = span(buffer, record["at"], "i8", data["rows"] + 1)
    content = record["content"]
    return ak.unflatten(span(buffer, content["at"], content["kind"], content["count"]), np.diff(offsets))
