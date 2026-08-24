"""The .rvr format, version 6 — five drawers, delta tabs, and a writer that only merges.

A clean break from 4/5: no reader here opens an older file and no writer emits one.
`page.py` still owns 4/5 and is untouched.

THE FRAME, unchanged from 5 because it is the part that was right:

    gzip( [uint32 LE header length][header JSON = META][column buffer] )

The buffer is column-major, no padding: every f8 offset a multiple of 8, every i4 a
multiple of 4, all floats before all ints, then one int32 per event saying which sample
the row came from. Native byte order, never converted — that is what makes the page's
unpack a zero-copy typed-array view instead of a decode pass.

META — FIVE DRAWERS. The drawer a key sits in IS its scope.

    rowver      6      the one key outside a drawer: check it before trusting the rest
    about       {}     what am I looking at
    data        {}     the buffer contract — THE ONLY IMMUTABLE DRAWER
    view        {}     what is true on every tab
    tabs        []     an ordered list; each entry is one page setup
    opens_on    int    which tab opens
    made        {}     where the numbers came from. Delete it and the file still opens.

`about` — four lines, so the answer is above the fold.

    name    str    the study
    title   str    free text for the masthead
    note    str    optional: what this file is, in a sentence, for whoever you send it to
    viewer  str    where to open it, so an emailed .rvr is not an unidentifiable binary

`data` — facts about the bytes. NOTHING WRITES HERE. Every number is checked against the
buffer at read time and the file refuses to open if the header contradicts itself.

    rows        int    events in every column
    bytes       int    length of the column buffer
    weight_col  str|null   name of the weights column, or null when no sample carries one
    weight_at   int    its byte offset (absent when weight_col is null)
    sample_at   int    byte offset of the int32 sample index
    samples     [{name, rows, weighted}]
                       ORDER IS LOAD-BEARING: index i here is the value stored at
                       sample_at. `weighted` false means REAL DATA — its rows are counts,
                       and no reader may ever multiply them by a luminosity. That fact
                       lives here rather than in `made`, because a reader needs it to draw
                       a correct number and `made` is optional.
    cols        [{name, kind, at, lim, nice, p95, p99, vals?}]
                       kind is "f8" or "i4", the only two types in the format. `at` is a
                       byte offset — in this drawer every `at` is a byte offset. `lim` is
                       the measured extremes, `nice` those rounded outward (where a
                       slider's end stops sit), `vals` the distinct values of a small
                       integer column, absent when it is continuous or too wide to chip.

                       There is no `names` here. What a value MEANS is a human's opinion,
                       not a property of the bytes; welding it in was why nobody could
                       ever edit it. It lives in view.cats.

`view` — page-global. Everything D4 calls "not tab dependent". EVERY KEY IS OPTIONAL and
an absent key means its neutral value, so a barebones file is `"view": {}`.

    groups   {of: {sample: group}, sub: {sample: subgroup}, order: [group]}
             RESOLVED, never globs. A glob belongs in cart.py, where python expands it;
             putting one here would make every JS reader reimplement fnmatch.
    shown    [sample]      LOCKABLE, like cuts and stage. Absent means every sample is
             drawn; an EMPTY LIST means none. Three states, and the difference moves a
             number: on one shipped study the tabs naming 13 of 23 samples draw 626 rows
             where the tab naming none draws 1,388.
    cats     {column: {names: {value: label}, bands: [{label, vals, at?}]}}
             Keyed by column NAME. `names` is what one value means; `bands` is a NAME FOR
             A SET of values, which had nowhere to live before. Both are editable in the
             page and written back here — that is the whole point of moving them out of
             `data`. `at` is an explicit rank from a drag; absent means "sit where your
             smallest member sits".
    look     {colour: {sample|group|sub: hex}, style: {...}}  typed by namespace, so a
             rename can carry its entry and an import can say exactly what missed
    lumis    {sample: {name: fb}, sub: {name: fb}, group: {name: fb}}
             TYPED BY NAMESPACE, and that is not decoration. Luminosity is set in the sample
             tool against samples, subgroups and groups (Derek, 2026-08-23), and a flat
             {name: fb} map cannot say which of the three a key meant — real v4/v5 files
             already mix them (one golden keys a GROUP and a subgroup together; the Run 2
             cross-check keys four ERAS, which are subgroups). A sample named like a group
             is then unresolvable, and no ordering rule can rescue it. Typing the keys makes
             every entry unambiguous whichever way the order below is settled.

             ⚠️ THE ORDER IS DEREK'S OPEN CALL. When a sample sits in a subgroup inside a
             group and more than one names a luminosity, does the MOST SPECIFIC win
             (sample > sub > group) or the OUTERMOST? A divergence means one .rvr reads two
             ways. The front end currently implements most-specific-wins. Nothing is written
             down here until he rules, because writing down a guess is how two readers end
             up disagreeing quietly.

             There is NO `view.lumi` scalar. A page-wide default was the thing Derek removed,
             and keeping it as a fallback would put a resolution rule in every reader — which
             is exactly how the `weighted` default went wrong. A file that needs every group
             at one luminosity says so explicitly, once, per group.
    fom      [numerator, denominator]        syst  float
    label    {exp, tag, right}               prefs {theme, ts, scale}

    cuts     PRESENT MEANS LOCKED. See THE LOCK below.
    stage    PRESENT MEANS THE PLOTS ARE LOCKED. Same rule.
    shown    PRESENT MEANS THE VISIBLE SAMPLES ARE LOCKED. Same rule again — one
             mechanism, three concerns, and a fourth costs one entry in NEUTRAL.

`tabs` — a flat ordered list. Each entry is one page setup and holds only what makes it
different. Three keys, and two of them are optional:

    name   str    required; also the folder name in a multi-tab zip export
    shown  [str]  absent means this tab draws every sample. A tab is its own world,
                  including which samples it draws (Derek, 2026-08-23).
    stage  [...]  ordered, and it IS the on-screen order: the plots, the yields board and
                  the region map as kind-tagged peers in one list. Absent means an EMPTY
                  stage — never inheritance. A fresh tab is empty on purpose.
    cuts   {}     absent means this tab does not narrow anything of its own
    plot   {}     optional: settings every plot-kind entry on this tab shares

    a stage entry:  {"kind": "1d"|"2d"|"board"|"map"|"block", ...}

    A BLOCK is up to four plots drawn on one canvas, and it is ONE stage entry with
    children rather than a tag shared between siblings:

        {"kind": "block", "cells": [{"kind": "1d", ...}, {"kind": "2d", ...}, null, {...}]}

    `cells` is ordered and POSITION IS THE SLOT — 0 top-left, 1 top-right, 2 bottom-left,
    3 bottom-right — with null for a slot left empty. One to four entries; the arrangement
    follows the count (one solo, two side by side, three or four a 2x2), so there is no
    layout key to keep in sync with the contents. `cells` holds plot kinds only, and a
    block may NOT contain a block: one level, so a stage stays something you can read.

    This is deliberately NOT v5's `sheet`/`quad`, and not just in the naming. Those were
    a group id and a slot stamped on sibling entries, which meant `sheetPlan()` gathered a
    canvas by id and the list's own order stopped being the screen's order — the exact
    defect that folding plots, boards and maps into one `stage` exists to remove. A
    container keeps list order == screen order, makes "up to four" a length rather than a
    convention, and makes deleting a block one splice instead of a sweep.

`cuts` — ONE dict, keyed by column NAME, holding both grammars:

    "MET": [200.0, null]      a range, inclusive; null on either side means open there
    "nJet": {"in": [1, 2]}    a categorical pick: only these values pass

    A name no column answers to is REFUSED, not ignored. Skipping it would silently widen
    the selection, and a silently widened locked cut is wrong on every tab at once.

THE LOCK. Locked means the concern lives in `view` instead of on the tabs. There is no
lock flag: `"cuts" in view` IS the flag. Locking moves one object, so "write the change to
every tab at once" stops being an operation and becomes an address.

A LOCK IS A SHARED TRUTH (Derek, 2026-08-23), and it is PROMOTE-AND-SHADOW, never a copy
into every tab:

    lock(k)     the tab you are looking at wins: its value is promoted into `view`.
                Nothing is deleted. Every tab keeps its own copy, dormant.
    while on    effective(tab, k) = view.get(k, tab.get(k, NEUTRAL[k])) — the shared value,
                for every tab, no exceptions and no per-tab opt-out.
    unlock(k)   `view[k]` is dropped and every tab wakes up with the value it had before.
                A tab CREATED while locked has nothing to wake up to, so it keeps a copy of
                what was on screen — never the neutral value, because dropping to "no cuts"
                would silently widen a selection nobody widened.

That is why the dormant copies live in the FILE rather than in a session variable: today's
`setLock` stashes them on a runtime object, which is why its own message has to admit
"unticking reverts every tab, but only in this session: an exported .rvr keeps what is on
screen". Under 6 unlocking behaves the same before and after a save.

⚠️ While a lock is on, a tab whose own value was deliberately different is SHOWING the
shared one, and so is every consumer that reads the file — `yields.py`, the Combine bridge.
That is correct under shared truth (the exporters must agree with the page) but it means a
locked file can compute different numbers from the same tab than an unlocked one. The
per-tab value is still in the file, so nothing is lost; it is simply not in effect.

`made` — provenance, and the only drawer you can delete and still have a working file.

    by, built            the tool and when it burned this file
    skimmed              when the columns were actually read. null means UNKNOWN, and
                         unknown is not the same as "now" — a stamp that claims a recipe
                         which never ran is the failure this key exists to prevent.
    source               optional: {file, rowver, made_by, sha256} when this file was
                         transcoded from another rather than skimmed
    keep, prepare, columns, cart, merged     as in 5, all optional, all nullable

THE WRITER CONTRACT. The browser writes every drawer but `data`, so the format's safety
is not a property of the shape — it is a property of the writers. Four rules:

    W1  MERGE, NEVER REBUILD. No writer may assign a whole drawer, a whole tab or a whole
        stage item from a fresh object literal. That is the one and only way keys get
        silently deleted, and it is measurably what happens in 5: a probe key injected
        into a tab's state or into `grouping` does not survive a browser save, because
        collectState() and groupingDict() build literals.
    W2  DELETE IS EXPLICIT. A key leaves only through a named delete, at a call site, in
        response to something the user did.
    W3  `data` IS NEVER WRITTEN, and the column buffer is passed through as the same bytes
        that were read — never decoded, never re-encoded.
    W4  UNKNOWN KEYS ARE NOT A CASE. Survival is the default rather than a feature,
        because no code path enumerates META to decide what to keep.

`write()` below takes the meta you loaded and hands it back out; `demo()` pins W1 and W3.
"""
from __future__ import annotations

