"""Read a study's cart.py for the app's recipe form, without running it: `rowver recipe <study> [--json]`.

A cart.py is in the dialect (docs/recipe-dialect.md) when a `# ---- recipe ----` line splits it
into hooks above, kept as text, and below it only FILESET, COLUMNS, LABELS and CART written as
plain values. Every mapping is reported as a list of [key, value] pairs, so its order survives
JSON readers that sort number-like keys first.
"""
import ast
import importlib.util
import io
import json
import math
import sys
import tokenize
from pathlib import Path

MARKER = "# ---- recipe ----"
NAMES = ("FILESET", "COLUMNS", "LABELS", "CART")
KEYWORDS = ("fileset", "columns", "keep", "prepare", "labels", "title", "note")


class Outside(Exception):
    """The file is valid Python but leaves the dialect; the message says where and why."""

    def __init__(self, message, node=None):
        super().__init__(f"{message} (line {node.lineno})" if node is not None else message)


# ---- reading values ----

def text(node, what):
    """A plain string literal."""
    if isinstance(node, ast.JoinedStr):
        raise Outside(f"{what} is an f-string; the form needs the text written out", node)
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    raise Outside(f"{what} is not written out as text in quotes", node)


def whole(node, what):
    """An integer literal, a negative one included; True and False are not integers here."""
    negative = isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub)
    inner = node.operand if negative else node
    if isinstance(inner, ast.Constant) and type(inner.value) is int:
        return -inner.value if negative else inner.value
    raise Outside(f"{what} is not a whole number", node)


def scalar(node, what):
    """A metadata value: text, a number, True, False, None, or a list of those."""
    if isinstance(node, ast.List):
        return [scalar(item, what) for item in node.elts]
    if isinstance(node, ast.Constant) and (node.value is None or isinstance(node.value, (str, bool))):
        return node.value
    negative = isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub)
    inner = node.operand if negative else node
    if isinstance(inner, ast.Constant) and type(inner.value) in (int, float):
        if not math.isfinite(inner.value):
            raise Outside(f"{what} is not a finite number", node)
        return -inner.value if negative else inner.value
    if isinstance(node, ast.JoinedStr):
        raise Outside(f"{what} is an f-string; the form needs the text written out", node)
    raise Outside(f"{what} must be text, a number, True, False, None or a list of those", node)


def pairs(node, what, key, value):
    """A dict literal as [[key, value], ...] in file order; a repeated key leaves the dialect."""
    if not isinstance(node, ast.Dict):
        raise Outside(f"{what} is not written out as {{...}}", node)
    out, seen = [], set()
    for k, v in zip(node.keys, node.values):
        if k is None:
            raise Outside(f"{what} unpacks another dict with **", v)
        name = key(k)
        if name in seen:
            raise Outside(f"{what} names {name!r} twice", k)
        seen.add(name)
        out.append([name, value(v, name)])
    return out


def spec(node, path):
    """One file's tree: "Events", or {"tree": "Events", "stop": 800000}."""
    if isinstance(node, ast.Dict):
        fields = dict(pairs(node, f"the entry for {path}", lambda k: text(k, "a key"), lambda v, k: v))
        if sorted(fields) != ["stop", "tree"]:
            raise Outside(f"the entry for {path} must be a tree name or {{\"tree\": ..., \"stop\": ...}}", node)
        stop = whole(fields["stop"], "stop")
        if stop < 1:
            raise Outside("stop must be 1 or more", fields["stop"])
        return {"tree": text(fields["tree"], "the tree name"), "stop": stop}
    return text(node, f"the tree name for {path}")


def sample(node, name):
    """One FILESET entry: {"files": {path: tree}, "metadata": {key: value}}."""
    fields = dict(pairs(node, f"sample {name!r}", lambda k: text(k, "a key"), lambda v, k: v))
    if sorted(fields) != ["files", "metadata"]:
        raise Outside(f"sample {name!r} must have \"files\" and \"metadata\", and nothing else", node)
    files = pairs(fields["files"], f"the files of {name!r}", lambda k: text(k, "a file name"), spec)
    metadata = pairs(fields["metadata"], f"the metadata of {name!r}", lambda k: text(k, "a metadata key"),
                     lambda v, k: scalar(v, f"metadata {k!r}"))
    return {"files": files, "metadata": metadata}


