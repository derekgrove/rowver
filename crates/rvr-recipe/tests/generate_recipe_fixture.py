"""Regenerate the expression fixture: inputs, expressions, and what NumPy, awkward and coffea give for each.

Run with `mamba run -n rowver python tests/generate_recipe_fixture.py`. The versions used are
printed at the end and recorded in the fixture.

Precision: the Rust evaluator computes each operation in 64 bits and rounds once to the column's
kind. For a kinematic formula (delta phi, delta R, a mass) the reference here is therefore the
coffea call on 64-bit copies of the columns, rounded once to 32 bits where the columns are
32-bit; coffea on the 32-bit columns themselves rounds after every operation and can differ in
the last digit, or by much more where a mass cancels. Such a case is marked `close` with the
scale of the quantities going in, and the Rust test allows a few parts in ten million of the
larger of that scale and the value.
"""
import json
import pathlib

import awkward as ak
import coffea
import numpy as np
from coffea.nanoevents.methods import vector

EVENTS = 60
SAMPLE = {"trigger": "flag", "year": 2018, "xsec": 87.3, "is_data": False, "era": "UL18"}
# The names the cases may use, as a recipe's [define] gives them: each may use the ones above it.
DEFINES = {
    "good_muon": "Muon_tight and Muon_pt > 15",
    "central_jet": "Jet_pt > 30 and abs(Jet_eta) < 2.4",
    "hem_veto": "count(Jet_pt > 30 and Jet_eta > -3.2 and Jet_eta < -1.3 and Jet_phi > -1.57 and Jet_phi < -0.87) == 0",
    "met_twice": "MET * 2",
}


def inputs():
    """Columns named as NanoAOD names them, with every kind an expression may meet."""
    r = np.random.default_rng(11)
    jets, muons = r.poisson(2.2, EVENTS), r.poisson(1.3, EVENTS)
    jets[:3], muons[:3] = 0, 0

    def lists(counts, values):
        return ak.unflatten(values, counts)

    nj, nm = int(jets.sum()), int(muons.sum())
    return {
        "MET": r.exponential(45, EVENTS).astype("f4"), "weight": r.normal(1, 0.3, EVENTS),
        "nlep": r.integers(0, 5, EVENTS).astype("i4"), "small": r.integers(-9, 9, EVENTS).astype("i2"),
        "run": r.integers(1, 6, EVENTS).astype("u4"), "event": (r.integers(0, 2 ** 62, EVENTS).astype("u8") * np.uint64(4)),
        "flag": r.random(EVENTS) > 0.4, "tiny": r.integers(0, 200, EVENTS).astype("u1"),
        "nJet": jets.astype("i4"), "Jet_pt": lists(jets, r.exponential(40, nj).astype("f4") + np.float32(15)),
        "Jet_eta": lists(jets, r.uniform(-3.5, 3.5, nj).astype("f4")), "Jet_phi": lists(jets, r.uniform(-np.pi, np.pi, nj).astype("f4")),
        "Jet_btag": lists(jets, r.random(nj).astype("f4")), "Jet_id": lists(jets, r.integers(0, 7, nj).astype("i4")),
        "nMuon": muons.astype("i4"), "Muon_pt": lists(muons, r.exponential(25, nm).astype("f4") + np.float32(3)),
        "Muon_eta": lists(muons, r.uniform(-2.4, 2.4, nm).astype("f4")), "Muon_phi": lists(muons, r.uniform(-np.pi, np.pi, nm).astype("f4")),
        "Muon_mass": lists(muons, np.full(nm, 0.10566, dtype="f4")),
        "Muon_charge": lists(muons, r.choice([-1, 1], nm).astype("i4")), "Muon_tight": lists(muons, r.random(nm) > 0.3),
        # Drawn last, so that the columns above stay as they were before this one was added.
        "MET_phi": r.uniform(-np.pi, np.pi, EVENTS).astype("f4"),
    }


def wide(array):
    """A 64-bit copy of a column, so that a formula is rounded once at the end."""
    return ak.values_astype(array, "f8")


def narrow(array):
    """A result rounded once to 32 bits, as the Rust evaluator rounds a formula of 32-bit columns."""
    return ak.values_astype(array, "f4")