import gzip
import hashlib
import json
from pathlib import Path

ROWVER: int = 6
READS: tuple = (6,)          # a clean break: 4 and 5 belong to page.py
VERSION: str = "0.35.0-v6"
VIEWER: str = "https://rowver.web.cern.ch"
MAX_CHIPS: int = 25

#: See THE LOCK above. A lock is a shared TRUTH (Derek, 2026-08-23): while it is on, the
#: shared value is what every tab shows, with no per-tab opt-out.
LOCK_WINS: bool = True

DRAWERS = ("about", "data", "view", "tabs", "opens_on", "made")

#: the concerns a lock can move between a tab and `view`, and what "not set" means for each.
#: `shown` is None rather than [] on purpose: absent means every sample is drawn, while an
#: EMPTY LIST means draw none. Three states, as with a region map's `srcs`.
NEUTRAL = {"cuts": {}, "stage": [], "shown": None}
LOCKABLE = tuple(NEUTRAL)


def frame(meta: dict, buf: bytes) -> bytes:
    """The uncompressed file: the header's length, the header, then the columns."""
    header = json.dumps(meta, allow_nan=False).encode()
    return len(header).to_bytes(4, "little") + header + buf


def unframe(raw: bytes) -> tuple[dict, bytes]:
    """Split an uncompressed file back into (META, buffer)."""
    assert len(raw) >= 4, "too short to hold a header"
    length = int.from_bytes(raw[:4], "little")
    assert 4 + length <= len(raw), f"header claims {length} B, file has {len(raw) - 4}"
    return json.loads(raw[4:4 + length]), raw[4 + length:]


