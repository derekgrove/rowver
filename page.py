"""Write the viewer: the columns, plotly.js and the controls, as one HTML file or as two.

THE .rvr FORMAT, version 4

A gzip stream. Uncompressed it is:

    bytes 0..3      little-endian uint32: the length of the header
    bytes 4..4+L    that header, UTF-8 JSON — this is META
    bytes 4+L..     the column buffer, column-major, no padding

META:

    rowver      4                format version; a reader refuses anything else
    n           int              events in every column
    bytes       int              length of the column buffer
    datasets    [str]            sample names, in the order the dataset index uses
    ds_offset   int              byte offset of the dataset index, one int32 per event
    cols        [{...}]          one entry per column, NOT counting the weights
    weights     str | false      name of the weights column, or false for unweighted data
    w_offset    int              its byte offset, whenever weights is a name

    tabs        [{name, state}]  optional: how each tab opens — a state may carry an
                                 optional `regions` key, the region map's partition. It
                                 rides INSIDE the state, so the format did not move: a
                                 reader that knows nothing of it opens the file and shows
                                 no map. See the `regions` note below.
    tab         int              optional: which tab the page opens on
    grouping    {group, sub, order}   optional: sample to group, page-global
    colour      {name: hex}      optional: colours chosen by hand
    made_by     str              optional: the RowVer version that wrote the file
    title, path str              optional: for the masthead

Each entry of cols:

    name        str              column name
    kind        "f8" | "i4"      the only two types in the format
    offset      int              byte offset of this column in the buffer
    lim         [min, max]       the measured extremes
    nice        [lo, hi]         lim rounded outward — where the slider stops sit
    p95, p99    float            percentiles, for the default drawn range
    vals        [int] | null     the distinct values of a small integer column
    names       [str] | null     what each of those values means

`regions`, the region map's partition, inside a tab's state. The map reads the TAB's
cuts and the tab's luminosity — it is a picture of what the rail is showing, not of a
frozen copy of it. (0.18.0 briefly gave it its own `pre` selection; a file carrying one
still opens, the key is simply ignored.)

    at      int                 which block of the tab's stack the map is, like `table`
    view    "stack" | "ledger"  how a camp shows its samples
    sig     str                 the group that is S; bkg [str] is what B sums
    srcs    [str] | absent      the sources the map counts. An EMPTY list is a map nobody
                                has filled yet; ABSENT means every source the page offers,
                                so a file written before the tray reopens as it was
    root    node | null         null is one unsplit bar — the map before any cut

Each node is either null (a leaf region) or one CUT and its two sides:

    {"cut": {"col": name, "lt": x} | {"col": name, "in": [v, ...]},
     "yes": node, "no": node}

`yes` is what satisfies the predicate and `no` is EVERYTHING ELSE — the complement, never
a second comparison. That is the whole guarantee: the two sides of a cut are mutually
exclusive and exhaustive whatever the data holds, including a NaN or a value no one
anticipated, so no event can fall out of a partition and there is no leftover to store.
A many-binned variable is a chain of cuts down the `no` side; the page draws a chain on
one column as a single camp with its slices in order. `col` is a column NAME, not the
index the cuts use, so a re-skim that reorders columns fails loudly instead of quietly
repointing every cut at the wrong variable.

Two rules a writer must follow. Every f8 offset is a multiple of 8 and every i4 offset a
multiple of 4, because a browser refuses a typed array on a misaligned offset — writing
all floats before all ints guarantees it. And bool is stored as i4, since numpy cannot
take a percentile of a bool and a two-value chip picker is the right control anyway.
"""
import base64
import fnmatch
import gzip
import json
import math
import re
from pathlib import Path

import numpy as np
from plotly.offline import get_plotlyjs

TEMPLATE: Path = Path(__file__).with_name("page_template.html")
PAGES: Path = Path(__file__).with_name("pages")
VERSION: str = "0.18.0"   # the tool, shown on every page it builds; bump when the page changes
ROWVER: int = 4           # .rvr format version, which moves separately
READS: tuple = (4,)       # v2 and v3 are gone: their weights were undeclared
MAX_MB: int = 40
MAX_CHIPS: int = 25

