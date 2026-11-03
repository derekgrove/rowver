"""Plan a release from a vX.Y.Z tag, or rehearse one on main: which crates go to crates.io and
whether the Python package goes to PyPI. A version already published with the same contents is skipped, so a release
that stopped part way can run again; one published with other contents stops the release."""

import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.request

# A rehearsal (the workflow run by hand on main) pushes no tag: what fixes a mistake differs.
REHEARSAL = os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
AGAIN = "then rehearse again" if REHEARSAL else "then tag again"

CRATES = ["rvr-recipe", "rowver", "rvr"]  # the order they are published in: each needs the ones before
AGENT = {"User-Agent": "rowver-release (https://gitlab.cern.ch/dgrove/rowver)"}


def fail(message):
    print(f"release: {message}", file=sys.stderr)
    sys.exit(1)


def get(url):
    """The body at a URL, or None when it is not there (404)."""
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers=AGENT), timeout=60) as reply:
            return reply.read()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        fail(f"{url} answered {e.code}; try the release again later")


def run(*command, cwd=None):
    done = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if done.returncode:
        fail(f"{' '.join(command)} failed:\n{done.stderr.strip()}")
    return done.stdout


def members(data):
    """A .tar.gz as {path below its top folder: bytes}."""
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        return {m.name.split("/", 1)[1]: tar.extractfile(m).read() for m in tar if m.isfile()}


def manifest(text):
    """What a packaged Cargo.toml says that matters: its dependencies and features, not the layout
    that differs between cargo versions."""
    toml = tomllib.loads(text.decode())
    keys = ("dependencies", "build-dependencies", "features", "target")
    return {k: toml.get(k) for k in keys} | {"version": toml["package"]["version"]}


def lock(text):
    return sorted((p["name"], p["version"]) for p in tomllib.loads(text.decode()).get("package", []))


def differences(ours, theirs, skip=(), special=None):
    """The paths whose contents differ between two packages."""
    special = special or {}
    paths = sorted((set(ours) | set(theirs)) - set(skip))
    same = lambda p, a, b: special[p](a) == special[p](b) if p in special and a and b else a == b
    return [p for p in paths if not same(p, ours.get(p), theirs.get(p))]


def crate_plan(name, version):
    """True when the crate is to be published; stops when crates.io holds this version with other
    contents."""
    if get(f"https://crates.io/api/v1/crates/{name}/{version}") is None:
        return True
    published = get(f"https://static.crates.io/crates/{name}/{name}-{version}.crate")
    with tempfile.TemporaryDirectory() as out:
        run("cargo", "package", "-p", name, "--locked", "--no-verify", "--target-dir", out)
        with open(f"{out}/package/{name}-{version}.crate", "rb") as f:
            ours = members(f.read())
    changed = differences(ours, members(published), skip=[".cargo_vcs_info.json"],
                          special={"Cargo.toml": manifest, "Cargo.lock": lock})
    if changed:
        fail(f"{name} {version} is on crates.io with other contents ({', '.join(changed[:5])}). "
             f"Raise the version in Cargo.toml, {AGAIN}.")
    return False


def python_plan(version):
    """True when the Python package is to be published; stops when PyPI holds this version with
    another source package."""
    info = get(f"https://pypi.org/pypi/rowver/{version}/json")
    if info is None:
        return True
    sdists = [u["url"] for u in json.loads(info)["urls"] if u["packagetype"] == "sdist"]
    if not sdists:
        fail(f"rowver {version} is on PyPI with no source package; raise the version in Cargo.toml")
    with tempfile.TemporaryDirectory() as out:
        run("maturin", "sdist", "--out", out, cwd="crates/rowver-py")
        with open(f"{out}/rowver-{version}.tar.gz", "rb") as f:
            ours = members(f.read())
    # COMMIT, which the sdist job writes, names the commit, as .cargo_vcs_info.json does a crate's.
    changed = differences(ours, members(get(sdists[0])), skip=["PKG-INFO", "COMMIT"])
    if changed:
        fail(f"rowver {version} is on PyPI with other contents ({', '.join(changed[:5])}). "
             f"Raise the version in Cargo.toml, {AGAIN}.")
    return False


def main():
    tag = os.environ.get("TAG") or fail("TAG is not set: run this for a vX.Y.Z tag")
    meta = json.loads(run("cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"))
    versions = {p["name"]: p["version"] for p in meta["packages"]}
    version = versions["rowver"]
    others = [f"{n} {v}" for n, v in versions.items() if v != version]
    if others:
        fail(f"every crate shares the workspace version {version}, but {', '.join(others)}: "
             f"give each crate version.workspace = true")
    if tag != f"v{version}" and REHEARSAL:
        fail(f"the rehearsal names {tag}, but Cargo.toml's version is {version}: run it naming v{version}")
    if tag != f"v{version}":
        fail(f"the tag {tag} does not name the version {version} (Cargo.toml): "
             f"tag v{version}, or raise the version first")
    if subprocess.run(["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"]).returncode:
        fail("a rehearsal runs on main: start it there" if REHEARSAL else f"{tag} is not on main: tag a commit of main")
    # The commit planned, so that a rehearsal and the tag after it can be told to be the same.
    print(f"{'Rehearsing' if REHEARSAL else 'Releasing'} {tag} from commit {run('git', 'rev-parse', 'HEAD').strip()}")
    crates = [c for c in CRATES if crate_plan(c, version)]
    python = python_plan(version)
    for c in CRATES:
        print(f"{c} {version}: {'publish' if c in crates else 'already on crates.io'}")
    print(f"Python rowver {version}: {'publish' if python else 'already on PyPI'}")
    with open(os.environ.get("GITHUB_OUTPUT", os.devnull), "a") as out:
        out.write(f"crates={' '.join(crates)}\npython={str(python).lower()}\nversion={version}\n")


if __name__ == "__main__":
    main()