def declared_size(data: dict) -> int:
    """The buffer size `data`'s own declarations imply, as a check on what it claims."""
    per_event = sum(4 if c["kind"] == "i4" else 8 for c in data["cols"])
    per_event += 8 if data.get("weight_col") else 0
    return (per_event + 4) * data["rows"]          # + the int32 sample index


def check(meta: dict, buf: bytes) -> None:
    """Refuse a file whose header disagrees with its own bytes, before anything reads them."""
    assert meta.get("rowver") in READS, \
        f"format version {meta.get('rowver')!r}, this reader knows {list(READS)}"
    for drawer in ("about", "data"):
        assert isinstance(meta.get(drawer), dict), f"META has no {drawer!r} drawer"
    d = meta["data"]
    for key in ("rows", "bytes", "sample_at", "samples", "cols"):
        assert key in d, f"data has no {key!r}"
    assert len(buf) == d["bytes"], f"buffer is {len(buf)} B, data says {d['bytes']}"
    assert declared_size(d) == d["bytes"], (
        f"data says {len(d['cols'])} columns "
        f"{'plus weights ' if d.get('weight_col') else ''}over {d['rows']:,} events, which "
        f"needs {declared_size(d):,} B, but declares a buffer of {d['bytes']:,} B. "
        f"The header contradicts itself.")
    assert sum(s["rows"] for s in d["samples"]) == d["rows"], (
        f"the samples sum to {sum(s['rows'] for s in d['samples']):,} rows, "
        f"data says {d['rows']:,}")
    # absent is not a third state: whichever way a reader defaults it, the other reader
    # scales that sample by a luminosity the first one does not, and both open the file
    for s in d["samples"]:
        assert isinstance(s.get("weighted"), bool), (
            f"sample {s.get('name')!r} does not say whether it is weighted. There is no "
            f"default that is safe — true scales real data by a luminosity, false stops "
            f"scaling MC — so the key is required.")
    seen = set()
    for c in d["cols"]:
        width = 4 if c["kind"] == "i4" else 8
        assert c["at"] % width == 0, \
            f"column {c['name']} starts at byte {c['at']}, not a multiple of {width}"
        assert c["at"] + width * d["rows"] <= d["bytes"], \
            f"column {c['name']} runs past the end of the buffer"
        assert c["name"] not in seen, (
            f"two columns are called {c['name']!r}. A name-keyed cut cannot say which it "
            f"means, and a reader building a dict by name silently keeps the last one.")
        seen.add(c["name"])
    # the docstring promises this and nothing was enforcing it: a cut naming a column that
    # is not here would be skipped, which WIDENS the selection instead of failing
    for where, holder in [("view", meta.get("view") or {}),
                          *((f"tab {i} ({t.get('name')!r})", t)
                            for i, t in enumerate(meta.get("tabs") or []))]:
        stray = sorted(set(holder.get("cuts") or {}) - seen)
        assert not stray, (
            f"{where} cuts on {stray}, which this file has no column for. Skipping it "
            f"would silently widen the selection — rename the cut or re-skim.")
    tabs = meta.get("tabs") or []
    assert 0 <= int(meta.get("opens_on") or 0) < max(len(tabs), 1), \
        f"opens_on is {meta.get('opens_on')} but the file has {len(tabs)} tabs"