def column(node, name):
    """A branch or field by name, or the name of a hook."""
    if isinstance(node, ast.Name):
        return {"hook": node.id}
    if isinstance(node, ast.Lambda):
        raise Outside(f"column {name!r} is a lambda; the form needs a named def above the recipe line", node)
    return {"branch": text(node, f"column {name!r}")}


def labels(node, name):
    """{value: label} for one column, the values whole numbers."""
    return pairs(node, f"the labels of {name!r}", lambda k: whole(k, f"a label value of {name!r}"),
                 lambda v, k: text(v, f"the label for {k}"))


def cart(node, assigned, bound):
    """CART = Cart(...): keywords only, the four names, hook names, and text or None."""
    if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "Cart"):
        raise Outside("CART must be Cart(...)", node)
    if node.args:
        raise Outside("Cart(...) takes keywords only", node)
    given = {}
    for keyword in node.keywords:
        if keyword.arg not in KEYWORDS:
            raise Outside(f"Cart(...) has no keyword {keyword.arg or '**'}", keyword.value)
        given[keyword.arg] = keyword.value
    out = {}
    for name, value in (("fileset", "FILESET"), ("columns", "COLUMNS"), ("labels", "LABELS")):
        arg = given.get(name)
        if arg is None and name != "labels":
            raise Outside(f"Cart(...) needs {name}={value}", node)
        if name == "labels" and (arg is None or (isinstance(arg, ast.Constant) and arg.value is None)):
            if "LABELS" in assigned:
                raise Outside("LABELS is written but not passed to Cart as labels=LABELS", node)
            continue
        if not (isinstance(arg, ast.Name) and arg.id == value):
            raise Outside(f"Cart(...) must say {name}={value}", arg)
        if value not in assigned:
            raise Outside(f"{value} is passed to Cart but not written in the recipe", arg)
    for name in ("keep", "prepare"):
        arg = given.get(name)
        if arg is None and name == "keep":
            raise Outside("Cart(...) needs keep=<the name of a hook>", node)
        if arg is None or (isinstance(arg, ast.Constant) and arg.value is None and name == "prepare"):
            out[name] = None
        elif isinstance(arg, ast.Name):
            out[name] = hook(arg, bound, name)
        else:
            raise Outside(f"{name} must be the name of a hook defined above the recipe line", arg)
    for name in ("title", "note"):
        arg = given.get(name)
        out[name] = None if arg is None or (isinstance(arg, ast.Constant) and arg.value is None) \
            else text(arg, name)
    return out


def hook(node, bound, what):
    """A hook name, which the hooks region must bind."""
    if node.id not in bound:
        raise Outside(f"{what} names {node.id}, which is not defined above the recipe line", node)
    return node.id


# ---- the file ----

def marker_line(source):
    """The line number of the one `# ---- recipe ----` line, or None."""
    try:
        tokens = list(tokenize.generate_tokens(io.StringIO(source).readline))
    except (tokenize.TokenError, SyntaxError) as error:
        raise Outside(f"the file could not be split into tokens: {error}") from error
    lines = [token.start[0] for token in tokens
             if token.type == tokenize.COMMENT and token.start[1] == 0 and token.string.rstrip() == MARKER]
    if len(lines) > 1:
        raise Outside(f"the file has {len(lines)} recipe lines; it needs exactly one")
    return lines[0] if lines else None


def binds(statement):
    """The names one top-level statement binds, in order."""
    if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        return [statement.name]
    if isinstance(statement, (ast.Import, ast.ImportFrom)):
        return [alias.asname or alias.name.split(".")[0] for alias in statement.names if alias.name != "*"]
    targets = statement.targets if isinstance(statement, ast.Assign) else \
        [statement.target] if isinstance(statement, (ast.AnnAssign, ast.AugAssign)) else []
    return [node.id for target in targets for node in ast.walk(target) if isinstance(node, ast.Name)]


