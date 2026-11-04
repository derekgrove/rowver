"""Random expressions of the recipe language, and what the README's rules give for each, worked
out here with NumPy (and plain Python where the README differs from NumPy): the reference for
tests/properties.rs, which evaluates each with rvr-recipe and compares kind, shape and bits.

    mamba run -n rowver python tests/generate_recipe_properties.py [CASES] [OUT]

writes CASES expressions (1,000 by default) to OUT (tests/fixtures/properties.json). The cases use
only what the README defines to the bit without a math library: arithmetic, comparisons, logic,
abs, sign, sqrt, floor, ceil, round, min, max, isfinite, if, bit_and, shift_right, stored,
masks and the list reductions; exp, log, trigonometry, ** and hypot come from libm and are the
hand-written fixture's. A case whose values the recipe cannot take records `error` instead.
"""
import json
import math
import pathlib
import sys

import numpy as np

EVENTS = 20
KINDS = ["f8", "f4", "i8", "i4", "i2", "i1", "u8", "u4", "u2", "u1", "b1"]
WHOLE = ["i8", "i4", "i2", "i1", "u8", "u4", "u2", "u1"]


class Refused(Exception):
    """The recipe refuses this expression (as written, before any value): not a case."""


# Set when a value the recipe cannot take was met (README "Values the recipe cannot take"): the
# case is then a data error, once every kind has been checked (a refusal of the expression comes first).
MET = []


def met():
    """Note a data error and go on, with values of the right kind and shape."""
    MET.append(True)


class SplitMix:
    """SplitMix64, as the Rust tests draw."""

    def __init__(self, seed):
        self.state = seed % 2**64

    def next(self):
        self.state = (self.state + 0x9E3779B97F4A7C15) % 2**64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) % 2**64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) % 2**64
        return z ^ (z >> 31)

    def below(self, n):
        return self.next() % n if n else 0

    def one(self, items):
        return items[self.below(len(items))]


def dtype(kind):
    return np.dtype("bool" if kind == "b1" else kind)


def edges(kind):
    if kind == "b1":
        return [False, True]
    if kind[0] == "f":
        return [float("nan"), float("inf"), -float("inf"), -0.0, 0.0, 1.0, -1.5, 0.5, 2.5, 3.5, 1e-300, 1e30]
    info = np.iinfo(dtype(kind))
    return [info.min, info.max, 0, 1, -1 if info.min < 0 else 2, 7]


def column(rng, kind, n):
    """`n` values of `kind`, half the kind's edges."""
    values = []
    for _ in range(n):
        if rng.below(2) == 0:
            values.append(rng.one(edges(kind)))
        elif kind == "b1":
            values.append(rng.below(2) == 1)
        elif kind[0] == "f":
            values.append((rng.below(2001) - 1000) / 8)
        else:
            info = np.iinfo(dtype(kind))
            values.append(int(info.min) + rng.below(int(info.max) - int(info.min) + 1))
    return np.array(values, dtype=dtype(kind))


class V:
    """A value: per event (`flat`), a list per event (`counts` and the members), or fixed (weak)."""

    def __init__(self, data, kind, counts=None, fixed=False):
        self.data, self.kind, self.counts, self.fixed = data, kind, counts, fixed


def inputs(rng):
    """The columns: one of every kind per event, and lists of five kinds sharing one count per event."""
    counts = np.array([rng.below(5) for _ in range(EVENTS)])
    counts[:3] = [0, 1, 3]
    held = {f"x_{k}": V(column(rng, k, EVENTS), k) for k in KINDS}
    for k in ["f4", "f8", "i4", "u2", "b1"]:
        held[f"L_{k}"] = V(column(rng, k, int(counts.sum())), k, counts)
    return held