USAGE = """usage: python page.py extract file.rvr > studies/mine/view.py

Prints a ready view.py: every tab, its cuts, plots, boards, grouping and colours.
Tune a page by hand, export the .rvr, extract it, and the next build opens the same way.
"""

#: settings that belong to one plot
PER_PLOT = {"varx", "vary", "mode", "nb", "nby", "decouple", "compare",
            "logx", "logy", "logz", "lbx", "lby", "pull", "pmin", "pmax",
            "xtitle", "ytitle", "titles", "err", "oflow", "d3style",
            "norm", "span", "layer", "layer2", "opts", "view", "cmpmode",
            "xauto", "xmin", "xmax", "yauto", "ymin", "ymax", "zauto", "zmin", "zmax",
            "cmt", "anno", "drawcol", "draw",
            "sheet", "quad", "merge", "cmton", "lines"}

#: settings the whole tab shares
SHARED = {"wt", "cms", "rlabel", "theme", "scale", "show", "sampopen", "syst", "cflog",
          "ts", "cutorder", "cutopen", "catopen", "cmsopen", "exp", "cntopen", "expopen",
          "fomopen", "drawer", "expcuts", "table", "regions"}

#: the tab settings a view.py carries through untouched
TAB_KEYS = ("lumi", "cms", "exp", "rlabel", "wt", "syst", "scale",
            "expcuts", "cutorder", "cflog", "table", "regions")

#: names that exist only in smoke.py's DOM stub, so a page using one passes the battery
#: and then dies in a real browser
STUB_ONLY = ("DOC", "OPENIDS")


def kinds(columns_by_sample: dict[str, dict[str, np.ndarray]],
          weights: str | None = "weight") -> dict[str, str]:
    """Per column, 'i4' for an integer or boolean axis, 'f8' for a continuous one."""
    first = next(iter(columns_by_sample.values()))
    is_int = lambda a: a.dtype == bool or np.issubdtype(a.dtype, np.integer)
    return {c: ("i4" if is_int(first[c]) else "f8") for c in first if c != weights}


def bump(hi: float, integer: bool = False, sig: int = 2) -> float:
    """hi truncated to `sig` significant figures then raised one unit there: 2.2998 -> 2.3."""
    if hi == 0 or not math.isfinite(hi):
        return float(hi)
    step = 10.0 ** (math.floor(math.log10(abs(hi))) - (sig - 1))
    if integer:
        step = max(1.0, round(step))
    return math.ceil(hi / step) * step


def nice_range(lo: float, hi: float, integer: bool = False) -> list[float]:
    """A slider span: the low end floored on a 1-2-2.5-5 step, the top just clear of the data."""
    if not (math.isfinite(lo) and math.isfinite(hi)) or hi <= lo:
        return [float(lo), float(hi)]
    rough_step = (hi - lo) / 8
    magnitude = 10.0 ** math.floor(math.log10(rough_step))
    step = next(magnitude * m for m in (1, 2, 2.5, 5, 10) if magnitude * m >= rough_step)
    if integer:
        step = max(1.0, round(step))
    span = [math.floor(lo / step) * step, bump(hi, integer)]
    return [float(f"{v:.12g}") for v in span]


def named(column: str, values: np.ndarray, kind: str, labels: dict | None) -> list | None:
    """The label for each distinct value of a chip column, or None if it has no names."""
    if kind != "i4" or not (labels or {}).get(column):
        return None
    present = np.unique(values)
    if len(present) > MAX_CHIPS:
        return None
    unlabelled = sorted(set(int(v) for v in present) - set(labels[column]))
    assert not unlabelled, (
        f"column {column}: no label for {unlabelled}. Every value present in the data "
        f"needs one, or a chip shows a bare number.")
    return [str(labels[column][int(v)]) for v in present]


