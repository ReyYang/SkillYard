from __future__ import annotations

import json
import re
import shutil
import subprocess
from typing import Any


def discover_with_gh(query: str) -> list[dict[str, Any]]:
    gh = shutil.which("gh")
    if gh is None:
        raise RuntimeError("gh command not found; install GitHub CLI and the gh skill extension")

    try:
        result = subprocess.run(
            [gh, "skill", "search", query, "--json"],
            check=False,
            capture_output=True,
            text=True,
        )
    except FileNotFoundError as exc:
        raise RuntimeError("gh command not found; install GitHub CLI and the gh skill extension") from exc

    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        message = f"gh skill search failed with exit code {result.returncode}"
        if detail:
            message = f"{message}: {detail}"
        raise RuntimeError(message)

    output = result.stdout.strip()
    if not output:
        return []
    parsed_json = _parse_json_output(output)
    if parsed_json is not None:
        return parsed_json
    return _parse_text_output(output)


def discovery_result_to_import_source(result: dict[str, Any]) -> str:
    for key in ["url", "repository", "repo", "source", "html_url"]:
        value = result.get(key)
        if isinstance(value, str) and value:
            return value
    raise ValueError("discovery result does not include an importable source URL")


def _parse_json_output(output: str) -> list[dict[str, Any]] | None:
    try:
        data = json.loads(output)
    except json.JSONDecodeError:
        return None

    if isinstance(data, dict):
        for key in ["items", "results", "skills"]:
            value = data.get(key)
            if isinstance(value, list):
                data = value
                break
        else:
            data = [data]

    if not isinstance(data, list):
        return []

    rows: list[dict[str, Any]] = []
    for item in data:
        if isinstance(item, dict):
            rows.append(item)
        elif isinstance(item, str):
            rows.extend(_parse_text_output(item))
    return rows


def _parse_text_output(output: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for line in output.splitlines():
        parsed = _parse_text_line(line.strip())
        if parsed:
            rows.append(parsed)
    return rows


def _parse_text_line(line: str) -> dict[str, str] | None:
    if not line:
        return None

    tab_parts = [part.strip() for part in line.split("\t") if part.strip()]
    if len(tab_parts) >= 3:
        return {"name": tab_parts[0], "description": tab_parts[1], "url": tab_parts[2]}

    dash_parts = [part.strip() for part in line.split(" - ") if part.strip()]
    if len(dash_parts) >= 3:
        return {"name": dash_parts[0], "description": " - ".join(dash_parts[1:-1]), "url": dash_parts[-1]}

    url_match = re.search(r"https?://\S+", line)
    if url_match:
        url = url_match.group(0)
        before_url = line[: url_match.start()].strip(" -\t")
        name, description = _split_name_description(before_url)
        return {"name": name, "description": description, "url": url}

    name, description = _split_name_description(line)
    return {"name": name, "description": description, "url": ""}


def _split_name_description(text: str) -> tuple[str, str]:
    for separator in [" - ", ": "]:
        if separator in text:
            name, description = text.split(separator, 1)
            return name.strip(), description.strip()
    return text.strip(), ""