def combine(a, b):
    """The kind two values combine in (README "Two kinds together"; a fixed value weak beside a column)."""
    if a.fixed and b.fixed:
        raise Refused("fixed with fixed")
    if a.fixed or b.fixed:
        fixed, other = (a, b) if a.fixed else (b, a)
        if other.kind == "b1":
            return fixed.kind
        if other.kind in WHOLE and fixed.kind == "f8":
            return "f8"
        return other.kind
    return kind_of(np.result_type(dtype(a.kind), dtype(b.kind)))


def kind_of(dt):
    return "b1" if dt == np.bool_ else f"{dt.kind}{dt.itemsize}".replace("f2", "f4")


def float_kind(kind):
    return "f4" if kind in ["b1", "i1", "u1", "i2", "u2", "f4"] else "f8"


def broadcast(*values):
    """The values, every one per member when any is a list (the lists' counts the same)."""
    lists = [v.counts for v in values if v.counts is not None]
    if not lists:
        return [v.data for v in values], None
    counts = lists[0]
    if any(not np.array_equal(c, counts) for c in lists):
        met()
        values = [V(np.zeros(int(counts.sum()), dtype=dtype(v.kind)), v.kind, counts) if v.counts is not None else v for v in values]
    out = []
    for v in values:
        if v.counts is not None:
            out.append(v.data)
        elif v.fixed:
            out.append(v.data)
        else:
            out.append(np.repeat(v.data, counts))
    return out, counts


def cast(v, kind, data):
    """`data` of the value `v` brought to `kind` (a fixed whole number refused when it does not fit)."""
    if v.fixed and kind in WHOLE:
        info = np.iinfo(dtype(kind))
        if not (info.min <= int(data) <= info.max):
            raise Refused("does not fit")
        return np.array(int(data), dtype=dtype(kind))
    return np.asarray(data).astype(dtype(kind))


def wrap(values, kind):
    """Python ints brought back to a whole kind by wrapping."""
    bits = int(kind[1]) * 8
    out = []
    for x in values:
        x %= 2**bits
        if kind[0] == "i" and x >= 2 ** (bits - 1):
            x -= 2**bits
        out.append(x)
    return np.array(out, dtype=dtype(kind))


def arithmetic(op, a, b):
    if a.kind == "b1" and b.kind == "b1":
        raise Refused("arithmetic on two b1")
    kind = combine(a, b)
    if kind == "b1":
        raise Refused("b1")
    (x, y), counts = broadcast(a, b)
    x, y = np.broadcast_arrays(cast(a, kind, x), cast(b, kind, y))
    if op == "/" and kind in WHOLE:
        with np.errstate(all="ignore"):
            return V(np.divide(x.astype("f8"), y.astype("f8")), "f8", counts)
    if kind in WHOLE:
        xi, yi = [int(v) for v in x], [int(v) for v in y]
        if op == "+":
            r = [p + q for p, q in zip(xi, yi)]
        elif op == "-":
            r = [p - q for p, q in zip(xi, yi)]
        elif op == "*":
            r = [p * q for p, q in zip(xi, yi)]
        else:
            r = [p % q if q else 0 for p, q in zip(xi, yi)]
        return V(wrap(r, kind), kind, counts)
    f = dtype(kind)
    xd, yd = x.astype("f8"), y.astype("f8")
    with np.errstate(all="ignore"):
        if op == "+":
            r = xd + yd
        elif op == "-":
            r = xd - yd
        elif op == "*":
            r = xd * yd
        elif op == "/":
            r = xd / yd
        else:
            r = np.array([remainder(p, q) for p, q in zip(xd, yd)])
    with np.errstate(all="ignore"):
        return V(r.astype(f), kind, counts)


def remainder(x, y):
    """Python's %, as the README says: the exact fmod, y added to a nonzero r of the other sign, a zero r y's sign; y = 0 NaN."""
    if y == 0 or math.isnan(x) or math.isnan(y) or math.isinf(x):
        return float("nan")
    if math.isinf(y):
        r = x
    else:
        r = math.fmod(x, y)
    if r != 0 and (r < 0) != (y < 0):
        r += y
    if r == 0:
        r = math.copysign(0.0, y)
    return r