def momenta(pt, eta, phi, mass=None):
    """Coffea four-momenta from 64-bit copies of the columns."""
    held = {"pt": wide(pt), "eta": wide(eta), "phi": wide(phi), "mass": wide(mass) if mass is not None else ak.zeros_like(wide(pt))}
    return ak.zip(held, with_name="PtEtaPhiMLorentzVector", behavior=vector.behavior)


def summed_mass(particles):
    """The mass of each event's particles added together, summed component by component."""
    total = ak.zip({part: ak.sum(getattr(particles, part), axis=1) for part in ("x", "y", "z", "t")},
                   with_name="LorentzVector", behavior=vector.behavior)
    return narrow(total.mass)


def ranked(table, rank, descending):
    """The rank-th nearest (or farthest) member of the other list and the distance, with None where there is none."""
    order = ak.argsort(table, axis=2, ascending=not descending)
    picked = ak.pad_none(order, rank + 1, axis=2)[:, :, rank]
    distance = ak.pad_none(ak.sort(table, axis=2, ascending=not descending), rank + 1, axis=2)[:, :, rank]
    return picked, distance


def cases(c):
    """Each expression beside the NumPy, awkward or coffea line it stands for.

    A third item marks a last-digit tolerance: True for a relative one, or the scale of the
    quantities that went in, for a formula that cancels.
    """
    f4, pi = np.float32, np.pi
    good = c["Muon_tight"] & (c["Muon_pt"] > 15)
    central = (c["Jet_pt"] > 30) & (np.abs(c["Jet_eta"]) < 2.4)
    hem = (c["Jet_pt"] > 30) & (c["Jet_eta"] > -3.2) & (c["Jet_eta"] < -1.3) & (c["Jet_phi"] > -1.57) & (c["Jet_phi"] < -0.87)

    def pad(array, position, otherwise, dtype):
        return np.array([row[position] if len(row) > position else otherwise for row in array.tolist()], dtype=dtype)

    def extreme(array, function, otherwise, dtype):
        return np.array([function(row) if len(row) else otherwise for row in array.tolist()], dtype=dtype)

    gm = {key: c[f"Muon_{key}"][good] for key in ("pt", "eta", "phi", "mass", "charge")}
    jets = momenta(c["Jet_pt"], c["Jet_eta"], c["Jet_phi"])
    muons = momenta(c["Muon_pt"], c["Muon_eta"], c["Muon_phi"], c["Muon_mass"])
    good_muons = momenta(gm["pt"], gm["eta"], gm["phi"], gm["mass"])
    met = ak.zip({"rho": wide(c["MET"]), "phi": wide(c["MET_phi"])}, with_name="PolarTwoVector", behavior=vector.behavior)
    jet_muon = jets.metric_table(muons)
    nearest_muon, nearest_dr = jets.nearest(muons, return_metric=True)
    muon_pairs = ak.combinations(muons, 2, fields=["a", "b"])
    jet_pairs = ak.combinations(jets, 2, fields=["a", "b"])
    jet_pt_pairs = ak.combinations(c["Jet_pt"], 2, fields=["a", "b"])
    jet_id_pairs = ak.combinations(c["Jet_id"], 2, fields=["a", "b"])
    pair_dr = muon_pairs.a.delta_r(muon_pairs.b)
    pair_mass = (muon_pairs.a + muon_pairs.b).mass
    scale = float(ak.max(ak.sum(muons.t, axis=1)))
    far_index, far_dr = ranked(jet_muon, 0, True)
    second_index, second_dr = ranked(jet_muon, 1, False)
    second_far_index, second_far_dr = ranked(jet_muon, 1, True)
    by_pt_down = ak.argsort(c["Jet_pt"], axis=1, ascending=False)
    nearest_index = ak.fill_none(ak.argmin(jet_muon, axis=2), -1)
    return [
        ("MET > 30", c["MET"] > 30), ("MET < 0.15", c["MET"] < 0.15), ("MET * 2 + 1", c["MET"] * 2 + 1),
        ("MET / (nlep + 1)", c["MET"] / (c["nlep"] + 1)), ("nlep / 2", c["nlep"] / 2), ("nlep % 3", c["nlep"] % 3),
        ("(nlep - 2) % 3", (c["nlep"] - 2) % 3), ("small % -4", c["small"] % -4), ("MET % 7.5", c["MET"] % 7.5),
        ("-MET % 7.5", -c["MET"] % 7.5), ("nlep ** 2", c["nlep"] ** 2), ("MET ** 2", c["MET"] ** 2),
        ("MET ** 0.5", c["MET"] ** 0.5, True), ("-MET ** 2", -c["MET"] ** 2), ("small * 3", c["small"] * 3),
        ("small + nlep", c["small"] + c["nlep"]), ("run + nlep", c["run"] + c["nlep"]), ("tiny + small", c["tiny"] + c["small"]),
        ("tiny * 2", c["tiny"] * 2), ("event + 1", c["event"] + 1), ("event > 5", c["event"] > 5), ("run == 3", c["run"] == 3),
        ("event / 4", c["event"] / 4), ("weight * MET", c["weight"] * c["MET"]), ("MET - weight", c["MET"] - c["weight"]),
        ("abs(MET - 50)", np.abs(c["MET"] - 50)), ("abs(small)", np.abs(c["small"])), ("sqrt(MET)", np.sqrt(c["MET"])),
        ("sqrt(nlep)", np.sqrt(c["nlep"])), ("max(MET, 40)", np.maximum(c["MET"], 40)), ("min(MET, weight)", np.minimum(c["MET"], c["weight"])),
        ("max(nlep, small)", np.maximum(c["nlep"], c["small"])), ("sign(MET - 50)", np.sign(c["MET"] - 50)), ("sign(small)", np.sign(c["small"])),
        ("hypot(MET, 3)", np.hypot(c["MET"], f4(3)), True), ("cos(MET)", np.cos(c["MET"]), True), ("sinh(MET / 50)", np.sinh(c["MET"] / 50), True),
        ("cosh(weight)", np.cosh(c["weight"]), True), ("isfinite(MET / (nlep - 2))", np.isfinite(c["MET"] / (c["nlep"] - 2))),
        ("flag and MET > 30", c["flag"] & (c["MET"] > 30)), ("not flag or nlep == 2", ~c["flag"] | (c["nlep"] == 2)),
        ("not (MET > 30)", ~(c["MET"] > 30)), ("nlep in [1, 3]", (c["nlep"] == 1) | (c["nlep"] == 3)),
        ("MET + pi", c["MET"] + pi), ("2 * pi", 2 * pi), ("1 + 2 * 3 - 4 / 8", 1 + 2 * 3 - 4 / 8), ("7 % 3 == 1", True),
        ("Jet_pt > 30", c["Jet_pt"] > 30), ("Jet_pt * MET", c["Jet_pt"] * c["MET"]), ("Jet_pt - 2 * Jet_btag", c["Jet_pt"] - 2 * c["Jet_btag"]),
        ("Jet_id + nlep", c["Jet_id"] + c["nlep"]), ("Jet_pt[Jet_pt > 30]", c["Jet_pt"][c["Jet_pt"] > 30]),
        ("count(Jet_pt > 30)", ak.sum(c["Jet_pt"] > 30, axis=1)), ("count(Jet_pt)", ak.num(c["Jet_pt"])),
        ("count(Jet_pt > 30 and abs(Jet_eta) < 2.4)", ak.sum(central, axis=1)), ("count(central_jet)", ak.sum(central, axis=1)),
        ("Jet_pt[Jet_btag > 0.5]", c["Jet_pt"][c["Jet_btag"] > 0.5]), ("Jet_eta[central_jet and Jet_btag > 0.5]", c["Jet_eta"][central & (c["Jet_btag"] > 0.5)]),
        ("sum(Jet_pt)", ak.sum(c["Jet_pt"], axis=1)), ("sum(Muon_charge)", ak.sum(c["Muon_charge"], axis=1)),
        ("sum(Jet_pt > 30)", ak.sum(c["Jet_pt"] > 30, axis=1)), ("any(Jet_btag > 0.5)", ak.any(c["Jet_btag"] > 0.5, axis=1)),
        ("all(Jet_pt > 20)", ak.all(c["Jet_pt"] > 20, axis=1)),
        ("least(Jet_pt, inf)", extreme(c["Jet_pt"], min, np.inf, "f4")), ("greatest(Jet_pt, -1)", extreme(c["Jet_pt"], max, -1, "f4")),
        ("greatest(Jet_id, -1)", extreme(c["Jet_id"], max, -1, "i4")), ("least(Jet_id, 0.5)", extreme(c["Jet_id"], min, 0.5, "f8")),
        ("first(Jet_pt, -1)", pad(c["Jet_pt"], 0, -1, "f4")), ("nth(Jet_pt, 1, 0)", pad(c["Jet_pt"], 1, 0, "f4")),
        ("first(Muon_charge, 0)", pad(c["Muon_charge"], 0, 0, "i4")), ("nth(Jet_eta, 2, -99.5)", pad(c["Jet_eta"], 2, -99.5, "f4")),
        ("Muon_pt[good_muon]", gm["pt"]), ("count(good_muon)", ak.sum(good, axis=1)), ("sum(Muon_charge[good_muon]) == 0", ak.sum(gm["charge"], axis=1) == 0),
        ("Muon_eta[good_muon]", gm["eta"]), ("index(Jet_pt)", ak.local_index(c["Jet_pt"], axis=1)),
        ("listed(Jet_pt, Jet_id)", ak.any(ak.local_index(c["Jet_pt"], axis=1)[:, :, None] == c["Jet_id"][:, None, :], axis=2)),
        # Coffea: the nearest member of the other list and the distance to it.
        ("dr_nearest(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", narrow(ak.fill_none(nearest_dr, np.inf)), True),
        ("nearest_index(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", nearest_index),
        ("least(dr_nearest(Muon_eta, Muon_phi, Jet_eta, Jet_phi), inf) > 0.4",
         ak.fill_none(ak.min(muons.nearest(jets, return_metric=True)[1], axis=1), np.inf) > 0.4),
        ("dr_nearest(Jet_eta, Jet_phi, Muon_eta[good_muon], Muon_phi[good_muon])", narrow(ak.fill_none(jets.nearest(good_muons, return_metric=True)[1], np.inf)), True),
        # The ranks after the nearest: argsort over coffea's metric_table.
        ("dr_second_nearest(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", narrow(ak.fill_none(second_dr, np.inf)), True),
        ("second_nearest_index(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", ak.fill_none(second_index, -1)),
        ("dr_farthest(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", narrow(ak.fill_none(far_dr, np.inf)), True),
        ("dr_farthest(Jet_eta, Jet_phi, Muon_eta, Muon_phi, -1)", narrow(ak.fill_none(far_dr, -1)), True),
        ("farthest_index(Jet_eta, Jet_phi, Muon_eta, Muon_phi, 99)", ak.fill_none(far_index, 99)),
        ("dr_second_farthest(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", narrow(ak.fill_none(second_far_dr, np.inf)), True),
        ("second_farthest_index(Jet_eta, Jet_phi, Muon_eta, Muon_phi)", ak.fill_none(second_far_index, -1)),
        ("take(Muon_pt, nearest_index(Jet_eta, Jet_phi, Muon_eta, Muon_phi), -1)", narrow(ak.fill_none(nearest_muon.pt, -1))),
        # Coffea: the mass of a sum of four-momenta, signed where it is spacelike.
        ("mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon])", summed_mass(momenta(gm["pt"], gm["eta"], gm["phi"])), scale),
        ("mass(Muon_pt[good_muon], Muon_eta[good_muon], Muon_phi[good_muon], Muon_mass[good_muon])", summed_mass(good_muons), scale),
        ("mass(Muon_pt, Muon_eta, Muon_phi, Muon_mass) > 60", summed_mass(muons) > 60),
        # Coffea: delta phi and delta R between two values, a list against one value per event.
        ("delta_phi(Jet_phi, MET_phi)", narrow(jets.delta_phi(met)), True),
        ("delta_phi(MET_phi, 0.5)", narrow((wide(c["MET_phi"]) - 0.5 + pi) % (2 * pi) - pi), True),
        ("abs(delta_phi(Jet_phi, MET_phi)) < pi / 2", np.abs(narrow(jets.delta_phi(met))) < pi / 2),
        ("delta_r(Jet_eta, Jet_phi, 0, MET_phi)", narrow(np.sqrt(wide(c["Jet_eta"]) ** 2 + jets.delta_phi(met) ** 2)), True),
        ("delta_r(Muon_eta, Muon_phi, first(Jet_eta, 0), first(Jet_phi, 0))",
         narrow(muons.delta_r(momenta(pad(c["Jet_pt"], 0, 0, "f4"), pad(c["Jet_eta"], 0, 0, "f4"), pad(c["Jet_phi"], 0, 0, "f4")))), True),
        # Awkward: every pair of a list, and coffea on the pairs.
        ("pair_first(Jet_pt)", jet_pt_pairs.a), ("pair_second(Jet_id)", jet_id_pairs.b),
        ("pair_dr(Muon_eta, Muon_phi)", narrow(pair_dr), True),
        ("pair_mass(Muon_pt, Muon_eta, Muon_phi, Muon_mass)", narrow(pair_mass), scale),
        ("pair_mass(Jet_pt, Jet_eta, Jet_phi)", narrow((jet_pairs.a + jet_pairs.b).mass), float(ak.max(ak.sum(jets.t, axis=1)))),
        ("all(pair_mass(Muon_pt, Muon_eta, Muon_phi, Muon_mass) > if(pair_dr(Muon_eta, Muon_phi) < 1.5, 1.5 - pair_dr(Muon_eta, Muon_phi), 0))",
         ak.all(narrow(pair_mass) > ak.where(narrow(pair_dr) < 1.5, f4(1.5) - narrow(pair_dr), f4(0)), axis=1)),
        ("count(pair_first(Muon_charge) + pair_second(Muon_charge) == 0)",
         ak.sum(ak.combinations(c["Muon_charge"], 2, fields=["a", "b"]).a + ak.combinations(c["Muon_charge"], 2, fields=["a", "b"]).b == 0, axis=1)),
        # Awkward: which member, and in what order.
        ("least_index(Jet_pt, -1)", ak.fill_none(ak.argmin(c["Jet_pt"], axis=1), -1)),
        ("greatest_index(Jet_btag, -1)", ak.fill_none(ak.argmax(c["Jet_btag"], axis=1), -1)),
        ("greatest_index(Jet_id, -1)", ak.fill_none(ak.argmax(c["Jet_id"], axis=1), -1)),
        ("order(Jet_pt)", ak.argsort(c["Jet_pt"], axis=1)), ("order(Jet_pt, 'descending')", by_pt_down),
        ("sorted(Jet_pt, 'descending')", ak.sort(c["Jet_pt"], axis=1, ascending=False)), ("sorted(Jet_id)", ak.sort(c["Jet_id"], axis=1)),
        ("take(Jet_pt, order(Jet_pt, 'descending'), 0)", c["Jet_pt"][by_pt_down]),
        ("first(sorted(Jet_pt, 'descending'), -1)", narrow(ak.fill_none(ak.firsts(ak.sort(c["Jet_pt"], axis=1, ascending=False)), -1))),  # awkward widens to f8 when it fills,
        ("take(Jet_btag, Jet_id, -1)",
         narrow(ak.Array([[row[i] if i < len(row) else -1.0 for i in ids] for row, ids in zip(c["Jet_btag"].tolist(), c["Jet_id"].tolist())]))),
        # NumPy: clipping and bits.
        ("clip(MET, 20, 100)", np.clip(c["MET"], 20, 100)), ("clip(small, -3, 3)", np.clip(c["small"], -3, 3)),
        ("clip(Jet_pt, 0, 50)", np.minimum(np.maximum(c["Jet_pt"], f4(0)), f4(50))),  # np.clip itself refuses a jagged array
        ("bit_and(Jet_id, 3)", np.bitwise_and(c["Jet_id"], 3)), ("shift_right(Jet_id, 1)", np.right_shift(c["Jet_id"], 1)),
        ("bit_and(shift_right(Jet_id, 1), 1)", np.bitwise_and(np.right_shift(c["Jet_id"], 1), 1)),
        ("shift_right(tiny, 2)", np.right_shift(c["tiny"], 2)), ("shift_right(small, 1)", np.right_shift(c["small"], 1)),
        ("if(MET > 50, MET, 0)", np.where(c["MET"] > 50, c["MET"], f4(0))), ("if(Jet_pt > 30, Jet_pt, -1)", ak.where(c["Jet_pt"] > 30, c["Jet_pt"], f4(-1))),
        ("if(flag, nlep, weight)", np.where(c["flag"], c["nlep"], c["weight"])), ("if(MET > 50, Jet_pt, Jet_btag)", ak.where(c["MET"] > 50, c["Jet_pt"], c["Jet_btag"])),
        ("if(sample.is_data, 1, MET)", c["MET"]),
        ("sample.year == 2018", True), ("sample.era in ['UL18', 'Autumn18']", True), ("sample.era == 'UL17'", False),
        ("sample.absent == 3", False), ("sample.absent in ['x']", False), ("branch(sample.trigger)", c["flag"]),
        ("MET * sample.xsec", c["MET"] * 87.3), ("sample.is_data or MET > 30", c["MET"] > 30), ("sample.year - 2000", 18),
        ("stored(count(Jet_pt > 30), 'i4')", ak.to_numpy(ak.sum(c["Jet_pt"] > 30, axis=1)).astype("i4")), ("stored(MET, 'f8')", c["MET"].astype("f8")),
        ("stored(Jet_pt, 'i2')", ak.values_astype(c["Jet_pt"], "i2")),
        ("hem_veto", ak.sum(hem, axis=1) == 0), ("met_twice > 60", c["MET"] * 2 > 60), ("Jet_pt * 2", c["Jet_pt"] * 2),
    ]


