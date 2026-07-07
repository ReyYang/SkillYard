from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import uuid
from pathlib import Path
from typing import Any

from .hosts import get_host


def capture_install(app, host: str, host_dir_override: Path | None, command_args: list[str]) -> dict[str, Any]:
    if not command_args:
        raise ValueError("capture_install requires command_args")

    app.ensure_initialized()
    host_dir = get_host(host).skill_dir("user", app.host_home, override=host_dir_override)

    before = _snapshot_host_entries(host_dir)
    completed = subprocess.run(command_args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    after = _snapshot_host_entries(host_dir)
    changed_entries = _changed_entries(before, after)

    package = _infer_npm_package(command_args)
    npm_metadata = _resolve_npm_metadata(package) if package is not None else {}
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or f"captured install failed with exit code {completed.returncode}")

    source_tree_id: int | None = None
    source_tree_kind: str | None = None
    entries: list[dict[str, Any]] = []

    if changed_entries:
        if package is not None:
            source_tree_kind = "package"
            namespace = _package_namespace(package["name"])
            version = npm_metadata.get("version") or package["version"]
            source_root = app.packages_dir / _slug(package["name"]) / _slug(version or "unknown")
            _copy_host_entries(changed_entries, source_root)
            source_tree_id = app.state.upsert_source_tree_by_root(
                kind="package",
                namespace=namespace,
                root_path=str(source_root),
                repo_url=None,
                current_ref=version,
                package_name=package["name"],
                package_version=version,
                tarball_url=npm_metadata.get("tarball_url"),
                integrity=npm_metadata.get("integrity"),
                repository_url=npm_metadata.get("repository_url"),
            )
            entries = app._discover_entries(source_tree_id, source_root, namespace, confirmed_provenance=True)
        else:
            source_tree_kind = "candidate"
            namespace = f"captured-{uuid.uuid4().hex[:12]}"
            source_root = app.candidates_dir / namespace
            _copy_host_entries(changed_entries, source_root)
            source_tree_id = app.state.insert_source_tree(
                kind="candidate",
                namespace=namespace,
                root_path=str(source_root),
                repo_url=None,
                current_ref=None,
            )
            entries = app._discover_entries(source_tree_id, source_root, namespace, confirmed_provenance=False)

    receipt_id = app.state.insert_install_receipt(
        command=command_args,
        provider="npm" if package is not None else None,
        package_name=package["name"] if package is not None else None,
        package_version=package["version"] if package is not None else None,
        before_snapshot=_public_snapshot(before),
        after_snapshot=_public_snapshot(after),
        source_metadata={
            "host": host,
            "host_dir": str(host_dir),
            "returncode": completed.returncode,
            "source_tree_id": source_tree_id,
            "source_tree_kind": source_tree_kind,
            "npm": npm_metadata,
        },
        changed_entries=[_public_changed_entry(entry) for entry in changed_entries],
    )

    if source_tree_kind == "candidate" and source_tree_id is not None:
        source_root = Path(app.state.get_source_tree(source_tree_id)["root_path"])
        for entry in entries:
            app.state.insert_provenance_inference(
                library_entry_id=entry["id"],
                source_candidate_path=str(source_root),
                evidence={
                    "install_receipt_id": receipt_id,
                    "command": command_args,
                    "host": host,
                    "host_dir": str(host_dir),
                },
                confidence=0,
                accepted=0,
            )

    app.state.insert_event(
        "capture_install",
        f"Captured install from {Path(command_args[0]).name}",
        {"install_receipt_id": receipt_id, "source_tree_id": source_tree_id},
    )

    return {
        "ok": True,
        "command": command_args,
        "returncode": completed.returncode,
        "host": host,
        "host_dir": str(host_dir),
        "install_receipt_id": receipt_id,
        "package": package,
        "npm_metadata": npm_metadata,
        "source_tree_id": source_tree_id,
        "source_tree_kind": source_tree_kind,
        "changed_entries": [_public_changed_entry(entry) for entry in changed_entries],
        "entries": entries,
    }


def _snapshot_host_entries(host_dir: Path) -> dict[str, dict[str, Any]]:
    if not host_dir.exists():
        return {}
    if not host_dir.is_dir():
        raise NotADirectoryError(str(host_dir))

    entries: dict[str, dict[str, Any]] = {}
    for child in sorted(host_dir.iterdir(), key=lambda path: path.name):
        if not child.is_dir():
            continue
        skill_paths = [
            str(path.relative_to(child))
            for path in sorted(child.rglob("SKILL.md"))
            if ".git" not in path.relative_to(child).parts
        ]
        if not skill_paths:
            continue
        entries[child.name] = {
            "name": child.name,
            "path": child,
            "skill_paths": skill_paths,
            "tree_hash": _tree_hash(child),
        }
    return entries