def siblings(tree, folder):
    """Modules the file imports that live in its own folder, or that this Python cannot find."""
    found = []
    imports = [node for node in ast.walk(tree) if isinstance(node, (ast.Import, ast.ImportFrom))]
    for statement in sorted(imports, key=lambda node: (node.lineno, node.col_offset)):  # also inside a def, try or if
        if isinstance(statement, ast.Import):
            modules = [alias.name for alias in statement.names]
        elif isinstance(statement, ast.ImportFrom) and statement.level == 0 and statement.module:
            modules = [statement.module]
        else:
            continue
        for module in modules:
            top = module.split(".")[0]
            if top in found or top == "rowver":
                continue
            beside = (folder / f"{top}.py").is_file() or (folder / top / "__init__.py").is_file()
            if beside or not findable(top):
                found.append(top)
    return found


def findable(module):
    """Whether this Python can import a top-level module, without importing it."""
    try:
        return importlib.util.find_spec(module) is not None
    except (ImportError, ValueError):
        return False


def recipe(source, folder):
    """Everything the form needs from one cart.py, or why the form cannot show it."""
    try:
        tree = ast.parse(source, filename="cart.py")
    except SyntaxError as error:
        return {"dialect": False, "reason": f"cart.py does not parse: {error.msg} (line {error.lineno})",
                "line": error.lineno, "siblings": [], "missing": []}
    near = siblings(tree, folder)
    missing = [name for name in near
               if not ((folder / f"{name}.py").is_file() or (folder / name / "__init__.py").is_file())]
    try:
        found = read(source, tree)
    except Outside as outside:
        return {"dialect": False, "reason": f"{str(outside)[0].upper()}{str(outside)[1:]}.",
                "siblings": near, "missing": missing}
    found.update(siblings=near, missing=missing)
    found["canonical"] = source == found["hooks"] + region(found)
    return found


def read(source, tree):
    """The dialect's values from a parsed file; raises Outside."""
    line = marker_line(source)
    if line is None:
        raise Outside(f"the file has no \"{MARKER}\" line, so it is read as hand-written Python")
    hooks, body = [], []
    for statement in tree.body:
        if statement.end_lineno < line:
            hooks.append(statement)
        elif statement.lineno > line:
            body.append(statement)
        else:
            raise Outside("the recipe line sits inside a statement", statement)
    bound, functions = [], []
    for statement in hooks:
        bound += [name for name in binds(statement) if name not in bound]
        if isinstance(statement, ast.FunctionDef):
            functions.append(statement.name)
    if "Cart" not in bound:
        raise Outside("Cart is not imported above the recipe line (from rowver import Cart)")
    assigned = {}
    for statement in body:
        if not (isinstance(statement, ast.Assign) and len(statement.targets) == 1
                and isinstance(statement.targets[0], ast.Name) and statement.targets[0].id in NAMES):
            raise Outside("below the recipe line only FILESET, COLUMNS, LABELS and CART may be written", statement)
        name = statement.targets[0].id
        if name in assigned:
            raise Outside(f"{name} is written twice", statement)
        if "CART" in assigned:
            raise Outside("CART must come last", statement)
        assigned[name] = statement.value
    for name in ("FILESET", "COLUMNS", "CART"):
        if name not in assigned:
            raise Outside(f"the recipe has no {name}")
    columns = pairs(assigned["COLUMNS"], "COLUMNS", lambda k: text(k, "a column name"), column)
    for name, value in columns:
        if "hook" in value:
            hook(ast.Name(id=value["hook"], lineno=assigned["COLUMNS"].lineno), bound, f"column {name!r}")
    found = {
        "dialect": True,
        "hooks": source[:line_start(source, line)],
        "bound": bound,
        "functions": functions,
        "fileset": pairs(assigned["FILESET"], "FILESET", lambda k: text(k, "a sample name"), sample),
        "columns": columns,
        "labels": pairs(assigned["LABELS"], "LABELS", lambda k: text(k, "a labelled column's name"), labels)
        if "LABELS" in assigned else [],
        "cart": cart(assigned["CART"], assigned, bound),
    }
    for name, _ in found["labels"]:
        if name not in dict(columns):
            raise Outside(f"LABELS names {name!r}, which is not a column", assigned["LABELS"])
    return found


def line_start(source, line):
    """The offset at which a 1-based line begins."""
    at = 0
    for _ in range(line - 1):
        at = source.index("\n", at) + 1
    return at