def read(path: str | Path) -> tuple[dict, bytes]:
    """Read a v6 file into (META, the column buffer), validating it on the way."""
    meta, buf = unframe(gzip.decompress(Path(path).read_bytes()))
    check(meta, buf)
    return meta, buf


def columns(meta: dict, buf: bytes) -> dict:
    """The buffer as {column name: ndarray}, zero-copy views over the bytes as they lie."""
    import numpy as np
    d = meta["data"]
    out = {}
    if d.get("weight_col"):
        out[d["weight_col"]] = np.frombuffer(buf, np.float64, d["rows"], d["weight_at"])
    for c in d["cols"]:
        kind = np.float64 if c["kind"] == "f8" else np.int32
        out[c["name"]] = np.frombuffer(buf, kind, d["rows"], c["at"])
    return out


def write(path: str | Path, meta: dict, buf: bytes) -> dict:
    """Burn a v6 file. W1/W3: `meta` is handed back out, not rebuilt; `buf` is passed through."""
    check(meta, buf)
    payload = gzip.compress(frame(meta, buf), 6)
    Path(path).write_bytes(payload)
    return {"file": str(path), "mb": len(payload) / 1e6, "rows": meta["data"]["rows"]}


def cut_of(spec) -> str:
    """One cut in the format's grammar, as words — the shape both readers must agree on."""
    if isinstance(spec, dict):
        return "in " + ", ".join(str(v) for v in spec["in"])
    lo, hi = spec
    if lo is None:
        return f"<= {hi:g}"
    if hi is None:
        return f">= {lo:g}"
    return f"{lo:g} .. {hi:g}"


def effective(tab: dict, view: dict, key: str, neutral=None):
    """What this tab actually shows. A lock is a shared truth, so `view` wins while it is on."""
    if neutral is None:
        neutral = NEUTRAL.get(key)
    if LOCK_WINS:
        return view.get(key, tab.get(key, neutral))
    return tab.get(key, view.get(key, neutral))


def locked(view: dict, key: str) -> bool:
    """Presence IS the flag — and presence, not truthiness: a locked empty dict is a state."""
    return key in view


def lock(meta: dict, key: str, from_tab: int = 0) -> dict:
    """Promote one tab's value to the shared truth. Nothing is deleted; every tab keeps its own."""
    assert key in LOCKABLE, f"{key!r} is not lockable — have: {list(LOCKABLE)}"
    src = meta["tabs"][from_tab] if meta["tabs"] else {}
    meta["view"][key] = _clone(src.get(key, NEUTRAL[key]))   # merge, never rebuild (W1)
    return meta


def unlock(meta: dict, key: str) -> dict:
    """Hand every tab back what it had. A tab born under the lock keeps what was on screen."""
    if not locked(meta["view"], key):
        return meta
    shared = meta["view"].pop(key)                    # the explicit delete (W2)
    for tab in meta["tabs"]:
        tab.setdefault(key, _clone(shared))           # never drops to neutral: that would widen
    return meta


def _clone(o):
    """A value that shares nothing with the one it came from."""
    return json.loads(json.dumps(o))


def compact(o, indent: int = 0, width: int = 88) -> str:
    """Pretty JSON that keeps any value fitting on one line on one line."""
    pad = " " * indent
    flat = json.dumps(o, separators=(", ", ": "), ensure_ascii=False)
    if len(flat) + indent <= width or not isinstance(o, (dict, list)):
        return flat
    if isinstance(o, list):
        return ("[\n" + ",\n".join(pad + "  " + compact(v, indent + 2, width) for v in o)
                + "\n" + pad + "]")
    return ("{\n" + ",\n".join(f"{pad}  {json.dumps(k, ensure_ascii=False)}: "
                               f"{compact(v, indent + 2, width)}" for k, v in o.items())
            + "\n" + pad + "}")


