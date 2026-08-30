#!/usr/bin/env python
"""Authenticated local helper for RowVer's experimental structured builder."""
from __future__ import annotations

import argparse
import asyncio
import hmac
import json
import os
import resource
import secrets
import shutil
import signal
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import urlencode, urlsplit, urlunsplit

import uproot
from aiohttp import web

from .builder_spec import NAME, SpecError, compile_spec, resolve_path, validate_spec
from .press import Cart, CartError
from .rvr6 import FormatError

API = 1
DEFAULT_PORT = 8271
MAX_REQUEST = 1024 * 1024
MAX_JOBS = 2
MAX_FILES = 128
MAX_ROWS = 10_000_000
TIMEOUT_SECONDS = 15 * 60
MEMORY_BYTES = 4 * 1024 * 1024 * 1024


class ApiError(Exception):
    def __init__(self, status: int, code: str, message: str):
        super().__init__(message)
        self.status, self.code = status, code


def envelope_error(status: int, code: str, message: str) -> web.Response:
    return web.json_response({"ok": False, "error": {"code": code, "message": message}},
                             status=status)


def _host_ok(host: str, port: int) -> bool:
    return host in {f"127.0.0.1:{port}", f"localhost:{port}", "127.0.0.1", "localhost"}


def _cors(response: web.StreamResponse, origin: str | None) -> web.StreamResponse:
    if origin is not None:
        response.headers["Access-Control-Allow-Origin"] = origin
        response.headers["Vary"] = "Origin"
    response.headers["X-Content-Type-Options"] = "nosniff"
    response.headers["Cache-Control"] = "no-store"
    return response


async def _json(request: web.Request) -> Any:
    try:
        return await request.json(loads=json.loads)
    except (json.JSONDecodeError, UnicodeDecodeError, TypeError) as exc:
        raise ApiError(400, "request.json", "request body must be valid JSON") from exc


@dataclass
class Job:
    id: str
    name: str
    directory: Path
    status: str = "queued"
    error: dict | None = None
    result: Path | None = None
    task: asyncio.Task | None = None
    process: asyncio.subprocess.Process | None = None

    def public(self) -> dict:
        out = {"id": self.id, "status": self.status}
        if self.error is not None:
            out["error"] = self.error
        return out