# ---- printing: the recipe region as the app's form writes it ----

def quoted(value):
    """A string in double quotes: backslash, quote and control characters escaped, the rest as is."""
    out = []
    for char in value:
        if char in '\\"':
            out.append("\\" + char)
        elif char in "\n\r\t":
            out.append({"\n": "\\n", "\r": "\\r", "\t": "\\t"}[char])
        elif ord(char) < 0x20 or 0x7F <= ord(char) <= 0x9F:
            out.append(f"\\x{ord(char):02x}")
        else:
            out.append(char)
    return '"' + "".join(out) + '"'


def literal(value):
    """One value as Python source, on one line."""
    if isinstance(value, str):
        return quoted(value)
    if isinstance(value, list):
        return "[" + ", ".join(literal(item) for item in value) + "]"
    if isinstance(value, dict):  # a file's {"tree": ..., "stop": ...}
        return "{" + ", ".join(f"{quoted(k)}: {literal(v)}" for k, v in value.items()) + "}"
    return repr(value)


def block(name, entries):
    """NAME = {...} with one entry per line, or {} when empty."""
    if not entries:
        return f"{name} = {{}}\n"
    return f"{name} = {{\n" + "".join(f"    {entry},\n" for entry in entries) + "}\n"


def region(found):
    """The recipe region, from its line to the end of the file, as the form prints it."""
    fileset = []
    for name, entry in found["fileset"]:
        files = "".join(f"            {quoted(path)}: {literal(tree)},\n" for path, tree in entry["files"])
        files = f"{{\n{files}        }}" if files else "{}"
        metadata = "{" + ", ".join(f"{quoted(k)}: {literal(v)}" for k, v in entry["metadata"]) + "}"
        fileset.append(f"{quoted(name)}: {{\n        \"files\": {files},\n        \"metadata\": {metadata},\n    }}")
    columns = [f"{quoted(name)}: {value['hook'] if 'hook' in value else quoted(value['branch'])}"
               for name, value in found["columns"]]
    labelled = [f"{quoted(name)}: {{" + ", ".join(f"{value}: {quoted(label)}" for value, label in values) + "}"
                for name, values in found["labels"]]
    c = found["cart"]
    keywords = [("fileset", "FILESET"), ("columns", "COLUMNS"), ("keep", c["keep"]),
                ("prepare", c["prepare"] or "None"), ("labels", "LABELS" if labelled else "None"),
                ("title", "None" if c["title"] is None else quoted(c["title"])),
                ("note", "None" if c["note"] is None else quoted(c["note"]))]
    parts = [block("FILESET", fileset), block("COLUMNS", columns)] + ([block("LABELS", labelled)] if labelled else [])
    parts.append("CART = Cart(\n" + "".join(f"    {k}={v},\n" for k, v in keywords) + ")\n")
    return f"{MARKER}\n" + "\n".join(parts)


# ---- the command ----

def main(study, json_out):
    """Print what the form needs from <study>/cart.py (or a .py file named directly); exit 1 when unreadable."""
    path = Path(study)
    path = path if path.suffix == ".py" else path / "cart.py"
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        message = f"{path} could not be read: {error.strerror if isinstance(error, OSError) else error}"
        if json_out:
            print(json.dumps({"event": "error", "kind": "input", "message": message}))
        else:
            print(message, file=sys.stderr)
        return 1
    found = recipe(source, path.parent)
    if json_out:
        print(json.dumps(found, ensure_ascii=False))
        return 0
    if not found["dialect"]:
        print(f"Shown as text: {found['reason']}")
    else:
        files = sum(len(entry["files"]) for _, entry in found["fileset"])
        print(f"In the form's dialect{'' if found['canonical'] else ', not yet in the printed layout'}.")
        counts = [(len(found["fileset"]), "sample"), (files, "file"), (len(found["columns"]), "column")]
        print(", ".join(f"{count:,} {noun}{'' if count == 1 else 's'}" for count, noun in counts))
        print(f"Functions above the recipe line: {', '.join(found['functions']) or 'none'}")
    if found["siblings"]:
        print(f"Imports from its folder: {', '.join(found['siblings'])}")
    if found["missing"]:
        print(f"Missing from its folder: {', '.join(found['missing'])}")
    return 0