def number(value):
    """A JSON spelling of one value; infinities as text."""
    if isinstance(value, (bool, np.bool_)):
        return bool(value)
    if isinstance(value, (float, np.floating)):
        return float(value) if np.isfinite(value) else ("inf" if value > 0 else "-inf" if value < 0 else "nan")
    return int(value)


def record(value):
    """The kind, shape, and values of a column or result."""
    if isinstance(value, (bool, int, float)):
        return {"shape": "constant", "values": [number(value)]}
    counts = None
    if isinstance(value, ak.Array) and value.ndim == 2:
        counts = ak.to_numpy(ak.num(value)).tolist()
        value = ak.flatten(value)
    flat = ak.to_numpy(value) if isinstance(value, ak.Array) else np.asarray(value)
    out = {"shape": "event" if counts is None else "object", "kind": f"{flat.dtype.kind}{flat.dtype.itemsize}",
           "values": [number(item) for item in flat.tolist()] if flat.dtype.kind != "f" else [number(item) for item in flat]}
    return {**out, "counts": counts} if counts is not None else out


def main():
    """Write the fixture as one JSON file."""
    columns = inputs()
    with np.errstate(divide="ignore"):
        results = []
        for case in cases(columns):
            close = case[2] if len(case) > 2 else False
            results.append({"expression": case[0], "close": close is not False, "scale": 0.0 if isinstance(close, bool) else close, **record(case[1])})
    held = {name: record(value) for name, value in columns.items()}
    versions = {"numpy": np.__version__, "awkward": ak.__version__, "coffea": coffea.__version__}

    def lines(records):
        """One compact line per record."""
        return ",\n".join("  " + json.dumps(item, separators=(",", ":")) for item in records)

    text = ("{\n" + f' "versions": {json.dumps(versions)},\n "sample": {json.dumps(SAMPLE)},\n "defines": {json.dumps(DEFINES)},\n'
            + ' "columns": {\n' + ",\n".join(f"  {json.dumps(name)}: {json.dumps(item, separators=(',', ':'))}" for name, item in held.items())
            + '\n },\n "cases": [\n' + lines(results) + "\n ]\n}\n")
    json.loads(text)
    (pathlib.Path(__file__).parent / "fixtures" / "recipe.json").write_text(text)
    print(len(results), "cases;", versions)


if __name__ == "__main__":
    main()