def column_meta(name: str, values: np.ndarray, kind: str, offset: int,
                labels: dict | None) -> dict:
    """One entry of META.cols: where the column sits, and everything the controls need."""
    lo, hi = float(values.min()), float(values.max())
    distinct = np.unique(values) if kind == "i4" else ()
    return {"name": name, "kind": kind, "offset": offset,
            "lim": [lo, hi],
            "nice": nice_range(lo, hi, kind == "i4"),
            "p95": float(np.percentile(values, 95)),
            "p99": float(np.percentile(values, 99)),
            "vals": [int(v) for v in distinct] if 0 < len(distinct) <= MAX_CHIPS else None,
            "names": named(name, values, kind, labels)}


def pack(columns_by_sample: dict[str, dict[str, np.ndarray]],
         labels: dict | None = None,
         weights: str | None = "weight") -> tuple[dict[str, object], bytes]:
    """Concatenate every sample into one column-major buffer, plus the META describing it."""
    every_column = list(next(iter(columns_by_sample.values())))
    if weights is not None:
        assert weights in every_column, (
            f"the weights column is declared as {weights!r}, which this skim does not "
            f"produce — COLUMNS defines: {every_column}. Set WEIGHTS_BRANCH to one of "
            f"those, or HAS_WEIGHTS = False if these events carry no weights.")
    kind = kinds(columns_by_sample, weights)
    axes = [c for c in every_column if c != weights]
    assert axes, "a file needs at least one column that is not the weights"

    joined = {c: np.concatenate([s[c] for s in columns_by_sample.values()])
              for c in every_column}
    joined = {c: v.astype(np.int32) if v.dtype == bool else v for c, v in joined.items()}
    dataset_index = np.concatenate(
        [np.full(len(s[every_column[0]]), i, np.int32)
         for i, s in enumerate(columns_by_sample.values())])

    float_columns = ([weights] if weights is not None else []) \
        + [c for c in axes if kind[c] == "f8"]
    int_columns = [c for c in axes if kind[c] == "i4"]
    for c in int_columns:
        lo, hi = int(joined[c].min()), int(joined[c].max())
        assert -2 ** 31 <= lo and hi < 2 ** 31, (
            f"column {c} spans {lo}..{hi}, which does not fit int32. The page has no "
            f"64-bit integer array; store it as a float column, or bin it first.")

    blocks, offset, at = [], {}, 0
    for c in float_columns + int_columns:
        as_bytes = np.ascontiguousarray(
            joined[c], np.float64 if c in float_columns else np.int32)
        offset[c], at = at, at + as_bytes.nbytes
        blocks.append(as_bytes)
    ds_offset, at = at, at + dataset_index.nbytes
    blocks.append(dataset_index)

    for c in float_columns:
        assert offset[c] % 8 == 0, f"{c} starts at byte {offset[c]}, not a multiple of 8"
    for c in int_columns:
        assert offset[c] % 4 == 0, f"{c} starts at byte {offset[c]}, not a multiple of 4"
    assert ds_offset % 4 == 0

    meta = {
        "rowver": ROWVER,
        "n": int(len(dataset_index)),
        "datasets": list(columns_by_sample),
        "bytes": at,
        "ds_offset": ds_offset,
        "weights": weights if weights is not None else False,
        **({"w_offset": offset[weights]} if weights is not None else {}),
        "cols": [column_meta(c, joined[c], kind[c], offset[c], labels) for c in axes],
    }
    return meta, b"".join(block.tobytes() for block in blocks)


def page_grouping(tab_state: dict) -> dict | None:
    """A tab's grouping in the page-global shape, or None when the tab has none."""
    held = {k: tab_state[k] for k in ("group", "sub", "order") if k in tab_state}
    if not held:
        return None
    return {"group": held.get("group", {}), "sub": held.get("sub", {}),
            "order": held.get("order", [])}


def build(columns_by_sample: dict[str, dict[str, np.ndarray]],
          labels: dict | None = None,
          weights: str | None = "weight",
          tabs: list | None = None,
          extra: dict | None = None) -> tuple[dict[str, object], bytes]:
    """Pack the columns and attach the opening view — the part export() and data() share."""
    meta, buf = pack(columns_by_sample, labels, weights)
    meta["made_by"] = VERSION
    if not tabs:
        return meta, buf

    extra = extra or {}
    meta["tabs"] = tabs
    meta["tab"] = extra.get("tab", 0)
    meta.update({k: v for k, v in extra.items() if k != "tab" and v})
    grouping = page_grouping(tabs[0]["state"])
    if grouping and not meta.get("grouping"):
        meta["grouping"] = grouping
    return meta, buf