def show(path: str | Path) -> str:
    """What a .rvr holds, for a human — the thing to run instead of dumping the JSON."""
    meta, buf = read(path)
    d, view, tabs = meta["data"], meta.get("view") or {}, meta.get("tabs") or []
    out = [f"{Path(path).name}  —  rowver {meta['rowver']}, "
           f"{d['rows']:,} events, {len(d['samples'])} samples, {len(d['cols'])} columns",
           f"  {meta['about'].get('title') or meta['about'].get('name', '')}"]
    if meta["about"].get("note"):
        out.append(f"  {meta['about']['note']}")
    unweighted = [s["name"] for s in d["samples"] if not s.get("weighted")]
    out += ["", f"COLUMNS   weights: {d.get('weight_col') or 'none — every row counts 1'}"]
    for c in d["cols"]:
        span = (f"{c['lim'][0]:.4g} .. {c['lim'][1]:.4g}" if c["kind"] == "f8"
                else " ".join(str(v) for v in (c.get("vals") or []))[:46])
        labels = (view.get("cats") or {}).get(c["name"], {}).get("names")
        out.append(f"  {c['name']:24s} {c['kind']}  {span:48s}"
                   + (f"  [{len(labels)} labels]" if labels else ""))
    out += ["", f"SAMPLES   {len(unweighted)} unweighted (real data, never scaled by lumi)"]
    for s in d["samples"]:
        out.append(f"  {s['name']:56s} {s['rows']:>9,}"
                   + ("" if s.get("weighted") else "   data"))
    locked = [k for k in ("cuts", "stage") if k in view]
    out += ["", "VIEW      " + (", ".join(sorted(view)) if view else "nothing set")
            + (f"   LOCKED: {', '.join(locked)}" if locked else "")]
    for column, cat in sorted((view.get("cats") or {}).items()):
        names = cat.get("names") or {}
        out.append(f"  {column:24s} " + ", ".join(f"{k}={v}" for k, v in names.items())[:78])
    for column, spec in sorted((view.get("cuts") or {}).items()):
        out.append(f"  cut {column:20s} {cut_of(spec)}")
    out.append("")
    out.append(f"TABS      {len(tabs) or 'none — the page opens with nothing set up'}")
    for i, t in enumerate(tabs):
        stage = effective(t, view, "stage", []) or []
        out.append(f"  {i}  {t.get('name', ''):28s} "
                   f"{' '.join(s.get('kind', '?') for s in stage) or 'empty'}")
        for column, spec in sorted((effective(t, view, 'cuts', {}) or {}).items()):
            out.append(f"       {column:22s} {cut_of(spec)}")
    made = meta.get("made") or {}
    out += ["", f"MADE      by {made.get('by', '?')} on {made.get('built', '?')}",
            f"          skimmed {made.get('skimmed') or 'UNKNOWN — these columns predate the stamp'}"]
    if made.get("source"):
        s = made["source"]
        out.append(f"          transcoded from {s.get('file')} (rowver {s.get('rowver')}, "
                   f"{s.get('made_by')})")
    return "\n".join(out)


#: v5 tab-state keys that are page-global in 6 — one copy in `view`, not N+1 in the tabs
PAGE_KEYS = ("lumi", "lumis", "wt", "syst", "cms", "exp", "rlabel", "cflog")

#: v5 tab-state keys 6 drops outright. `v` is the whole file's `rowver`; `scale` and
#: `expcuts` are export posture (and `scale` was already read once, not per tab);
#: `cutorder` is a drag order over the tab's own cuts and rides with them.
DROP_KEYS = ("v", "scale", "expcuts", "cutorder", "group", "sub", "order", "show",
             "cuts", "chips", "plots", "table", "regions", "fomn", "fomd")


def _cuts_of(state: dict, cols: list) -> dict:
    """v5's index-keyed cuts + chips as ONE name-keyed dict, keeping only what narrows."""
    out = {}
    for i, span in (state.get("cuts") or {}).items():
        col = cols[int(i)]
        stop_lo, stop_hi = col.get("nice") or col["lim"]
        lo = span[0] if span[0] > stop_lo else None      # a handle at its stop is OPEN,
        hi = span[1] if span[1] < stop_hi else None      # not a bound the mask applies
        if lo is not None or hi is not None:
            out[col["name"]] = [lo, hi]
    for i, vals in (state.get("chips") or {}).items():
        col = cols[int(i)]
        if len(vals) < len(col.get("vals") or ()):       # every chip on = no cut
            out[col["name"]] = {"in": sorted(int(v) for v in vals)}
    return out


