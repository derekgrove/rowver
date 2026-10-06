"""Build, write, and read an .rvr file: a plain JSON header, then each array compressed on its own."""
import hashlib
import json
import math
import struct
import zlib

import awkward as ak
import numpy as np

VERSION = 13
KINDS = ("f8", "f4", "i8", "i4", "i2", "i1", "u8", "u4", "u2", "u1", "b1")
MAGIC = b"\x89RVR\r\n\x1a\n"
FEW = 64
LEVEL = 6


def kind(dtype):
    """The header spelling of a NumPy dtype: f4, i8, u4, b1."""
    dtype = np.dtype(dtype)
    return f"{dtype.kind}{dtype.itemsize}"


def dtype(kind):
    """The little-endian NumPy dtype of a header kind."""
    return np.dtype(bool) if kind == "b1" else np.dtype("<" + kind)


def plain(value):
    """Whether a value is text, a whole number, a finite float, a truth value, None, or a list or dict of those."""
    if isinstance(value, float):
        return math.isfinite(value)
    if value is None or isinstance(value, (str, bool, int)):
        return True
    if isinstance(value, (list, tuple)):
        return all(plain(item) for item in value)
    return isinstance(value, dict) and all(isinstance(key, str) and plain(item) for key, item in value.items())


def carried(metadata):
    """A sample's record of its metadata: the plain values, and the names of the values left out."""
    record = {"metadata": {str(key): json.loads(json.dumps(value)) for key, value in metadata.items()
                           if isinstance(key, str) and plain(value)}}
    left = [str(key) for key in metadata if str(key) not in record["metadata"]]
    return {**record, "metadata_omitted": left} if left else record


def made_checksum(made):
    """SHA-256 of the complete made section, or the derived one, excluding its own checksum."""
    body = {key: value for key, value in made.items() if key != "sha256"}
    text = json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def little(array):
    """An array's values as little-endian bytes."""
    return np.ascontiguousarray(array).astype(array.dtype.newbyteorder("<")).tobytes()


def edges(data):
    """The row each sample starts at, then the row after the last."""
    return np.concatenate([[0], np.cumsum([s["rows"] for s in data["samples"]])]).astype(np.int64)


def by_sample(array, edges, steps=False):
    """One sha256 per sample: of array[lo:hi] between neighbouring edges, or, for the steps of a
    jagged column's offsets, of array[lo:hi + 1] counted from its first value."""
    parts = [array[lo:hi + 1] - array[lo] if steps else array[lo:hi] for lo, hi in zip(edges, edges[1:])]
    return [hashlib.sha256(little(part)).hexdigest() for part in parts]


def column_checksum(record):
    """One sha256 for a column: its array's, or for a jagged column that of its offsets' and content's."""
    if record["kind"] != "jagged":
        return record["sha256"]
    return hashlib.sha256((record["offsets"]["sha256"] + record["content"]["sha256"]).encode()).hexdigest()


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


def shuffle(raw, width):
    """Byte-shuffle: every value's first byte, then every value's second byte, and so on."""
    return raw if width == 1 else np.frombuffer(raw, np.uint8).reshape(-1, width).T.tobytes()


def unshuffle(raw, width):
    """Undo shuffle."""
    return raw if width == 1 else np.frombuffer(raw, np.uint8).reshape(width, -1).T.tobytes()


def blocks(data):
    """Every array of the data section in file order: (record, kind, count, label)."""
    yield data["sample"], "i4", data["rows"], "sample"
    yield data["entry"], "i8", data["rows"], "entry"
    for c in data["cols"]:
        if c["kind"] == "jagged":
            yield c["offsets"], "i8", data["rows"] + 1, f"{c['name']} offsets"
            yield c["content"], c["content"]["kind"], c["content"]["count"], f"{c['name']} content"
        else:
            yield c, c["kind"], data["rows"], c["name"]


def data_checksum(data):
    """SHA-256 of every array's sha256, in file order, as one ASCII string."""
    return hashlib.sha256("".join(record["sha256"] for record, *_ in blocks(data)).encode()).hexdigest()


def pack(samples, labels, inputs):
    """The data section and the compressed arrays of a skim.

    samples is {sample: {"columns": {...}, "entry": array}}; inputs lists the files read, in row
    order, as {"sample", "path", "rows"}.
    """
    rows = [len(s["entry"]) for s in samples.values()]
    columns = {name: join([s["columns"][name] for s in samples.values()])
               for name in next(iter(samples.values()))["columns"]}
    return build([{"name": name, "rows": r} for name, r in zip(samples, rows)], inputs,
                 np.repeat(np.arange(len(samples), dtype="<i4"), rows),
                 np.concatenate([s["entry"] for s in samples.values()]), columns, labels)