def _changed_entries(before: dict[str, dict[str, Any]], after: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    changed: list[dict[str, Any]] = []
    for name, after_entry in after.items():
        before_entry = before.get(name)
        if before_entry is None:
            status = "added"
        elif before_entry["tree_hash"] != after_entry["tree_hash"]:
            status = "changed"
        else:
            continue
        changed.append({**after_entry, "status": status})
    for name, before_entry in before.items():
        if name not in after:
            changed.append({**before_entry, "status": "deleted"})
    return changed


def _copy_host_entries(changed_entries: list[dict[str, Any]], source_root: Path) -> None:
    source_root.mkdir(parents=True, exist_ok=True)
    for entry in changed_entries:
        if entry["status"] == "deleted":
            continue
        target = source_root / entry["name"]
        if target.exists() or target.is_symlink():
            if target.is_symlink() or target.is_file():
                target.unlink()
            else:
                shutil.rmtree(target)
        shutil.copytree(entry["path"], target)


def _infer_npm_package(command_args: list[str]) -> dict[str, str | None] | None:
    command = Path(command_args[0]).name
    spec: str | None = None
    if command == "npx":
        spec = _first_npm_spec(command_args[1:], stop_at_double_dash=False)
    elif command == "npm" and len(command_args) >= 2 and command_args[1] in {"exec", "x"}:
        spec = _first_npm_spec(command_args[2:], stop_at_double_dash=True)
    if spec is None:
        return None
    parsed = _parse_package_spec(spec)
    if parsed is None:
        return None
    name, version = parsed
    return {"name": name, "version": version}


def _resolve_npm_metadata(package: dict[str, str | None]) -> dict[str, str]:
    inline = os.environ.get("SKILLYARD_NPM_VIEW_JSON")
    if inline:
        return _metadata_from_npm_view_output(inline)

    npm = shutil.which("npm")
    if npm is None:
        return {}
    spec = package["name"] if package["version"] is None else f"{package['name']}@{package['version']}"
    try:
        result = subprocess.run(
            [npm, "view", spec, "--json"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=8,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return {}
    if result.returncode != 0 or not result.stdout.strip():
        return {}
    return _metadata_from_npm_view_output(result.stdout)


def _metadata_from_npm_view_output(output: str) -> dict[str, str]:
    try:
        data = json.loads(output)
    except json.JSONDecodeError:
        return {}
    if isinstance(data, list):
        data = data[-1] if data else {}
    if not isinstance(data, dict):
        return {}
    dist = data.get("dist") if isinstance(data.get("dist"), dict) else {}
    repository = data.get("repository")
    repository_url = None
    if isinstance(repository, dict):
        repository_url = repository.get("url")
    elif isinstance(repository, str):
        repository_url = repository
    return {
        key: value
        for key, value in {
            "version": data.get("version"),
            "tarball_url": dist.get("tarball"),
            "integrity": dist.get("integrity") or dist.get("shasum"),
            "repository_url": _canonical_repository_url(repository_url) if repository_url else None,
        }.items()
        if isinstance(value, str) and value
    }


def _first_npm_spec(args: list[str], stop_at_double_dash: bool) -> str | None:
    value_options = {"--package", "-p"}
    value_option_prefixes = ("--package=",)
    options_with_values = {
        "--cache",
        "--call",
        "--prefix",
        "--registry",
        "--script-shell",
        "--shell",
        "--userconfig",
        "--workspace",
        "-c",
        "-w",
    }

    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--":
            if stop_at_double_dash:
                return None
            index += 1
            continue
        for prefix in value_option_prefixes:
            if arg.startswith(prefix):
                return arg[len(prefix) :]
        if arg in value_options:
            return args[index + 1] if index + 1 < len(args) else None
        if arg in options_with_values:
            index += 2
            continue
        if arg.startswith("-"):
            index += 1
            continue
        return arg
    return None


def _parse_package_spec(spec: str) -> tuple[str, str | None] | None:
    spec = spec.strip()
    if not spec or spec.startswith((".", "/", "~")):
        return None
    if "://" in spec or spec.startswith(("file:", "git:", "git+", "github:")):
        return None

    if spec.startswith("@"):
        slash_index = spec.find("/")
        if slash_index == -1:
            return None
        version_index = spec.rfind("@")
        if version_index > slash_index:
            name = spec[:version_index]
            version = spec[version_index + 1 :] or None
        else:
            name = spec
            version = None
    else:
        if "/" in spec:
            return None
        version_index = spec.rfind("@")
        if version_index > 0:
            name = spec[:version_index]
            version = spec[version_index + 1 :] or None
        else:
            name = spec
            version = None

    if not name or any(char.isspace() for char in name):
        return None
    return name, version


def _package_namespace(package_name: str) -> str:
    return _slug(package_name.removeprefix("@").replace("/", "-"))


def _canonical_repository_url(url: str) -> str:
    url = url.removeprefix("git+")
    if url.startswith("git://github.com/"):
        url = "https://github.com/" + url.removeprefix("git://github.com/")
    ssh = re.fullmatch(r"git@github\.com:([^/]+)/(.+?)(?:\.git)?", url)
    if ssh:
        return f"https://github.com/{ssh.group(1)}/{ssh.group(2)}"
    if url.startswith("https://github.com/") and url.endswith(".git"):
        return url[:-4]
    return url


def _slug(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value.strip()).strip("-") or "unknown"


def _public_changed_entry(entry: dict[str, Any]) -> dict[str, Any]:
    return {
        "name": entry["name"],
        "status": entry["status"],
        "host_path": str(entry["path"]),
        "skill_paths": entry["skill_paths"],
        "tree_hash": entry["tree_hash"],
    }


def _public_snapshot(snapshot: dict[str, dict[str, Any]]) -> dict[str, dict[str, Any]]:
    return {
        name: {
            "name": entry["name"],
            "host_path": str(entry["path"]),
            "skill_paths": entry["skill_paths"],
            "tree_hash": entry["tree_hash"],
        }
        for name, entry in snapshot.items()
    }


def _tree_hash(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted((path for path in root.rglob("*") if path.is_file()), key=lambda item: str(item.relative_to(root))):
        if ".git" in path.relative_to(root).parts:
            continue
        digest.update(str(path.relative_to(root)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()
