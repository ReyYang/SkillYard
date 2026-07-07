from __future__ import annotations

import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .models import Plan


def run_server(app: Any, host: str, port: int) -> None:
    if host not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("Local Server 只能绑定 localhost")
    server = _make_server(app, host, port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


def _make_server(app: Any, host: str, port: int) -> HTTPServer:
    return HTTPServer((host, port), _handler_for(app))


def _handler_for(app: Any) -> type[BaseHTTPRequestHandler]:
    pending_plans: dict[str, Plan] = {}

    class SkillYardHandler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            path = urlsplit(self.path).path
            if path == "/":
                self._send_html(_html_page())
                return
            if path == "/api/state":
                self._send_json(app.state_snapshot())
                return
            if path == "/api/doctor":
                self._send_json({"findings": app.doctor()})
                return
            self._send_json({"error": "not found"}, status=404)

        def do_POST(self) -> None:
            path = urlsplit(self.path).path
            try:
                data = self._read_json()
                if path == "/api/expose/plan":
                    plan = app.expose_plan(
                        identity=str(data["identity"]),
                        host=str(data.get("host", "codex")),
                        scope=str(data.get("scope", "user")),
                        project_root=_optional_path(data, "projectRoot", "project_root"),
                        host_dir_override=_optional_path(data, "hostDirOverride", "host_dir_override"),
                        mode=str(data.get("mode", "auto")),
                        entry_name=_optional_str(data, "entryName", "entry_name"),
                        on_conflict=str(data.get("onConflict", data.get("on_conflict", "error"))),
                    )
                    plan_id = _plan_id(plan)
                    pending_plans[plan_id] = plan
                    response = plan.as_dict()
                    response["planId"] = plan_id
                    self._send_json(response)
                    return
                if path == "/api/apply":
                    plan_id = str(data.get("planId", data.get("plan_id", "")))
                    if not plan_id or plan_id not in pending_plans:
                        raise ValueError("unknown or expired planId")
                    plan = pending_plans.pop(plan_id)
                    self._send_json(app.apply(plan))
                    return
            except (KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
                self._send_json({"error": str(exc)}, status=400)
                return
            self._send_json({"error": "not found"}, status=404)

        def log_message(self, format: str, *args: Any) -> None:
            return

        def _read_json(self) -> dict[str, Any]:
            length = int(self.headers.get("Content-Length", "0") or 0)
            raw = self.rfile.read(length) if length else b"{}"
            data = json.loads(raw.decode("utf-8") if raw else "{}")
            if not isinstance(data, dict):
                raise ValueError("JSON body must be an object")
            return data

        def _send_json(self, data: Any, status: int = 200) -> None:
            body = json.dumps(data, ensure_ascii=False, sort_keys=True).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _send_html(self, markup: str, status: int = 200) -> None:
            body = markup.encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    return SkillYardHandler


def _plan_id(plan: Plan) -> str:
    import hashlib
    import json

    return hashlib.sha256(json.dumps(plan.as_dict(), sort_keys=True).encode("utf-8")).hexdigest()[:16]


def _optional_path(data: dict[str, Any], *keys: str) -> Path | None:
    value = _first_value(data, *keys)
    if value in (None, ""):
        return None
    return Path(str(value))


def _optional_str(data: dict[str, Any], *keys: str) -> str | None:
    value = _first_value(data, *keys)
    if value in (None, ""):
        return None
    return str(value)


def _first_value(data: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        if key in data:
            return data[key]
    return None


def _html_page() -> str:
    return """<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>SkillYard Local Server</title>
  <style>
    body {
      margin: 0;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      color: #202124;
      background: #f7f8fa;
    }
    main {
      max-width: 1040px;
      margin: 0 auto;
      padding: 24px;
    }
    h1 {
      font-size: 28px;
      margin: 0 0 20px;
      letter-spacing: 0;
    }
    section {
      margin: 16px 0;
      padding: 16px;
      border: 1px solid #d7dbe0;
      border-radius: 8px;
      background: #fff;
    }
    h2 {
      font-size: 18px;
      margin: 0 0 12px;
      letter-spacing: 0;
    }
    pre {
      margin: 0;
      white-space: pre-wrap;
      word-break: break-word;
      font-size: 13px;
      line-height: 1.45;
    }
  </style>
</head>
<body>
  <main>
    <h1>SkillYard Local Server</h1>
    <section aria-labelledby="source-trees-title">
      <h2 id="source-trees-title">Source Trees</h2>
      <pre id="source-trees">Loading...</pre>
    </section>
    <section aria-labelledby="library-entries-title">
      <h2 id="library-entries-title">Library Entries</h2>
      <pre id="library-entries">Loading...</pre>
    </section>
    <section aria-labelledby="exposures-title">
      <h2 id="exposures-title">Exposures</h2>
      <pre id="exposures">Loading...</pre>
    </section>
    <section aria-labelledby="doctor-findings-title">
      <h2 id="doctor-findings-title">Doctor Findings</h2>
      <pre id="doctor-findings">Loading...</pre>
    </section>
  </main>
  <script>
    const format = value => JSON.stringify(value, null, 2);
    async function refresh() {
      const state = await fetch('/api/state').then(response => response.json());
      const doctor = await fetch('/api/doctor').then(response => response.json());
      document.getElementById('source-trees').textContent = format(state.sourceTrees);
      document.getElementById('library-entries').textContent = format(state.libraryEntries);
      document.getElementById('exposures').textContent = format(state.exposures);
      document.getElementById('doctor-findings').textContent = format(doctor.findings);
    }
    refresh().catch(error => {
      document.getElementById('doctor-findings').textContent = error.message;
    });
  </script>
</body>
</html>
"""
