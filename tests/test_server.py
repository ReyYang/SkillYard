from __future__ import annotations

import http.client
import json
import queue
import tempfile
import threading
import unittest
from pathlib import Path
from typing import Any

from skillyard.app import SkillYardApp
from skillyard.server import _make_server


class LocalServerHarness:
    def __init__(self, root: Path):
        self.root = root
        self.address: tuple[str, int] | None = None
        self.identity: str | None = None
        self._ready: queue.Queue[tuple[str, Any]] = queue.Queue()
        self._server = None
        self._thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        self._thread.start()
        status, payload = self._ready.get(timeout=5)
        if status == "error":
            raise payload
        self.address, self.identity = payload

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
        self._thread.join(timeout=5)
        if self._thread.is_alive():
            raise AssertionError("server thread did not stop")

    def request(self, method: str, path: str, body: dict[str, Any] | None = None):
        if self.address is None:
            raise AssertionError("server is not started")
        payload = None if body is None else json.dumps(body).encode("utf-8")
        headers = {}
        if payload is not None:
            headers["Content-Type"] = "application/json"
        conn = http.client.HTTPConnection(*self.address, timeout=5)
        try:
            conn.request(method, path, body=payload, headers=headers)
            response = conn.getresponse()
            data = response.read()
            return response.status, response.getheaders(), data
        finally:
            conn.close()

    def request_json(self, method: str, path: str, body: dict[str, Any] | None = None) -> dict[str, Any]:
        status, _headers, data = self.request(method, path, body)
        self.assert_ok(status, data)
        return json.loads(data.decode("utf-8"))

    def assert_ok(self, status: int, data: bytes) -> None:
        if status != 200:
            raise AssertionError(f"expected HTTP 200, got {status}: {data.decode('utf-8', errors='replace')}")

    def _run(self) -> None:
        app = SkillYardApp(self.root / "skillyard-home", self.root / "host-home")
        try:
            app.apply(app.init())
            result = app.import_local(self.root / "source", "demo")
            self._server = _make_server(app, "127.0.0.1", 0)
            self._ready.put(("ready", (self._server.server_address, result["entries"][0]["identity"])))
            self._server.serve_forever()
        except BaseException as exc:
            self._ready.put(("error", exc))
        finally:
            if self._server is not None:
                self._server.server_close()
            app.close()


class ServerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        skill_dir = self.root / "source" / "demo-skill"
        skill_dir.mkdir(parents=True)
        (skill_dir / "SKILL.md").write_text(
            "---\nname: demo-skill\ndescription: Demo skill\n---\n\nBody\n",
            encoding="utf-8",
        )
        self.server = LocalServerHarness(self.root)
        self.server.start()

    def tearDown(self) -> None:
        self.server.stop()
        self.temp.cleanup()

    def test_html_view_has_required_sections(self) -> None:
        status, headers, data = self.server.request("GET", "/")

        self.server.assert_ok(status, data)
        header_map = {key.lower(): value for key, value in headers}
        self.assertIn("text/html", header_map["content-type"])
        html = data.decode("utf-8")
        self.assertIn("Source Trees", html)
        self.assertIn("Library Entries", html)
        self.assertIn("Exposures", html)
        self.assertIn("Doctor Findings", html)

    def test_state_and_doctor_endpoints_return_json(self) -> None:
        state = self.server.request_json("GET", "/api/state")
        doctor = self.server.request_json("GET", "/api/doctor")

        self.assertEqual("demo", state["sourceTrees"][0]["namespace"])
        self.assertEqual(self.server.identity, state["libraryEntries"][0]["identity"])
        self.assertIn("exposures", state)
        self.assertEqual([], doctor["findings"])

    def test_expose_plan_and_apply_use_temp_host_directory(self) -> None:
        host_dir = self.root / "host-skills"
        plan = self.server.request_json(
            "POST",
            "/api/expose/plan",
            {
                "identity": self.server.identity,
                "host": "codex",
                "scope": "user",
                "mode": "snapshot",
                "hostDirOverride": str(host_dir),
            },
        )

        self.assertEqual("创建 Exposure", plan["title"])
        self.assertEqual("expose", plan["payload"]["kind"])

        result = self.server.request_json("POST", "/api/apply", {"planId": plan["planId"]})

        self.assertTrue(result["ok"])
        self.assertTrue((host_dir / "demo-skill" / "SKILL.md").exists())
        state = self.server.request_json("GET", "/api/state")
        self.assertEqual("demo-skill", state["exposures"][0]["host_entry_name"])

    def test_apply_rejects_unissued_plan(self) -> None:
        status, _headers, data = self.server.request("POST", "/api/apply", {"planId": "missing"})

        self.assertEqual(400, status)
        self.assertIn("unknown or expired planId", data.decode("utf-8"))


if __name__ == "__main__":
    unittest.main()
