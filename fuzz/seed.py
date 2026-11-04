"""Make the fuzz targets' seeds, fuzz/seeds/TARGET/, from every fixture, example and refusal case.

    mamba run -n rowver python fuzz/seed.py

The seeds are made again from what the repository holds (the conformance corpus, the examples, the
tests' argument lists), so a change of the format changes them: run it again after one.
"""

import hashlib
import json
import pathlib
import re
import shutil
import struct
import subprocess
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SEEDS = ROOT / "fuzz" / "seeds"
MOST = 1 << 20
COMMANDS = {"new", "samples", "press", "status", "branches", "try", "derive", "validate", "dump",
            "header", "diff", "reproduce", "empty", "refill", "key", "proxy"}


def unzstd(data):
    """One zstd frame, decompressed by Python when it can, else by the zstd command."""
    try:
        from compression import zstd
        return zstd.decompress(data)
    except ImportError:
        pass
    try:
        import zstandard
        return zstandard.ZstdDecompressor().decompress(data)
    except ImportError:
        return subprocess.run(["zstd", "-d", "-c"], input=data, capture_output=True, check=True).stdout


def header(data):
    """A .rvr file's JSON header, or None when it has none that reads."""
    try:
        size = struct.unpack("<I", data[8:12])[0]
        return json.loads(data[12:12 + size])
    except (struct.error, ValueError, UnicodeDecodeError):
        return None


def rvr_files():
    """Every valid file of the corpus, and every case of cases.zst."""
    part = ROOT / "tests" / "conformance" / "rvr"
    files = [p.read_bytes() for p in sorted((part / "valid").glob("*.rvr"))]
    cases = unzstd((part / "cases.zst").read_bytes())
    for case in json.loads((part / "manifest.json").read_text())["cases"]:
        files.append(cases[case["at"]:case["at"] + case["size"]])
    return files


def texts(kind, files):
    """The texts of one kind (samples, lock, recipe): the corpus's cases, those the files carry, and the examples'."""
    found = [case["text"] for case in json.loads((ROOT / "tests" / "conformance" / kind / "manifest.json").read_text())["cases"]]
    for data in files:
        made = header(data)
        for key in ["made", kind, "text"]:
            made = made.get(key) if isinstance(made, dict) else None
        if isinstance(made, str):
            found.append(made)
    name = {"samples": "samples.toml", "recipe": "recipe.toml", "lock": "samples.lock"}[kind]
    for path in sorted(ROOT.glob(f"docs/examples/*/{name}")) + sorted(ROOT.glob(f"crates/rvr/templates/{name}")):
        found.append(path.read_text())
    return found


def expressions(recipes):
    """Every expression of the recipe fixtures, of the recipes, and every code span of the recipe README."""
    found = []
    for name in ["recipe.json", "properties.json"]:
        fixture = json.loads((ROOT / "crates" / "rvr-recipe" / "tests" / "fixtures" / name).read_text())
        found += [case["expression"] for case in fixture["cases"]]
        found += list(fixture.get("defines", {}).values())
    for text in recipes:
        try:
            recipe = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            continue
        for table in ["define", "keep", "columns", "derived", "totals"]:
            for value in recipe.get(table, {}).values() if isinstance(recipe.get(table), dict) else []:
                value = value.get("value") if isinstance(value, dict) else value
                if isinstance(value, str):
                    found.append(value)
    readme = (ROOT / "crates" / "rvr-recipe" / "README.md").read_text()
    found += re.findall(r"`([^`\n]+)`", readme)
    return found