def build(samples, inputs, sample, entry, columns, labels):
    """The data section and the compressed arrays of whole arrays: the samples' names and rows, the
    files read, each row's sample index and entry number, and {name: column} over every row."""
    data = {"rows": len(entry), "samples": samples, "files": inputs}
    rows, arrays, end = edges(data), [], [0]

    def put(array, bounds=None, steps=False):
        """Compress one array after the ones before it; return its at, size and sha256, and with
        the samples' bounds its by_sample."""
        raw = little(array)
        packed = zlib.compress(shuffle(raw, array.dtype.itemsize), LEVEL)
        record = {"at": end[0], "size": len(packed), "sha256": hashlib.sha256(raw).hexdigest()}
        if bounds is not None:
            record["by_sample"] = by_sample(array, bounds, steps)
        arrays.append(packed)
        end[0] += len(packed)
        return record

    data["sample"] = put(sample.astype("<i4"))
    data["entry"] = put(entry.astype("<i8"), rows)
    data["cols"] = []
    for name, array in columns.items():
        if isinstance(array, np.ndarray):
            data["cols"].append({"name": name, "kind": kind(array.dtype), **put(array, rows),
                                 **facts(array, labels.get(name))})
        else:
            offsets = np.concatenate([[0], np.cumsum(ak.to_numpy(ak.num(array)))]).astype("<i8")
            content = ak.to_numpy(ak.flatten(array))
            data["cols"].append({"name": name, "kind": "jagged", "offsets": put(offsets, rows, True),
                                 "content": {"kind": kind(content.dtype), "count": len(content),
                                             **put(content, offsets[rows])}})
    data["sha256"] = data_checksum(data)
    return data, arrays


def write(path, header, arrays):
    """Write the magic, the header's length and text, then the arrays, as one .rvr file."""
    text = json.dumps(header).encode()
    with open(path, "wb") as f:
        f.write(MAGIC + struct.pack("<I", len(text)) + text)
        for packed in arrays:
            f.write(packed)


def header_of(raw, path):
    """The header at the front of an .rvr file's bytes, and where the arrays start; refused unless
    the file starts with the magic and says rowver 13."""
    if raw[:8] != MAGIC:
        raise ValueError(f"{path} is not an .rvr file of rowver {VERSION} (its first bytes are not the magic)")
    n = struct.unpack("<I", raw[8:12])[0]
    header = json.loads(bytes(raw[12:12 + n]))
    if header.get("rowver") != VERSION:
        raise ValueError(f"{path} says rowver {header.get('rowver')}; this reads rowver {VERSION} only")
    return header, 12 + n


def read_header(path):
    """Only the header of an .rvr file, without reading its arrays."""
    with open(path, "rb") as f:
        start = f.read(12)
        return header_of(start + f.read(struct.unpack("<I", start[8:12])[0] if len(start) == 12 else 0), path)[0]


def read(path):
    """The header of an .rvr file and its arrays, still compressed; refused unless the arrays fill the
    rest of the file exactly."""
    with open(path, "rb") as f:
        raw = f.read()
    header, start = header_of(raw, path)
    section = memoryview(raw)[start:]
    if sum(record["size"] for record, *_ in blocks(header["data"])) != len(section):
        raise ValueError(f"{path}: the arrays do not fill the file after the header")
    return header, section


def decode(section, record, width):
    """One array's uncompressed little-endian bytes."""
    return unshuffle(zlib.decompress(section[record["at"]:record["at"] + record["size"]]), width)


def array(section, record, kind, count):
    """One array, decompressed: count values of the given kind."""
    return np.frombuffer(decode(section, record, dtype(kind).itemsize), dtype(kind), count)


def column(header, section, name):
    """One column's array: NumPy for flat, Awkward for jagged."""
    data = header["data"]
    record = next(c for c in data["cols"] if c["name"] == name)
    if record["kind"] != "jagged":
        return array(section, record, record["kind"], data["rows"])
    offsets = array(section, record["offsets"], "i8", data["rows"] + 1)
    content = record["content"]
    return ak.unflatten(array(section, content, content["kind"], content["count"]), np.diff(offsets))