def weight_of(meta: dict) -> tuple[str | None, int]:
    """(name, byte offset) of the weights column, or (None, 0) for a file that has none."""
    assert "weights" in meta, "META has no 'weights' key — every v4 file declares one"
    return (meta["weights"] or None), int(meta.get("w_offset") or 0)


def declared_size(meta: dict) -> int:
    """The buffer size META's own declarations imply, as a check on what it claims."""
    per_event = sum(4 if c["kind"] == "i4" else 8 for c in meta["cols"])
    weights_name, _ = weight_of(meta)
    per_event += 8 if weights_name else 0
    per_event += 4                                    # the dataset index
    return per_event * meta["n"]


def read(path: str | Path) -> tuple[dict[str, object], dict[str, np.ndarray]]:
    """Read a .rvr back into (META, {column: ndarray}), validating it on the way."""
    raw = gzip.decompress(Path(path).read_bytes())
    assert len(raw) >= 4, f"{path}: too short to hold a header"
    length = int.from_bytes(raw[:4], "little")
    assert 4 + length <= len(raw), f"{path}: header claims {length} B, file has {len(raw) - 4}"

    meta = json.loads(raw[4:4 + length])
    assert meta.get("rowver") in READS, \
        f"{path}: format version {meta.get('rowver')!r}, this reader knows {list(READS)}"
    for key in ("n", "bytes", "datasets", "ds_offset", "cols"):
        assert key in meta, f"{path}: META has no {key!r}"

    buf = raw[4 + length:]
    assert len(buf) == meta["bytes"], \
        f"{path}: buffer is {len(buf)} B, META says {meta['bytes']}"

    weights_name, w_offset = weight_of(meta)
    assert declared_size(meta) == meta["bytes"], (
        f"{path}: says {'weights ' + repr(weights_name) if weights_name else 'no weights'} "
        f"and {len(meta['cols'])} columns over {meta['n']:,} events, which needs "
        f"{declared_size(meta):,} B, but the buffer declares {meta['bytes']:,} B. "
        f"The header contradicts itself.")

    columns = {}
    if weights_name:
        columns[weights_name] = np.frombuffer(buf, np.float64, meta["n"], w_offset)
    for col in meta["cols"]:
        dtype = np.float64 if col["kind"] == "f8" else np.int32
        columns[col["name"]] = np.frombuffer(buf, dtype, meta["n"], col["offset"])
    return meta, columns


def resolve(names: list, spec, subs: dict | None = None) -> list:
    """One GROUPS or SUBGROUPS value to sample names: a subgroup name, exact names, globs."""
    out = []
    for pattern in ([spec] if isinstance(spec, str) else list(spec)):
        if subs and pattern in subs:
            out += subs[pattern]
        elif any(ch in pattern for ch in "*?["):
            matched = [n for n in names if fnmatch.fnmatchcase(n, pattern)]
            assert matched, f"{pattern!r} matches no sample — have: {names}"
            out += matched
        else:
            assert pattern in names, f"no sample or subgroup {pattern!r} — have: {names}"
            out.append(pattern)
    return out


def membership(meta: dict, groups: dict | None,
               subgroups: dict | None = None) -> tuple[dict[str, str], dict[str, str]]:
    """Resolve SUBGROUPS and GROUPS into ({sample: subgroup}, {sample: group})."""
    names = meta["datasets"]

    sample_subgroup: dict[str, str] = {}
    for subgroup, spec in (subgroups or {}).items():
        for sample in resolve(names, spec):
            assert sample_subgroup.get(sample, subgroup) == subgroup, (
                f"{sample!r} is in both subgroups {sample_subgroup[sample]!r} and "
                f"{subgroup!r}. A sample has one subgroup, so that every total stays a "
                f"sum over disjoint parts.")
            sample_subgroup[sample] = subgroup

    members_of: dict[str, list] = {}
    for sample, subgroup in sample_subgroup.items():
        members_of.setdefault(subgroup, []).append(sample)

    sample_group: dict[str, str] = {}
    for group, spec in (groups or {}).items():
        for sample in resolve(names, spec, members_of):
            assert sample_group.get(sample, group) == group, (
                f"{sample!r} is in both groups {sample_group[sample]!r} and {group!r}. A "
                f"sample belongs to exactly one group, so that every total stays a sum "
                f"over disjoint parts — name the members explicitly rather than "
                f"overlapping two globs.")
            sample_group[sample] = group

    return sample_subgroup, sample_group