def _expand_lumi(view: dict, old: dict, samples: list) -> None:
    """A v4/v5 GLOBAL luminosity becomes explicit namespaced entries — 6 has no scalar.

    Onto every group, and onto any sample that has none, so the resolved value is unchanged
    whichever way the sample>sub>group order is later settled: nothing else holds an entry
    yet, so there is nothing for an order to arbitrate."""
    lumi = view.pop("lumi", None)
    if lumi in (None, "", 1):                        # 1 is the no-op default, not a choice
        view.pop("lumi", None)
        return
    of = ((old.get("grouping") or {}).get("group")) or {}
    lumis = view.setdefault("lumis", {"sample": {}, "sub": {}, "group": {}})
    for group in dict.fromkeys(of.values()):
        lumis["group"].setdefault(group, lumi)
    for name in samples:
        if name not in of:
            lumis["sample"].setdefault(name, lumi)


def _shown_of(state: dict, datasets: list) -> list | None:
    """Which samples a tab draws, as names. None means all — an EMPTY MAP is no opinion."""
    visible = [d for d, on in (state.get("show") or {}).items() if on]
    return None if not visible or len(visible) == len(datasets) else visible


def _stage_of(state: dict) -> list:
    """plots + the board + the region map as ONE ordered kind-tagged list, in screen order."""
    items, blocks = [], {}
    for plot in (state.get("plots") or []):
        spec = {k: v for k, v in plot.items() if k not in ("id", "opts", "cmton")}
        sheet, quad = spec.pop("sheet", None), spec.pop("quad", None)
        entry = {"kind": spec.pop("mode", "1d"), **spec}
        if sheet is None:
            items.append((0, entry))
            continue
        # v5 stamped a group id and a slot on siblings; 6 makes the block a container, and
        # sheetPlan drew a sheet where its FIRST member sat, so that is where it goes
        if sheet not in blocks:
            blocks[sheet] = {"kind": "block", "cells": []}
            items.append((0, blocks[sheet]))
        cells = blocks[sheet]["cells"]
        slot = len(cells) if quad is None else int(quad)
        cells.extend([None] * (slot + 1 - len(cells)))
        cells[slot] = entry
    for key, kind in (("table", "board"), ("regions", "map")):
        block = state.get(key)
        if block:
            items.append((block.get("at", 0),
                          {"kind": kind, **{k: v for k, v in block.items() if k != "at"}}))
    items.sort(key=lambda pair: pair[0])                 # `at` indexed the block stack; the
    return [spec for _, spec in items]                   # list's own order replaces it


def _tabs_of(old: dict, view: dict) -> list:
    """v5's tabs as v6 delta tabs, hoisting into `view` everything that never differs."""
    states = [t.get("state") or {} for t in (old.get("tabs") or [])]
    if not states:
        return []
    agree = lambda k: len({json.dumps(s.get(k), sort_keys=True) for s in states}) == 1
    for key in PAGE_KEYS:
        if agree(key) and states[0].get(key) not in (None, "", {}, []):
            view[key] = states[0][key]
    if states[0].get("fomn"):
        view["fom"] = [states[0]["fomn"], states[0].get("fomd")]

    cuts = [_cuts_of(s, old["cols"]) for s in states]
    if len(states) > 1 and all(c == cuts[0] for c in cuts):
        view["cuts"] = cuts[0]                           # identical on every tab == LOCKED

    shown = [_shown_of(s, old["datasets"]) for s in states]
    if len(states) > 1 and all(v == shown[0] for v in shown) and shown[0] is not None:
        view["shown"] = shown[0]

    tabs = []
    for saved, state, cut, vis in zip(old["tabs"], states, cuts, shown):
        tab = {"name": saved.get("name") or "tab"}
        if not locked(view, "cuts"):
            tab["cuts"] = cut
        if not locked(view, "shown") and vis is not None:
            tab["shown"] = vis
        for key in PAGE_KEYS:
            if key not in view and state.get(key) not in (None, "", {}, []):
                tab[key] = state[key]                    # a genuine per-tab difference
        tab["stage"] = _stage_of(state)
        tabs.append(tab)
    return tabs