def ints(v, counts):
    """A whole value's numbers as Python ints, per event or per member."""
    if v.fixed:
        return [int(v.data)] * (EVENTS if counts is None else int(np.sum(counts)))
    data = v.data if v.counts is not None or counts is None else np.repeat(v.data, counts)
    return [int(x) for x in data]


def comparison(op, a, b):
    kind = combine(a, b)
    (x, y), counts = broadcast(a, b)
    whole = lambda v: v.kind in WHOLE + ["b1"] and not (v.fixed and v.kind == "f8")
    if whole(a) and whole(b):
        # Exactly, each in its own kind; a fixed one must fit the other's kind.
        for fixed, other in [(a, b), (b, a)]:
            if fixed.fixed and other.kind in WHOLE:
                cast(fixed, other.kind, fixed.data)
        r = [compare(op, p, q) for p, q in zip(ints(a, counts), ints(b, counts))]
        return V(np.array(r, dtype=bool), "b1", counts)
    x, y = np.broadcast_arrays(cast(a, kind, x), cast(b, kind, y))
    r = [compare(op, float(p), float(q)) for p, q in zip(x.tolist(), y.tolist())]
    return V(np.array(r, dtype=bool), "b1", counts)


def compare(op, p, q):
    if isinstance(p, float) and (math.isnan(p) or math.isnan(q)):
        return op == "!="
    return {"==": p == q, "!=": p != q, "<": p < q, "<=": p <= q, ">": p > q, ">=": p >= q}[op]


def logic(op, a, b=None):
    values = [a] if b is None else [a, b]
    if any(v.kind != "b1" or v.fixed for v in values):
        raise Refused("logic of other kinds")
    if b is None:
        return V(~a.data, "b1", a.counts)
    (x, y), counts = broadcast(a, b)
    return V(x & y if op == "and" else x | y, "b1", counts)


def unary(name, a):
    if a.fixed:
        raise Refused("fixed")
    x, k = np.asarray(a.data), a.kind
    if name == "neg":
        if k == "b1":
            raise Refused("-b1")
        if k in WHOLE:
            return V(wrap([-int(v) for v in x], k), k, a.counts)
        return V(-x, k, a.counts)
    if name == "abs":
        if k in WHOLE:
            return V(wrap([abs(int(v)) for v in x], k), k, a.counts)
        if k == "b1":
            return V(x.copy(), k, a.counts)
        return V(np.abs(x), k, a.counts)
    if name == "sign":
        if k == "b1":
            return V(x.copy(), k, a.counts)
        if k in WHOLE:
            return V(np.array([(v > 0) - (v < 0) for v in map(int, x)], dtype=dtype(k)), k, a.counts)
        r = [float("nan") if math.isnan(v) else 1.0 if v > 0 else -1.0 if v < 0 else 0.0 for v in x.astype("f8")]
        return V(np.array(r, dtype=dtype(k)), k, a.counts)
    if name == "sqrt":
        f = float_kind(k)
        with np.errstate(all="ignore"):
            return V(np.sqrt(x.astype("f8")).astype(dtype(f)), f, a.counts)
    if name in ["floor", "ceil", "round"]:
        if k == "b1":
            if name == "round":
                raise Refused("round of b1")
            return V(x.copy(), k, a.counts)
        if k in WHOLE:
            return V(x.copy(), k, a.counts)
        fn = {"floor": np.floor, "ceil": np.ceil, "round": np.rint}[name]
        return V(fn(x.astype("f8")).astype(dtype(k)), k, a.counts)
    if name == "isfinite":
        if k[0] != "f":
            return V(np.ones(len(x), dtype=bool), "b1", a.counts)
        return V(np.isfinite(x), "b1", a.counts)
    raise ValueError(name)