@dataclass
class Service:
    roots: dict[str, Path]
    labels: dict[str, str]
    token: str
    origins: set[str]
    port: int
    max_jobs: int = MAX_JOBS
    max_files: int = MAX_FILES
    max_rows: int = MAX_ROWS
    timeout: int = TIMEOUT_SECONDS
    jobs: dict[str, Job] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self.roots = {key: path.resolve() for key, path in self.roots.items()}
        self.workspace = Path(tempfile.mkdtemp(prefix="rowver-builder-"))
        self.workspace.chmod(0o700)
        self.slots = asyncio.Semaphore(self.max_jobs)

    async def close(self) -> None:
        for job in self.jobs.values():
            if job.status in {"queued", "running"}:
                job.status = "cancelled"
            if job.process and job.process.returncode is None:
                job.process.terminate()
        tasks = [job.task for job in self.jobs.values() if job.task]
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        shutil.rmtree(self.workspace, ignore_errors=True)

    def safe_path(self, source: str, relative: str, *, file: bool = False) -> Path:
        try:
            path = resolve_path(self.roots, source, relative, must_exist=file)
        except SpecError as exc:
            raise ApiError(400, exc.code, str(exc)) from exc
        if not file and not path.is_dir():
            raise ApiError(400, "path.directory", f"{source}:{relative} is not a directory")
        return path

    async def run_job(self, job: Job, spec: dict) -> None:
        async with self.slots:
            if job.status == "cancelled":
                return
            job.status = "running"
            spec_path = job.directory / "spec.json"
            roots_path = job.directory / "roots.json"
            result_path = job.directory / f"{job.name}.rvr"
            spec_path.write_text(json.dumps(spec, allow_nan=False), encoding="utf-8")
            roots_path.write_text(json.dumps({key: str(path) for key, path in self.roots.items()}),
                                  encoding="utf-8")
            command = [sys.executable, "-m", "rowver.builder", "--worker",
                       str(spec_path), str(roots_path), str(result_path),
                       "--max-files", str(self.max_files), "--max-rows", str(self.max_rows),
                       "--timeout", str(self.timeout)]
            try:
                job.process = await asyncio.create_subprocess_exec(
                    *command, cwd=Path(__file__).resolve().parents[1],
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                if job.status == "cancelled" and job.process.returncode is None:
                    job.process.terminate()
                stdout, stderr = await asyncio.wait_for(job.process.communicate(), self.timeout + 5)
            except asyncio.CancelledError:
                if job.process and job.process.returncode is None:
                    job.process.terminate()
                    try:
                        await asyncio.wait_for(job.process.communicate(), 3)
                    except asyncio.TimeoutError:
                        job.process.kill()
                        await job.process.communicate()
                job.status = "cancelled"
                return
            except asyncio.TimeoutError:
                if job.process and job.process.returncode is None:
                    job.process.kill()
                    await job.process.wait()
                job.status = "failed"
                job.error = {"code": "job.timeout", "message": "build exceeded its time limit"}
                return
            except Exception as exc:
                job.status = "failed"
                job.error = {"code": "job.start", "message": str(exc)[:500]}
                return
            if job.status == "cancelled":
                return
            if job.process.returncode != 0 or not result_path.is_file():
                message = stderr.decode("utf-8", "replace").strip().splitlines()
                job.status = "failed"
                job.error = {"code": "job.build", "message": (message[-1] if message else
                              stdout.decode("utf-8", "replace").strip() or "build failed")[:1000]}
                return
            job.result = result_path
            job.status = "complete"


def create_app(service: Service) -> web.Application:
    @web.middleware
    async def guard(request: web.Request, handler):
        origin = request.headers.get("Origin")
        if not _host_ok(request.host, service.port):
            return envelope_error(403, "request.host", "Host must be loopback")
        if origin is not None and origin not in service.origins:
            return envelope_error(403, "request.origin", "Origin is not allowed")
        if request.method == "OPTIONS":
            response = web.Response(status=204)
            response.headers["Access-Control-Allow-Methods"] = "GET, POST, DELETE, OPTIONS"
            response.headers["Access-Control-Allow-Headers"] = "Authorization, Content-Type"
            response.headers["Access-Control-Max-Age"] = "600"
            return _cors(response, origin)
        supplied = request.headers.get("Authorization", "")
        if not supplied.startswith("Bearer ") or not hmac.compare_digest(
                supplied.removeprefix("Bearer "), service.token):
            return _cors(envelope_error(401, "auth.bearer", "valid bearer token required"), origin)
        try:
            response = await handler(request)
        except ApiError as exc:
            response = envelope_error(exc.status, exc.code, str(exc))
        except SpecError as exc:
            response = envelope_error(400, exc.code, str(exc))
        except web.HTTPException as exc:
            response = envelope_error(exc.status, "request.http", exc.reason)
        except Exception:
            response = envelope_error(500, "server.internal", "internal helper error")
        return _cors(response, origin)

    app = web.Application(middlewares=[guard], client_max_size=MAX_REQUEST)

    async def session(_: web.Request) -> web.Response:
        return web.json_response({
            "ok": True, "api": API,
            "sources": [{"id": key, "label": service.labels[key]} for key in service.roots],
            "limits": {"max_jobs": service.max_jobs, "max_files": service.max_files,
                       "max_rows": service.max_rows, "timeout_s": service.timeout},
        })

    async def listing(request: web.Request) -> web.Response:
        body = await _json(request)
        if not isinstance(body, dict) or set(body) != {"source", "path"}:
            raise ApiError(400, "request.shape", "list body must contain source and path")
        if not isinstance(body["source"], str) or not isinstance(body["path"], str):
            raise ApiError(400, "request.shape", "source and path must be strings")
        directory = service.safe_path(body["source"], body["path"])
        entries = []
        for child in sorted(directory.iterdir(), key=lambda item: item.name.casefold()):
            try:
                resolved = child.resolve()
            except OSError:
                continue
            if not resolved.is_relative_to(service.roots[body["source"]]) or child.name.startswith("."):
                continue
            kind = "directory" if child.is_dir() else "root" if child.is_file() and child.suffix == ".root" else None
            if kind:
                relative = child.relative_to(service.roots[body["source"]]).as_posix()
                entries.append({"name": child.name, "path": relative, "kind": kind})
        return web.json_response({"ok": True, "entries": entries})

    def inspect_file(source: str, relative: str) -> dict:
        path = service.safe_path(source, relative, file=True)
        if path.suffix != ".root":
            raise ApiError(400, "path.root", "inspect accepts ROOT files only")
        trees = []
        try:
            with uproot.open(path) as root:
                for name, classname in root.classnames(recursive=True, cycle=False).items():
                    if "TTree" not in classname and "RNTuple" not in classname:
                        continue
                    tree = root[name]
                    typenames = tree.typenames()
                    branches = [{"name": branch, "typename": typename,
                                 "jagged": "[]" in typename or "vector<" in typename}
                                for branch, typename in sorted(typenames.items())]
                    trees.append({"name": name, "entries": int(tree.num_entries),
                                  "branches": branches})
        except Exception as exc:
            raise ApiError(400, "root.inspect", f"cannot inspect ROOT file: {exc}") from exc
        return {"source": source, "path": relative, "trees": trees}

    async def inspect(request: web.Request) -> web.Response:
        body = await _json(request)
        if not isinstance(body, dict) or set(body) != {"source", "path"}:
            raise ApiError(400, "request.shape", "inspect body must contain source and path")
        if not isinstance(body["source"], str) or not isinstance(body["path"], str):
            raise ApiError(400, "request.shape", "source and path must be strings")
        result = await asyncio.to_thread(inspect_file, body["source"], body["path"])
        return web.json_response({"ok": True, "file": result})

    async def compile_route(request: web.Request) -> web.Response:
        spec = await _json(request)
        source = await asyncio.to_thread(compile_spec, spec, service.roots)
        return web.json_response({"ok": True, "cart_py": source})

    async def start_job(request: web.Request) -> web.Response:
        spec = await _json(request)
        validate_spec(spec, set(service.roots))
        active = sum(job.status in {"queued", "running"} for job in service.jobs.values())
        if active >= service.max_jobs:
            raise ApiError(429, "limit.jobs", "all builder job slots are in use")
        if sum(len(sample["files"]) for sample in spec["samples"]) > service.max_files:
            raise ApiError(400, "limit.files", "study exceeds the helper file limit")
        await asyncio.to_thread(compile_spec, spec, service.roots)
        identifier = secrets.token_urlsafe(12)
        directory = service.workspace / identifier
        directory.mkdir(mode=0o700)
        job = Job(identifier, spec["name"], directory)
        service.jobs[identifier] = job
        job.task = asyncio.create_task(service.run_job(job, spec))
        return web.json_response({"ok": True, "job": job.public()}, status=202)

    def job_of(request: web.Request) -> Job:
        job = service.jobs.get(request.match_info["id"])
        if job is None:
            raise ApiError(404, "job.missing", "unknown job")
        return job

    async def job_status(request: web.Request) -> web.Response:
        return web.json_response({"ok": True, "job": job_of(request).public()})

    async def cancel_job(request: web.Request) -> web.Response:
        job = job_of(request)
        if job.status in {"queued", "running"}:
            job.status = "cancelled"
            if job.process and job.process.returncode is None:
                job.process.terminate()
            if job.task:
                try:
                    await asyncio.wait_for(asyncio.shield(job.task), 3)
                except asyncio.TimeoutError:
                    if job.process and job.process.returncode is None:
                        job.process.kill()
                    await job.task
        return web.json_response({"ok": True, "job": job.public()})

    async def result(request: web.Request) -> web.StreamResponse:
        job = job_of(request)
        if job.status != "complete" or job.result is None:
            raise ApiError(409, "job.incomplete", "job has no completed result")
        response = web.FileResponse(job.result, headers={
            "Content-Type": "application/octet-stream",
            "Content-Disposition": f'attachment; filename="{job.name}.rvr"',
        })
        return response

    app.router.add_get("/v1/session", session)
    app.router.add_post("/v1/list", listing)
    app.router.add_post("/v1/inspect", inspect)
    app.router.add_post("/v1/compile", compile_route)
    app.router.add_post("/v1/jobs", start_job)
    app.router.add_get("/v1/jobs/{id}", job_status)
    app.router.add_delete("/v1/jobs/{id}", cancel_job)
    app.router.add_get("/v1/jobs/{id}/result", result)

    async def cleanup(_: web.Application) -> None:
        await service.close()

    app.on_cleanup.append(cleanup)
    return app


def _limits(timeout: int) -> None:
    resource.setrlimit(resource.RLIMIT_CPU, (timeout, timeout + 5))
    resource.setrlimit(resource.RLIMIT_FSIZE, (2 * 1024 ** 3, 2 * 1024 ** 3))
    try:
        resource.setrlimit(resource.RLIMIT_AS, (MEMORY_BYTES, MEMORY_BYTES))
    except (ValueError, OSError):
        pass


def worker(spec_path: Path, roots_path: Path, output: Path,
           max_files: int, max_rows: int, timeout: int) -> int:
    _limits(timeout)
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    roots = {key: Path(path).resolve() for key, path in
             json.loads(roots_path.read_text(encoding="utf-8")).items()}
    validate_spec(spec, set(roots))
    files = sum(len(sample["files"]) for sample in spec["samples"])
    if files > max_files:
        raise SpecError("limit.files", "study exceeds the helper file limit")
    total = 0
    for sample in spec["samples"]:
        for file in sample["files"]:
            path = resolve_path(roots, file["source"], file["path"])
            with uproot.open(path) as root:
                total += int(root[file["tree"]].num_entries)
            if total > max_rows:
                raise SpecError("limit.rows", "study exceeds the helper input-row limit")
    source = compile_spec(spec, roots)
    studies = spec_path.parent / "studies"
    cart_dir = studies / spec["name"]
    cart_dir.mkdir(parents=True)
    (cart_dir / "cart.py").write_text(source, encoding="utf-8")
    cart = Cart.load(spec["name"], studies)
    cart.build(output)
    return 0


def parse_roots(values: list[str]) -> tuple[dict[str, Path], dict[str, str]]:
    roots, labels = {}, {}
    for value in values:
        if "=" not in value:
            raise SystemExit("--root must be ID=/absolute/path")
        identifier, raw = value.split("=", 1)
        if NAME.fullmatch(identifier) is None or identifier in roots:
            raise SystemExit(f"invalid or duplicate root id {identifier!r}")
        path = Path(raw).expanduser().resolve()
        if not path.is_dir():
            raise SystemExit(f"root {identifier!r} is not a directory: {path}")
        roots[identifier], labels[identifier] = path, identifier
    if not roots:
        raise SystemExit("at least one --root ID=/path is required")
    return roots, labels


def viewer_url(raw: str | None) -> str | None:
    """Resolve an explicitly chosen local or hosted RowVer 6 viewer."""
    if raw is None:
        return None
    parsed = urlsplit(raw)
    if not parsed.scheme:
        return Path(raw).expanduser().resolve().as_uri()
    if (parsed.scheme not in {"file", "http", "https"} or parsed.username is not None
            or parsed.password is not None or parsed.scheme in {"http", "https"}
            and not parsed.netloc):
        raise SystemExit("--ui must be a file path or an http(s)/file URL")
    return urlunsplit((parsed.scheme, parsed.netloc, parsed.path, parsed.query, ""))


def parse(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", action="append", default=[], metavar="ID=PATH")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--origin", action="append", default=[])
    parser.add_argument("--ui", metavar="PATH_OR_URL",
                        help="viewer to open with the private pairing fragment")
    parser.add_argument("--max-files", type=int, default=MAX_FILES)
    parser.add_argument("--max-rows", type=int, default=MAX_ROWS)
    parser.add_argument("--timeout", type=int, default=TIMEOUT_SECONDS)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("worker_args", nargs="*")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse(argv)
    if args.worker:
        if len(args.worker_args) != 3:
            raise SystemExit("worker requires SPEC ROOTS OUTPUT")
        try:
            return worker(*(Path(value) for value in args.worker_args),
                          args.max_files, args.max_rows, args.timeout)
        except (SpecError, CartError, FormatError, OSError, KeyError, ValueError) as exc:
            print(f"{getattr(exc, 'code', 'job.build')}: {exc}", file=sys.stderr)
            return 1
    if not 1024 <= args.port <= 65535:
        raise SystemExit("--port must be between 1024 and 65535")
    roots, labels = parse_roots(args.root)
    token = secrets.token_urlsafe(32)
    ui = viewer_url(args.ui)
    origins = {"null", "https://rowver.web.cern.ch", "http://localhost:5173",
               "http://127.0.0.1:5173", *args.origin}
    if ui:
        parsed = urlsplit(ui)
        if parsed.scheme in {"http", "https"}:
            origins.add(f"{parsed.scheme}://{parsed.netloc}")
    service = Service(roots, labels, token, origins, args.port,
                      max_files=args.max_files, max_rows=args.max_rows, timeout=args.timeout)
    fragment = urlencode({"helper": f"http://127.0.0.1:{args.port}", "token": token})
    print(f"Pair RowVer: {ui + '#' if ui else '#'}{fragment}", flush=True)
    web.run_app(create_app(service), host="127.0.0.1", port=args.port, print=None)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