def from_rvr(path: str | Path, name: str = "", note: str = "", keep_view: bool = True,
             tabs: bool = False) -> tuple:
    """Transcode a v4/v5 file to v6 — the columns are passed through, never recomputed."""
    old, buf = unframe(gzip.decompress(Path(path).read_bytes()))
    assert old.get("rowver") in (4, 5), f"{path}: rowver {old.get('rowver')}, expected 4 or 5"
    stamp = old.get("cart") or {}

    # v4 has one global weights key and no per-sample declaration, so every sample on it
    # reads as weighted — that is what the file says, and inventing anything else here
    # would be the cartridge lying about columns it did not skim
    per_sample = {n: s.get("weights") for n, s in (stamp.get("samples") or {}).items()}
    weighted = lambda n: per_sample.get(n, "declared") is not None

    rows_of = _rows_per_sample(old, buf)
    data = {
        "rows": int(old["n"]),
        "bytes": int(old["bytes"]),
        "weight_col": old.get("weights") or None,
        **({"weight_at": int(old["w_offset"])} if old.get("weights") else {}),
        "sample_at": int(old["ds_offset"]),
        "samples": [{"name": n, "rows": rows_of[n], "weighted": weighted(n)}
                    for n in old["datasets"]],
        "cols": [{k: c[k] for k in ("name", "kind")} | {"at": c["offset"]}
                 | {k: c[k] for k in ("lim", "nice", "p95", "p99")}
                 | ({"vals": c["vals"]} if c.get("vals") else {})
                 for c in old["cols"]],
    }
    # cols[].names was the one editable thing welded into the immutable drawer; it moves
    view = {}
    cats = {c["name"]: {"names": {str(v): n for v, n in zip(c["vals"], c["names"])}}
            for c in old["cols"] if c.get("names")}
    if cats:
        view["cats"] = cats
    if keep_view and old.get("grouping"):
        g = old["grouping"]
        view["groups"] = {"of": g.get("group") or {}, "sub": g.get("sub") or {},
                          "order": g.get("order") or []}

    # tabs first: _tabs_of hoists the per-tab lumi into `view`, and only then is there a
    # global to expand away. 6 has no scalar, so it must not survive into the file.
    built_tabs = _tabs_of(old, view) if tabs else []
    _expand_lumi(view, old, list(old["datasets"]))

    meta = {
        "rowver": ROWVER,
        "about": {"name": name or stamp.get("name") or Path(path).stem,
                  "title": old.get("title") or "",
                  **({"note": note} if note else {}),
                  "viewer": VIEWER},
        "data": data,
        "view": view,
        "tabs": built_tabs,
        "opens_on": int(old.get("tab") or 0) if tabs else 0,
        "made": {"by": VERSION, "built": _now(),
                 "skimmed": stamp.get("skimmed"),
                 "source": {"file": Path(path).name, "rowver": old["rowver"],
                            "made_by": old.get("made_by"),
                            "sha256": hashlib.sha256(buf).hexdigest()},
                 **{k: stamp.get(k) for k in ("keep", "prepare", "columns", "cart", "merged")
                    if stamp.get(k)}},
    }
    return meta, buf


def _rows_per_sample(old: dict, buf: bytes) -> dict:
    """How many events each sample contributed, counted off the sample index itself."""
    import numpy as np
    index = np.frombuffer(buf, np.int32, old["n"], old["ds_offset"])
    counts = np.bincount(index, minlength=len(old["datasets"]))
    return {n: int(counts[i]) for i, n in enumerate(old["datasets"])}


def _now() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def demo() -> None:
    """Pin the frame, the self-consistency check, and W1/W3 — that a write loses nothing."""
    import tempfile
    meta = {"rowver": 6,
            "about": {"name": "demo", "title": "demo", "viewer": VIEWER},
            # TWO columns on purpose: a one-column fixture cannot tell a duplicate NAME
            # apart from a size mismatch, so that refusal would pass for the wrong reason
            "data": {"rows": 2, "bytes": 40, "weight_col": None, "sample_at": 32,
                     "samples": [{"name": "a", "rows": 1, "weighted": True},
                                 {"name": "b", "rows": 1, "weighted": False}],
                     "cols": [{"name": "x", "kind": "f8", "at": 0, "lim": [1.0, 2.0],
                               "nice": [0.0, 2.0], "p95": 2.0, "p99": 2.0},
                              {"name": "y", "kind": "f8", "at": 16, "lim": [3.0, 4.0],
                               "nice": [0.0, 4.0], "p95": 4.0, "p99": 4.0}]},
            "view": {"cats": {}, "zz_probe": 1},
            "tabs": [{"name": "t", "stage": [], "zz_probe": 1}],
            "opens_on": 0,
            "made": {"by": VERSION, "built": _now(), "skimmed": None},
            "zz_probe": 1}
    import struct
    buf = struct.pack("<dddd", 1.0, 2.0, 3.0, 4.0) + struct.pack("<ii", 0, 1)

    with tempfile.TemporaryDirectory() as d:
        out = Path(d) / "demo.rvr"
        write(out, meta, buf)
        back, buf2 = read(out)

        # W3: the bytes come back exactly as they went in
        assert buf2 == buf, "the column buffer did not survive the round trip"
        # W1/W4: a key no writer has heard of is still there
        for site in (back, back["view"], back["tabs"][0]):
            assert site.get("zz_probe") == 1, "a writer rebuilt from a literal"
        assert back["data"] == meta["data"], "data was rewritten"

        # each of these is a silent wrong number if it is accepted, so each must throw
        for what, break_it in (
            ("a header that contradicts the bytes",
             lambda m: m["data"].__setitem__("rows", 3)),
            ("a sample that does not say whether it is weighted",
             lambda m: m["data"]["samples"][0].pop("weighted")),
            ("a cut naming a column the file does not have",
             lambda m: m["tabs"][0].__setitem__("cuts", {"no_such_column": [0, 1]})),
            # rename, never append: the size must stay right so the NAME is what fails
            ("two columns with the same name",
             lambda m: m["data"]["cols"][1].__setitem__("name", "x")),
            ("opens_on past the end of the tabs",
             lambda m: m.__setitem__("opens_on", 7)),
        ):
            bad = json.loads(json.dumps(meta))
            break_it(bad)
            try:
                write(Path(d) / "bad.rvr", bad, buf)
            except AssertionError:
                continue
            raise AssertionError(f"{what} was accepted")

    _lock_demo()
    _lumi_demo()
    print("rvr6 demo ok")