def extreme(name, a, b):
    """min or max: NaN when either is (x when both), else x when less (greater), else y."""
    kind = combine(a, b)
    (x, y), counts = broadcast(a, b)
    x, y = np.broadcast_arrays(cast(a, kind, x), cast(b, kind, y))
    out = []
    for p, q in zip(x.tolist(), y.tolist()):
        if isinstance(p, float) and math.isnan(p):
            out.append(p)
        elif isinstance(q, float) and math.isnan(q):
            out.append(q)
        elif (p < q) if name == "min" else (p > q):
            out.append(p)
        else:
            out.append(q)
    return V(np.array(out, dtype=dtype(kind)), kind, counts)


def choose(c, yes, no):
    if c.kind != "b1" or c.fixed:
        raise Refused("condition")
    kind = combine(yes, no)
    (cond, y, n), counts = broadcast(c, yes, no)
    y, n = cast(yes, kind, y), cast(no, kind, n)
    cond, y, n = np.broadcast_arrays(cond, y, n)
    return V(np.where(cond, y, n).astype(dtype(kind)), kind, counts)


def bit_and(a, mask):
    if a.kind not in WHOLE or a.fixed:
        raise Refused("bit_and")
    info = np.iinfo(dtype(a.kind))
    if not info.min <= mask <= info.max:
        raise Refused("mask")
    return V(a.data & np.array(mask, dtype=dtype(a.kind)), a.kind, a.counts)


def shift_right(a, n):
    if a.kind not in WHOLE or a.fixed:
        raise Refused("shift")
    info = np.iinfo(dtype(a.kind))
    if n > info.max:
        raise Refused("n")
    if n >= 64:
        return V(np.array([-1 if int(v) < 0 else 0 for v in a.data], dtype=dtype(a.kind)), a.kind, a.counts)
    bits = int(a.kind[1]) * 8
    out = []
    for v in map(int, a.data):
        out.append(v >> n if a.kind[0] == "i" else (v % 2**bits) >> n)
    return V(np.array(out, dtype=dtype(a.kind)), a.kind, a.counts)


def stored(a, kind):
    if a.fixed:
        raise Refused("stored of fixed")
    out = []
    for v in a.data.tolist():
        if kind == "b1":
            if isinstance(v, float) and math.isnan(v):
                met()
            out.append(v != 0)
        elif kind in WHOLE:
            if isinstance(v, float):
                if math.isnan(v) or math.isinf(v):
                    met()
                    v = 0
                v = math.trunc(v)
            v = int(v)
            info = np.iinfo(dtype(kind))
            if not info.min <= v <= info.max:
                met()
                v = 0
            out.append(v)
        else:
            out.append(v)
    if kind[0] == "f" and a.kind in WHOLE:
        # Straight to the kind, rounded once (NumPy's int to float casts round once).
        return V(np.asarray(a.data).astype(dtype(kind)), kind, a.counts)
    return V(np.array(out, dtype=dtype(kind)), kind, a.counts)


def reduce(name, a, otherwise=None):
    if a.counts is None or a.fixed:
        raise Refused("not a list")
    starts = np.concatenate([[0], np.cumsum(a.counts)])
    lists = [a.data[starts[e]:starts[e + 1]] for e in range(EVENTS)]
    if name == "count":
        return V(np.array([int(np.sum(l)) if a.kind == "b1" else len(l) for l in lists], dtype="i8"), "i8")
    if name in ["any", "all"]:
        if a.kind != "b1":
            raise Refused("any of numbers")
        return V(np.array([(bool(l.any()) if name == "any" else bool(l.all())) for l in lists], dtype=bool), "b1")
    if name == "sum":
        if a.kind[0] == "f":
            out = []
            for l in lists:
                s = np.array(0, dtype=dtype(a.kind))
                for v in l:
                    with np.errstate(all="ignore"):
                        s = (s.astype("f8") + np.float64(v)).astype(dtype(a.kind))
                out.append(s)
            return V(np.array(out, dtype=dtype(a.kind)), a.kind)
        kind = "u8" if a.kind[0] == "u" else "i8"
        return V(wrap([sum(int(v) for v in l) for l in lists], kind), kind)
    # least, greatest, first: the kind of the list with otherwise beside it.
    kind = combine(a, otherwise)
    out = []
    for l in lists:
        if len(l) == 0:
            out.append(otherwise.data)
            continue
        if name == "first":
            out.append(l[0])
            continue
        best = l[0]
        for v in l[1:]:
            if isinstance(best, float) and math.isnan(best):
                break
            if isinstance(v, (float, np.floating)) and math.isnan(v):
                continue
            if (v < best) if name == "least" else (v > best):
                best = v
        out.append(best)
    held = []
    for v in out:
        held.append(cast(otherwise, kind, v) if v is otherwise.data else np.array(v).astype(dtype(kind)))
    return V(np.array([h.item() for h in held], dtype=dtype(kind)), kind)