def cut_for(column_meta_entry: dict, value) -> tuple[str, list]:
    """One cut as the page stores it: chips on a value column, a [lo, hi] range otherwise."""
    if column_meta_entry["vals"] is not None:
        unknown = set(value) - set(column_meta_entry["vals"])
        assert not unknown, f"{column_meta_entry['name']}: no such values {sorted(unknown)}"
        return "chips", [int(v) for v in value]
    assert len(value) == 2, \
        f"{column_meta_entry['name']} is continuous, so its cut needs [lo, hi]"
    stops = column_meta_entry["nice"]
    return "cuts", [float(stops[0] if value[0] is None else value[0]),
                    float(stops[1] if value[1] is None else value[1])]


def state(meta: dict, cuts: dict | None = None, groups: dict | None = None,
          subgroups: dict | None = None, fom: list | tuple | None = None,
          shown: list | tuple | None = None, lumi: float = 1, **extra) -> dict:
    """One tab's saved state: its cuts, its grouping, its FOM pair and its plots."""
    index_of = {c["name"]: i for i, c in enumerate(meta["cols"])}
    sample_subgroup, sample_group = membership(meta, groups, subgroups)
    st = {"v": 2, "cuts": {}, "chips": {}, "show": {},
          "sub": sample_subgroup, "group": sample_group, "lumi": lumi}
    group_names = set(sample_group.values())
    if groups:
        st["order"] = [g for g in groups if g in group_names]

    if shown is not None:
        st["show"] = {d: False for d in meta["datasets"]}
        st["show"].update(dict.fromkeys(resolve(meta["datasets"], shown), True))

    if fom:
        assert len(fom) == 2, f"fom is (numerator group, denominator group), got {fom!r}"
        for g in fom:
            assert g in group_names, \
                f"fom names {g!r}, which is not a group — have: {sorted(group_names)}"
        st["fomn"], st["fomd"] = fom

    for spec in [extra] + list(extra.get("plots") or []):
        for key in ("layer", "layer2"):
            if key in spec:
                allowed = group_names | set(meta["datasets"]) | {"all"}
                assert spec[key] in allowed, \
                    f"{key}={spec[key]!r} is not a group, a sample, or 'all'"

    for column, value in (cuts or {}).items():
        assert column in index_of, f"no column {column!r} — have: {sorted(index_of)}"
        i = index_of[column]
        where, stored = cut_for(meta["cols"][i], value)
        st[where][str(i)] = stored

    plots = extra.pop("plots", None)
    unknown = set(extra) - PER_PLOT - SHARED
    assert not unknown, (f"unknown setting {sorted(unknown)} — "
                         f"per plot: {sorted(PER_PLOT)}; shared: {sorted(SHARED)}")

    st |= {k: v for k, v in extra.items() if k in SHARED}
    inherited = {k: v for k, v in extra.items() if k in PER_PLOT}
    if plots is not None:
        for i, spec in enumerate(plots):
            stray = set(spec) - PER_PLOT
            assert not stray, f"plot {i}: unknown setting {sorted(stray)}"
        st["plots"] = [dict(inherited, **spec) for spec in plots]
    elif inherited:
        st["plots"] = [inherited]
    return st


def tabs(meta: dict, view: list, groups: dict | None = None,
         subgroups: dict | None = None) -> list[dict]:
    """A study's VIEW list as one saved state per tab."""
    named_by_tab = {"name", "cuts", "fom", "shown", "lumi"}
    return [{"name": tab["name"],
             "state": state(meta, cuts=tab.get("cuts"), groups=groups, subgroups=subgroups,
                            fom=tab.get("fom"), shown=tab.get("shown"),
                            lumi=tab.get("lumi", 1),
                            **{k: v for k, v in tab.items() if k not in named_by_tab})}
            for tab in view]