def _lumi_demo() -> None:
    """A v4/v5 global becomes namespaced entries that resolve to the SAME luminosity."""
    old = {"grouping": {"group": {"a": "sig", "b": "bkg"}}}
    view = {"lumi": 400}
    _expand_lumi(view, old, ["a", "b", "loose"])
    assert "lumi" not in view, "the global survived into a v6 file, which has no scalar"
    assert view["lumis"]["group"] == {"sig": 400, "bkg": 400}, "a group did not get the global"
    assert view["lumis"]["sample"] == {"loose": 400}, \
        "an ungrouped sample must carry it itself, or it silently resolves to 1"
    assert "a" not in view["lumis"]["sample"], "a grouped sample should not also be named"

    # an entry that already exists is never overwritten, and lumi 1 is a default not a choice
    view = {"lumi": 400, "lumis": {"sample": {}, "sub": {}, "group": {"sig": 10}}}
    _expand_lumi(view, old, ["a", "b"])
    assert view["lumis"]["group"] == {"sig": 10, "bkg": 400}, "the expansion clobbered a real entry"
    view = {"lumi": 1}
    _expand_lumi(view, old, ["a"])
    assert "lumis" not in view and "lumi" not in view, "lumi 1 invented an entry"


def _lock_demo() -> None:
    """Promote-and-shadow, end to end: lock, read through it, unlock, and get everything back."""
    m = {"view": {},
         "tabs": [{"name": "a", "cuts": {"x": [0, 1]}},
                  {"name": "b", "cuts": {}},              # authored EMPTY — the case that matters
                  {"name": "c"}]}                         # never narrowed anything
    before = _clone(m["tabs"])

    lock(m, "cuts", from_tab=0)
    assert locked(m["view"], "cuts"), "locking did not put the value in view"
    assert m["view"]["cuts"] == {"x": [0, 1]}, "the tab you were looking at did not win"
    # a shared TRUTH: every tab shows it, including the one authored with no cuts at all
    for tab in m["tabs"]:
        assert effective(tab, m["view"], "cuts") == {"x": [0, 1]}, "a tab opted out of the lock"
    # and nothing was destroyed on the way in
    assert m["tabs"] == before, "locking overwrote a tab's own cuts"

    m["view"]["cuts"]["y"] = [2, 3]                       # an edit while locked: ONE object
    m["tabs"].append({"name": "d"})                       # a tab born under the lock

    unlock(m, "cuts")
    assert not locked(m["view"], "cuts"), "unlocking left the value in view"
    # the two tabs that HAD their own copy get exactly it back, edits under the lock discarded
    assert [t["cuts"] for t in m["tabs"][:2]] == [b["cuts"] for b in before[:2]], \
        "a tab did not get its own cuts back"
    assert m["tabs"][1]["cuts"] == {}, "an authored-empty tab was widened, not restored"
    assert m["tabs"][2]["cuts"] == {"x": [0, 1], "y": [2, 3]}, \
        "a tab that never had cuts should keep what was on screen, not drop to none"
    assert m["tabs"][3]["cuts"] == {"x": [0, 1], "y": [2, 3]}, \
        "a tab born under the lock should keep what was on screen"

    # an absent tab key falls through to the page whichever way LOCK_WINS points
    assert effective({}, {"cuts": {"x": [0, 1]}}, "cuts") == {"x": [0, 1]}
    assert effective({}, {}, "cuts") == {}                # the neutral value, from NEUTRAL


USAGE = """usage:
    python rvr6.py show <file>.rvr            what a v6 file holds, for a human
    python rvr6.py dump <file>.rvr            its META, printed compactly
    python rvr6.py from <old>.rvr <new>.rvr   transcode a v4/v5 file to v6
    python rvr6.py demo                       the self-check
"""

if __name__ == "__main__":
    import sys
    args = sys.argv[1:]
    if not args or args[0] == "demo":
        demo()
    elif args[0] == "show" and len(args) == 2:
        print(show(args[1]))
    elif args[0] == "dump" and len(args) == 2:
        print(compact(read(args[1])[0]))
    elif args[0] == "from" and len(args) == 3:
        meta, buf = from_rvr(args[1])
        print(write(args[2], meta, buf))
    else:
        sys.exit(USAGE)