def masked(a, mask):
    if a.counts is None or mask.counts is None or mask.kind != "b1":
        raise Refused("mask")
    if not np.array_equal(a.counts, mask.counts):
        met()
        return V(a.data.copy(), a.kind, a.counts)
    starts = np.concatenate([[0], np.cumsum(a.counts)])
    counts = np.array([int(np.sum(mask.data[starts[e]:starts[e + 1]])) for e in range(EVENTS)])
    return V(a.data[mask.data], a.kind, counts)


def literal(rng):
    if rng.below(2) == 0:
        return ("whole", rng.one([0, 1, 2, 3, 7, 100, -1, -5]))
    return ("float", rng.one(["0.5", "2.5", "-1.25", "0.1", "1e3", "3.0"]))


UNARY = ["neg", "abs", "sign", "sqrt", "floor", "ceil", "round", "isfinite"]


def gen(rng, names, depth):
    """A random expression tree."""
    if depth == 0 or rng.below(4) == 0:
        return literal(rng) if rng.below(5) == 0 else ("name", rng.one(names))
    sub = lambda: gen(rng, names, depth - 1)
    form = rng.below(13)
    if form <= 2:
        return ("arith", rng.one(["+", "-", "*", "/", "%"]), sub(), sub())
    if form == 3:
        return ("compare", rng.one(["==", "!=", "<", "<=", ">", ">="]), sub(), sub())
    if form == 4:
        op = rng.one(["and", "or", "not"])
        return ("not", sub()) if op == "not" else ("logic", op, sub(), sub())
    if form == 5:
        return ("unary", rng.one(UNARY), sub())
    if form == 6:
        return ("extreme", rng.one(["min", "max"]), sub(), sub())
    if form == 7:
        return ("if", sub(), sub(), sub())
    if form == 8:
        return ("bit_and", sub(), rng.one([0, 1, 3, 255, 1 << 40]))
    if form == 9:
        return ("shift_right", sub(), rng.one([0, 1, 3, 7, 31, 63, 64, 100]))
    if form == 10:
        return ("stored", sub(), rng.one(KINDS))
    if form == 11:
        return ("mask", sub(), sub())
    name = rng.one(["count", "sum", "any", "all", "least", "greatest", "first"])
    return ("reduce", name, sub(), literal(rng) if name in ["least", "greatest", "first"] else None)


def text(node):
    """An expression tree as a recipe writes it."""
    tag = node[0]
    if tag == "whole":
        return str(node[1])
    if tag in ["float", "name"]:
        return node[1]
    if tag in ["arith", "compare", "logic"]:
        return f"({text(node[2])} {node[1]} {text(node[3])})"
    if tag == "not":
        return f"(not {text(node[1])})"
    if tag == "unary":
        return f"-({text(node[2])})" if node[1] == "neg" else f"{node[1]}({text(node[2])})"
    if tag == "extreme":
        return f"{node[1]}({text(node[2])}, {text(node[3])})"
    if tag == "if":
        return f"if({text(node[1])}, {text(node[2])}, {text(node[3])})"
    if tag in ["bit_and", "shift_right"]:
        return f"{tag}({text(node[1])}, {node[2]})"
    if tag == "stored":
        return f"stored({text(node[1])}, '{node[2]}')"
    if tag == "mask":
        return f"{text(node[1])}[{text(node[2])}]"
    rest = f", {text(node[3])}" if node[3] else ""
    return f"{node[1]}({text(node[2])}{rest})"