def grouping_of(meta: dict, st: dict | None = None) -> tuple[dict, dict]:
    """META's grouping as (GROUPS, SUBGROUPS) — the shape a study file writes."""
    grouping = meta.get("grouping") or {k: (st or {}).get(k)
                                        for k in ("group", "sub", "order")}
    sample_group = grouping.get("group") or {}
    sample_subgroup = grouping.get("sub") or {}

    subgroups: dict = {}
    for sample, subgroup in sample_subgroup.items():
        subgroups.setdefault(subgroup, []).append(sample)

    groups: dict = {}
    for sample, group in sample_group.items():
        if sample not in sample_subgroup:
            groups.setdefault(group, []).append(sample)
    for subgroup, members in subgroups.items():
        host = sample_group.get(members[0])
        if host:
            groups.setdefault(host, []).append(subgroup)

    declared = [g for g in (grouping.get("order") or []) if g in groups]
    in_order = {g: groups[g] for g in declared}
    in_order.update({g: v for g, v in groups.items() if g not in declared})
    return in_order, subgroups


def cuts_by_name(meta: dict, st: dict) -> dict:
    """A tab's cuts keyed by column name, dropping any handle still resting at its stop."""
    out: dict = {}
    for i, span in (st.get("cuts") or {}).items():
        col = meta["cols"][int(i)]
        stop_lo, stop_hi = col["nice"] or col["lim"]
        if span[0] > stop_lo or span[1] < stop_hi:
            out[col["name"]] = [span[0], span[1]]
    for i, values in (st.get("chips") or {}).items():
        col = meta["cols"][int(i)]
        if len(values) < len(col["vals"] or ()):
            out[col["name"]] = sorted(int(v) for v in values)
    return out


def extract(path: str | Path) -> dict:
    """Every tab of a .rvr as the data a view.py holds, cuts keyed by column name."""
    meta, _ = read(path)
    first_state = ((meta.get("tabs") or [{}])[0] or {}).get("state")
    groups, subgroups = grouping_of(meta, first_state)

    view = []
    for saved in meta.get("tabs") or []:
        st = saved.get("state") or {}
        tab = {"name": saved.get("name"),
               "cuts": cuts_by_name(meta, st),
               "plots": [{k: v for k, v in p.items() if k != "id"}
                         for p in (st.get("plots") or [])]}
        if st.get("fomn") and st.get("fomd"):
            tab["fom"] = (st["fomn"], st["fomd"])
        visible = [d for d, on in (st.get("show") or {}).items() if on]
        if 0 < len(visible) < len(meta["datasets"]):
            tab["shown"] = visible
        for key in TAB_KEYS:
            if st.get(key) not in (None, ""):
                tab[key] = st[key]
        view.append(tab)

    return {"opens_on": int(meta.get("tab") or 0),
            "groups": groups, "subgroups": subgroups,
            "colour": meta.get("colour") or {},
            "locks": {k: meta[k] for k in ("lockcuts", "lockplots", "lockshow")
                      if meta.get(k)},
            "view": view}


def extract_py(path: str | Path) -> str:
    """extract() rendered as a ready view.py — `python page.py extract f.rvr`."""
    import pprint
    lifted = extract(path)
    fmt = lambda v: pprint.pformat(v, width=94, sort_dicts=False)
    lines = [f'"""Extracted from {Path(path).name} — {len(lifted["view"])} tabs."""', ""]
    for name, key in (("OPENS_ON", "opens_on"), ("SUBGROUPS", "subgroups"),
                      ("GROUPS", "groups"), ("COLOUR", "colour"), ("LOCKS", "locks")):
        if lifted[key]:
            lines.append(f"{name} = {fmt(lifted[key])}\n")
    lines.append(f"VIEW = {fmt(lifted['view'])}")
    return "\n".join(lines)


