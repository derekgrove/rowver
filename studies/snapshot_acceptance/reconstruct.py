"""Print the ``made`` drawer of a cartridge laid out in recipe form, from nothing else in it.

Usage: python reconstruct.py <cartridge.rvr> [original cart.py]
The output is for reading beside the cart.py, not for running: columns and
labels are not in ``made``, and files are keyed by ROOT UUID, not by path.
"""
import pprint
import sys
from pathlib import Path

from rowver import rvr


class Ref(str):
    """Text printed bare inside a literal, so a hook appears as its identifier or its source."""

    def __repr__(self) -> str:
        return str(self)


def declared(declaration: dict | None) -> str | Ref | None:
    """A branch as its name, a named hook as its identifier, a lambda as its source, None as is."""
    if declaration is None:
        return None
    if declaration["kind"] == "branch":
        return declaration["value"]
    name = declaration["name"]
    return Ref(name if name.isidentifier() else repr(declaration["source"]))


def main(cartridge: str, original: str | None = None) -> None:
    meta, _ = rvr.read(cartridge)
    made = meta["made"]
    hooks = [made["prepare"], made["keep"]] + [s["weights"] for s in made["samples"].values()]
    defs = dict.fromkeys(h["source"] for h in hooks
                         if h and h["kind"] == "python" and h["name"].isidentifier())
    cart = made["cart"]
    lines = [
        f'"""The made drawer of {Path(cartridge).name} in recipe form; rvr.read verified every digest.',
        "",
        f"study_name: {made['study_name']}",
        f"by: {made['by']}",
        f"built: {made['built']}",
        f"made.sha256: {made['sha256']}",
        f"made.cart.sha256: {cart['sha256'] if cart else None}",
    ]
    if original:
        text = Path(original).read_text(encoding="utf-8")
        same = cart is not None and cart["source"] == text
        lines.append(f"sha256({original}): {rvr.cart_digest(text)}")
        lines.append(f"made.cart.source is {original} byte for byte: {same}")
    lines += ['"""', "", ""]
    lines += [source + "\n\n" for source in defs]
    samples = {name: {"files": sample["files"],
                      "metadata": {**sample["metadata"], "weights": declared(sample["weights"])}}
               for name, sample in made["samples"].items()}
    lines.append("SAMPLES = " + pprint.pformat(samples, width=100, sort_dicts=False))
    lines += ["", f"PREPARE = {declared(made['prepare'])!r}", f"KEEP = {declared(made['keep'])!r}"]
    lines += [f"{key.upper()} = {made.get(key)!r}" for key in ("title", "note")]
    lines.append("# columns and labels are not in made: data.cols[].source and data.cols[].names")
    lines.append("# hold them, and made.cart.source has the declarations verbatim")
    print("\n".join(lines))


if __name__ == "__main__":
    main(*sys.argv[1:3])