def evaluate(node, held):
    """What an expression tree gives, by the README's rules; Refused otherwise (a data error is noted in MET)."""
    tag, ev = node[0], lambda n: evaluate(n, held)
    if tag == "whole":
        return V(node[1], "i8", fixed=True)
    if tag == "float":
        return V(float(node[1]), "f8", fixed=True)
    if tag == "name":
        return held[node[1]]
    if tag == "arith":
        return arithmetic(node[1], ev(node[2]), ev(node[3]))
    if tag == "compare":
        return comparison(node[1], ev(node[2]), ev(node[3]))
    if tag == "logic":
        return logic(node[1], ev(node[2]), ev(node[3]))
    if tag == "not":
        return logic("not", ev(node[1]))
    if tag == "unary":
        return unary(node[1], ev(node[2]))
    if tag == "extreme":
        return extreme(node[1], ev(node[2]), ev(node[3]))
    if tag == "if":
        return choose(ev(node[1]), ev(node[2]), ev(node[3]))
    if tag == "bit_and":
        return bit_and(ev(node[1]), node[2])
    if tag == "shift_right":
        return shift_right(ev(node[1]), node[2])
    if tag == "stored":
        return stored(ev(node[1]), node[2])
    if tag == "mask":
        return masked(ev(node[1]), ev(node[2]))
    return reduce(node[1], ev(node[2]), ev(node[3]) if node[3] else None)


def number(value):
    """A value as JSON: infinities and NaN as text."""
    if isinstance(value, float):
        if math.isnan(value):
            return "nan"
        if math.isinf(value):
            return "inf" if value > 0 else "-inf"
    return value


def record(v):
    out = {"shape": "event" if v.counts is None else "object", "kind": v.kind}
    data = np.asarray(v.data)
    if v.kind[0] == "f":
        out["values"] = [number(float(x)) for x in data.astype("f8")]
        out["bits"] = [int(x) for x in data.view("u4" if v.kind == "f4" else "u8")]
    else:
        out["values"] = [bool(x) if v.kind == "b1" else int(x) for x in data]
    if v.counts is not None:
        out["counts"] = [int(c) for c in v.counts]
    return out


def main():
    want = int(sys.argv[1]) if len(sys.argv) > 1 else 1000
    out = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else pathlib.Path(__file__).parent / "fixtures" / "properties.json"
    rng = SplitMix(2026)
    held = inputs(rng)
    cases, tries, seen = [], 0, set()
    while len(cases) < want:
        tries += 1
        seed = rng.next()
        node = gen(SplitMix(seed), sorted(held), 4)
        written = text(node)
        if node[0] == "name" or written in seen:
            continue
        MET.clear()
        try:
            value = evaluate(node, held)
        except Refused:
            continue
        if value.fixed:
            continue
        seen.add(written)
        cases.append({"expression": written, "error": True} if MET else {"expression": written, **record(value)})
    columns = {name: record(v) for name, v in held.items()}
    with open(out, "w") as f:
        f.write('{\n "numpy": ' + json.dumps(np.__version__) + ',\n "columns": {\n')
        f.write(",\n".join(f"  {json.dumps(n)}: {json.dumps(c, separators=(',', ':'))}" for n, c in columns.items()))
        f.write('\n },\n "cases": [\n' + ",\n".join("  " + json.dumps(c, separators=(",", ":")) for c in cases) + "\n ]\n}\n")
    print(f"{len(cases)} cases of {tries} tried, to {out}")


if __name__ == "__main__":
    main()
