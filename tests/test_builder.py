from __future__ import annotations

import asyncio
import tempfile
import unittest
from pathlib import Path

import numpy as np
import uproot
from aiohttp.test_utils import TestClient, TestServer

from builder import Service, create_app, viewer_url
from builder_spec import SpecError, compile_spec, validate_spec
from cart import Cart
import rvr6


def study(source: str = "data", path: str = "events.root") -> dict:
    return {
        "schema": 1,
        "name": "mine",
        "samples": [
            {"name": "simulation", "files": [{"source": source, "path": path,
                                                 "tree": "Events"}],
             "weights": {"branch": "genWeight"}},
            {"name": "observed", "files": [{"source": source, "path": path,
                                               "tree": "Events"}],
             "weights": None},
        ],
        "columns": [
            {"name": "pt", "source": {"op": "branch", "name": "pt"}},
            {"name": "abs_pt", "source": {"op": "abs", "value": {
                "op": "branch", "name": "pt"}}},
        ],
        "keep": {"op": "compare", "left": {"op": "branch", "name": "pt"},
                 "operator": ">", "right": {"op": "scalar", "value": 10}},
        "labels": {},
    }


class BuilderSpecTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        with uproot.recreate(self.root / "events.root") as output:
            output["Events"] = {
                "pt": np.array([5.0, 15.0, 25.0]),
                "genWeight": np.array([0.5, 1.0, 2.0]),
            }

    def tearDown(self):
        self.temp.cleanup()

    def test_compiler_emits_current_cart_api_and_explicit_weight_policy(self):
        source = compile_spec(study(), {"data": self.root})
        self.assertNotIn("name=", source)
        self.assertIn("'weights': 'genWeight'", source)
        self.assertIn("'weights': None", source)
        studies = self.root / "studies"
        target = studies / "mine"
        target.mkdir(parents=True)
        (target / "cart.py").write_text(source, encoding="utf-8")
        cart = Cart.load("mine", studies)
        self.assertEqual(cart.name, "mine")
        self.assertEqual(cart.fileset["observed"]["metadata"]["weights"], None)

    def test_schema_and_path_jail_refuse_injection_shapes(self):
        bad = study(path="../secret.root")
        with self.assertRaisesRegex(SpecError, "safe source-relative"):
            validate_spec(bad, {"data"})
        bad = study()
        bad["keep"]["unknown"] = "exec"
        with self.assertRaises(SpecError):
            validate_spec(bad, {"data"})
        bad = study()
        bad["labels"] = {"pt": {'0\n__import__("os")': "bad"}}
        with self.assertRaises(SpecError):
            compile_spec(bad, {"data": self.root})
        bad = study()
        bad["columns"][0]["source"] = {"op": "branch", "name": "x']\nraise SystemExit()"}
        source = compile_spec(bad, {"data": self.root})
        compile(source, "cart.py", "exec")

    def test_pairing_viewer_is_explicit_and_fragment_safe(self):
        local = self.root / "viewer.html"
        self.assertEqual(viewer_url(str(local)), local.resolve().as_uri())
        self.assertEqual(viewer_url("https://rowver.example/viewer?x=1#old"),
                         "https://rowver.example/viewer?x=1")
        with self.assertRaises(SystemExit):
            viewer_url("javascript:alert(1)")


class BuilderApiTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        with uproot.recreate(self.root / "events.root") as output:
            output["Events"] = {
                "pt": np.array([5.0, 15.0, 25.0]),
                "genWeight": np.array([0.5, 1.0, 2.0]),
            }
        self.service = Service({"data": self.root}, {"data": "Local data"}, "secret",
                               {"null", "http://localhost:5173"}, 0,
                               max_jobs=1, max_files=4, max_rows=20, timeout=30)
        self.server = TestServer(create_app(self.service))
        self.client = TestClient(self.server)
        await self.client.start_server()
        self.service.port = self.server.port
        self.auth = {"Authorization": "Bearer secret", "Origin": "null"}

    async def asyncTearDown(self):
        await self.client.close()
        self.temp.cleanup()

    async def test_auth_origin_list_inspect_and_compile(self):
        response = await self.client.get("/v1/session")
        self.assertEqual(response.status, 401)
        response = await self.client.get("/v1/session", headers={
            "Authorization": "Bearer secret", "Origin": "https://evil.example"})
        self.assertEqual(response.status, 403)
        response = await self.client.get("/v1/session", headers=self.auth)
        self.assertEqual(response.status, 200)
        self.assertEqual((await response.json())["api"], 1)

        response = await self.client.post("/v1/list", headers=self.auth,
                                          json={"source": "data", "path": ""})
        self.assertEqual((await response.json())["entries"], [
            {"name": "events.root", "path": "events.root", "kind": "root"}])
        response = await self.client.post("/v1/list", headers=self.auth,
                                          json={"source": "data", "path": ".."})
        self.assertEqual(response.status, 400)

        response = await self.client.post("/v1/inspect", headers=self.auth,
                                          json={"source": "data", "path": "events.root"})
        payload = await response.json()
        self.assertEqual(payload["file"]["trees"][0]["entries"], 3)
        self.assertIn("pt", [branch["name"] for branch in payload["file"]["trees"][0]["branches"]])

        response = await self.client.post("/v1/compile", headers=self.auth, json=study())
        self.assertEqual(response.status, 200)
        self.assertIn("CART = Cart(", (await response.json())["cart_py"])

    async def test_job_builds_private_download_and_can_be_cancelled(self):
        response = await self.client.post("/v1/jobs", headers=self.auth, json=study())
        self.assertEqual(response.status, 202)
        identifier = (await response.json())["job"]["id"]
        status = "queued"
        for _ in range(100):
            await asyncio.sleep(0.05)
            response = await self.client.get(f"/v1/jobs/{identifier}", headers=self.auth)
            payload = await response.json()
            status = payload["job"]["status"]
            if status in {"complete", "failed"}:
                break
        self.assertEqual(status, "complete", payload)
        response = await self.client.get(f"/v1/jobs/{identifier}/result", headers=self.auth)
        self.assertEqual(response.status, 200)
        compressed = await response.read()
        path = self.root / "download.rvr"
        path.write_bytes(compressed)
        meta, _ = rvr6.read(path)
        self.assertEqual(meta["about"]["name"], "mine")
        self.assertEqual([sample["weighted"] for sample in meta["data"]["samples"]],
                         [True, False])

        response = await self.client.post("/v1/jobs", headers=self.auth, json=study())
        other = (await response.json())["job"]["id"]
        response = await self.client.delete(f"/v1/jobs/{other}", headers=self.auth)
        self.assertEqual((await response.json())["job"]["status"], "cancelled")


if __name__ == "__main__":
    unittest.main()