def check_template(html: str, where: str) -> None:
    """Refuse a page that reaches for something only the test harness provides."""
    for name in STUB_ONLY:
        assert not re.search(rf"\b{name}\s*[.\[]", html), (
            f"{where}: the page references {name!r}, which exists only in smoke.py's DOM "
            f"stub — it will pass the battery and throw '{name} is not defined' in a "
            f"browser. Use $(id) / document instead.")


def check_self_contained(html: str, where: str) -> None:
    """Refuse a page that would need the network, or that leaked a JS escape into CSS."""
    body = html.replace(get_plotlyjs(), "").replace("/*net-ok*/fetch(", "")
    for reference in ("<script src", "<link ", "@import", "fetch("):
        assert reference not in body, f"page is not self-contained: {reference}"
    check_template(html, where)
    leaked = re.findall(r'content\s*:\s*"[^"]*\\u[0-9a-fA-F]{4}', html)
    assert not leaked, f"a JS-style unicode escape reached a CSS content rule: {leaked[:3]}"


def viewer(name: str = "viewer", title: str = "RowVer viewer") -> dict[str, object]:
    """Write <name>.html with no data in it — drop a .rvr file onto it."""
    html = (TEMPLATE.read_text()
            .replace("/*__PLOTLY__*/", get_plotlyjs())
            .replace("__TITLE__", title)
            .replace("__VERSION__", VERSION))
    out = Path(__file__).with_name(f"{name}.html")
    check_template(html, str(out))
    out.write_text(html)
    return {"file": str(out), "mb": len(html) / 1e6}


def data(columns_by_sample: dict[str, dict[str, np.ndarray]], name: str = "rowver",
         labels: dict | None = None,
         dest: str | Path | None = None,
         weights: str | None = "weight",
         tabs: list | None = None,
         extra: dict | None = None) -> dict[str, object]:
    """Write <dest>/<name>.rvr, the data on its own, to drop onto viewer.html."""
    where = Path(dest) if dest else PAGES
    where.mkdir(parents=True, exist_ok=True)
    out = where / f"{name}.rvr"

    meta, buf = build(columns_by_sample, labels, weights, tabs, extra)
    meta["title"] = f"{name}.rvr"
    meta["path"] = str(out.resolve())
    header = json.dumps(meta).encode()
    payload = gzip.compress(len(header).to_bytes(4, "little") + header + buf, 6)
    out.write_bytes(payload)
    return {"file": str(out), "mb": len(payload) / 1e6, "events": meta["n"]}


def export(columns_by_sample: dict[str, dict[str, np.ndarray]], name: str = "rowver",
           title: str = "RowVer",
           labels: dict | None = None,
           dest: str | Path | None = None,
           weights: str | None = "weight",
           tabs: list | None = None,
           extra: dict | None = None) -> dict[str, object]:
    """Write <dest>/<name>.html, the page with its data inside — one file to send."""
    where = Path(dest) if dest else PAGES
    out = where / f"{name}.html"

    meta, buf = build(columns_by_sample, labels, weights, tabs, extra)
    meta["title"] = f"{name}.html"
    meta["path"] = str(out.resolve())
    blob = base64.b64encode(gzip.compress(buf, 6)).decode()
    mb = len(blob) / 1e6
    if mb > MAX_MB:
        raise ValueError(
            f"payload is {mb:,.0f} MB from {meta['n']:,} events — over the {MAX_MB} MB "
            f"limit. Pass fewer datasets or tighten skim_config's KEEP.")

    html = (TEMPLATE.read_text()
            .replace("/*__META__*/ null", json.dumps(meta))
            .replace("/*__BLOB__*/ null", json.dumps(blob))
            .replace("/*__PLOTLY__*/", get_plotlyjs())
            .replace("__TITLE__", title)
            .replace("__VERSION__", VERSION))
    check_self_contained(html, str(out))
    where.mkdir(parents=True, exist_ok=True)
    out.write_text(html)
    return {"file": str(out), "mb": len(html) / 1e6, "events": meta["n"], "blob_mb": mb}


if __name__ == "__main__":
    import sys
    if len(sys.argv) == 3 and sys.argv[1] == "extract":
        print(extract_py(sys.argv[2]))
    else:
        sys.exit(USAGE)