def answers():
    """Multipart answers as servers lay them out, and as they go wrong: a Content-Type line, then the answer."""
    def answer(boundary, parts, end=True, newline=b"\r\n", place="Content-Range"):
        out = b""
        for at, data in parts:
            head = f"--{boundary}".encode() + newline + b"Content-Type: application/octet-stream" + newline
            head += f"{place}: bytes {at}-{at + len(data) - 1}/1000".encode() + newline + newline
            out += newline + head + data
        return out + (newline + f"--{boundary}--".encode() + newline if end else b"")
    typed = lambda boundary, body: f"multipart/byteranges; boundary={boundary}\n".encode() + body
    tricky = b"\r\n--B\r\n\n\n--B--"
    return [
        typed("B", answer("B", [(10, b"abc"), (500, tricky), (0, bytes([0, 255, 10, 13]))])),
        typed("RVRTEST", answer("RVRTEST", [(0, b"0123456789"), (100, b"x" * 64)])),
        typed('"a b"', answer("a b", [(5, b"hello")])),
        typed("B", answer("B", [(1, b"x")], end=False)),
        typed("B", answer("B", [(7, b"lf only")], newline=b"\n")),
        typed("B", answer("B", [(3, b"abc")], place="Content-Type")),
        typed("B", b"--B\r\nContent-Range: bytes 9-3/10\r\n\r\n"),
        typed("B", b"--B\r\nX: " + b"y" * 3000 + b"\r\n\r\n"),
        typed("B", answer("B", [(0, b"abcdef")])[:-12]),
        typed("other", answer("B", [(0, b"abc")])),
        b"application/octet-stream\n" + b"abc",
    ]


def literal(item):
    """A Rust string literal's text (as JSON reads it), or None for any other element."""
    if not re.fullmatch(r'"(?:[^"\\]|\\.)*"', item, re.S):
        return None
    try:
        return json.loads(item, strict=False)
    except ValueError:
        return None


def arrays(source):
    """Every array literal `&[...]` of Rust source, as its elements: a string literal's text, or None."""
    found, at = [], 0
    while (at := source.find("&[", at)) >= 0:
        depth, i, items, item = 1, at + 2, [], ""
        while i < len(source) and depth:
            c = source[i]
            if c == '"':
                end = i + 1
                while end < len(source) and source[end] != '"':
                    end += 2 if source[end] == "\\" else 1
                item += source[i:end + 1]
                i = end + 1
                continue
            depth += c in "([{"
            depth -= c in ")]}"
            if depth == 0 or (depth == 1 and c == ","):
                items.append(item.strip())
                item = ""
            else:
                item += c
            i += 1
        found.append([literal(s) for s in items if s])
        at += 2
    return found


def argument_lists():
    """Argument lists rvr is given: those of the tests, the usage lines, and the README's command table."""
    found = []
    for path in sorted((ROOT / "crates" / "rvr" / "tests").rglob("*.rs")):
        for items in arrays(path.read_text()):
            first = items[0] if items else None
            if isinstance(first, str) and (first in COMMANDS or first.startswith("-")):
                found.append(["FILE.rvr" if item is None else item for item in items])
    readme = (ROOT / "crates" / "rvr" / "README.md").read_text()
    usage = re.findall(r"^\s*rvr (.+)$", (ROOT / "crates" / "rvr" / "src" / "lib.rs").read_text(), re.M)
    for line in usage + re.findall(r"^\| `rvr ([^`]+)`", readme, re.M):
        found.append(re.sub(r"[\[\]|…]|\.\.\.", " ", line.split("  ")[0]).split())
    return [b"".join(arg.encode() + b"\0" for arg in args) for args in found]


def write(target, seeds):
    folder = SEEDS / target
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    kept = {}
    for seed in seeds:
        data = seed.encode() if isinstance(seed, str) else seed
        if len(data) <= MOST:
            kept[hashlib.sha1(data).hexdigest()] = data
    for name, data in kept.items():
        (folder / name).write_bytes(data)
    print(f"{target}: {len(kept)} seeds")


files = rvr_files()
recipes = texts("recipe", files)
write("open", files)
write("validate", files)
write("lock", texts("lock", files))
write("samples", texts("samples", files))
write("recipe", recipes)
write("expression", expressions(recipes))
write("multipart", answers())
write("words", argument_lists())
