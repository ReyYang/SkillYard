from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any

from .hosts import get_host, write_symlink_or_snapshot
from .models import Operation, Plan
from .skills import discover_skill_documents
from .state import State, row_to_dict


class SkillYardApp:
    def __init__(self, home: Path, host_home: Path | None = None):
        self.home = home
        self.host_home = host_home or Path.home()
        self.library_dir = home / "library"
        self.sources_dir = self.library_dir / "sources"
        self.packages_dir = self.library_dir / "packages"
        self.candidates_dir = self.library_dir / "candidates"
        self.state = State(home / "state.sqlite3")

    def close(self) -> None:
        self.state.close()

    def init(self) -> Plan:
        return Plan(
            title="初始化 SkillYard 本机库",
            confirmation="将创建 SkillYard State File 和 Library 目录。",
            operations=[
                Operation("mkdir", str(self.library_dir)),
                Operation("sqlite_init", str(self.state.path)),
            ],
            payload={"kind": "init"},
        )

    def import_git_plan(self, source: str, namespace: str | None = None) -> Plan:
        return Plan(
            title="导入 Git Source Tree",
            confirmation=f"将 clone/fetch {source}，扫描 SKILL.md，并写入 Library state。",
            operations=[
                Operation("git_clone", source, {"destination": str(self.sources_dir / "git" / _slug_from_source(source))}),
                Operation("scan", "SKILL.md"),
                Operation("sqlite_insert", "source_trees/library_entries/events"),
            ],
            payload={"kind": "import_git", "source": source, "namespace": namespace},
        )

    def import_local_plan(self, path: Path, namespace: str) -> Plan:
        return Plan(
            title="导入本地 personal Source Tree",
            confirmation=f"将把 {path} 作为 {namespace} Source Tree 写入 Library state。",
            operations=[
                Operation("scan", str(path)),
                Operation("sqlite_insert", "source_trees/library_entries/events"),
            ],
            payload={"kind": "import_local", "path": str(path), "namespace": namespace},
        )

    def capture_install_plan(self, host: str, host_dir_override: Path | None, command_args: list[str]) -> Plan:
        target = str(host_dir_override) if host_dir_override else f"{host}/user default skill directory"
        return Plan(
            title="Captured Install",
            confirmation="将运行外部 installer，前后快照 Host skill 目录，并记录 Install Receipt。",
            operations=[
                Operation("snapshot_before", target),
                Operation("run", " ".join(command_args)),
                Operation("snapshot_after", target),
                Operation("sqlite_insert", "install_receipts/source_trees/library_entries/events"),
            ],
            payload={
                "kind": "capture_install",
                "host": host,
                "host_dir_override": str(host_dir_override) if host_dir_override else None,
                "command_args": command_args,
            },
        )

    def update_plan(self, source_tree_id: int) -> Plan:
        preview = self.update_preview(source_tree_id)
        if preview.get("blocked"):
            operations: list[Operation] = []
            confirmation = "Dirty Source Tree 阻塞 update；不会修改 working tree。"
        elif preview.get("changed"):
            operations = [
                Operation("git_merge_ff_only", str(source_tree_id), {"from": preview.get("current"), "to": preview.get("upstream")}),
                Operation("sqlite_update", "source_trees/events"),
            ]
            confirmation = f"将 fast-forward 更新 Source Tree {source_tree_id}。"
        else:
            operations = []
            confirmation = "Source Tree 已经是最新；不会修改 working tree。"
        return Plan(
            title="Source Tree Update",
            confirmation=confirmation,
            operations=operations,
            payload={"kind": "update", "source_tree_id": source_tree_id, "preview": preview},
        )

    def apply(self, plan: Plan) -> dict[str, Any]:
        kind = plan.payload.get("kind")
        if kind == "init":
            self.library_dir.mkdir(parents=True, exist_ok=True)
            self.sources_dir.mkdir(parents=True, exist_ok=True)
            self.packages_dir.mkdir(parents=True, exist_ok=True)
            self.candidates_dir.mkdir(parents=True, exist_ok=True)
            self.state.initialize()
            self.state.insert_event("init", "Initialized SkillYard library")
            return {"ok": True}
        if kind == "noop":
            return {"ok": True, "noop": True}
        if kind == "import_git":
            return self.import_git(plan.payload["source"], plan.payload.get("namespace"))
        if kind == "import_local":
            return self.import_local(Path(plan.payload["path"]), plan.payload["namespace"])
        if kind == "expose":
            return self._apply_expose(plan.payload)
        if kind == "capture_install":
            from .captured_install import capture_install

            host_dir = plan.payload.get("host_dir_override")
            return capture_install(
                self,
                plan.payload["host"],
                Path(host_dir) if host_dir else None,
                list(plan.payload["command_args"]),
            )
        if kind == "update":
            return self.update_apply(int(plan.payload["source_tree_id"]), plan.payload.get("preview"))
        raise ValueError(f"unsupported plan kind: {kind}")

    def import_git(self, source: str, namespace: str | None = None) -> dict[str, Any]:
        self.ensure_initialized()
        repo_url = source
        dest = self.sources_dir / "git" / _slug_from_source(source)
        if not dest.exists():
            dest.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["git", "clone", source, str(dest)], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        ns = namespace or _namespace_from_source(source)
        current_ref = _git_output(dest, ["rev-parse", "HEAD"], check=False)
        source_tree_id = self.state.upsert_source_tree_by_root(
            kind="git",
            namespace=ns,
            root_path=str(dest),
            repo_url=repo_url,
            current_ref=current_ref,
        )
        entries = self._discover_entries(source_tree_id, dest, ns, confirmed_provenance=True)
        self.state.insert_event("import", f"Imported Git Source Tree {source}", {"source_tree_id": source_tree_id})
        return {"source_tree_id": source_tree_id, "entries": entries}

    def import_local(self, path: Path, namespace: str) -> dict[str, Any]:
        self.ensure_initialized()
        root = path.resolve()
        if not root.exists():
            raise FileNotFoundError(str(root))
        source_tree_id = self.state.upsert_source_tree_by_root(
            kind="local",
            namespace=namespace,
            root_path=str(root),
            repo_url=None,
            current_ref=_git_output(root, ["rev-parse", "HEAD"], check=False) if (root / ".git").exists() else None,
        )
        entries = self._discover_entries(source_tree_id, root, namespace, confirmed_provenance=True)
        self.state.insert_event("import", f"Imported local Source Tree {root}", {"source_tree_id": source_tree_id})
        return {"source_tree_id": source_tree_id, "entries": entries}

    def expose_plan(
        self,
        identity: str,
        host: str,
        scope: str,
        project_root: Path | None = None,
        host_dir_override: Path | None = None,
        mode: str = "symlink",
        entry_name: str | None = None,
        on_conflict: str = "error",
    ) -> Plan:
        if not self.state.exists():
            raise ValueError("SkillYard is not initialized")
        entry = self.state.get_library_entry(identity)
        if entry is None:
            raise ValueError(f"unknown Library Entry: {identity}")
        adapter = get_host(host)
        source_dir = self._entry_skill_dir(entry)
        host_dir = adapter.skill_dir(scope, self.host_home, project_root, host_dir_override)
        host_entry_name = entry_name or entry["name"]
        target = host_dir / host_entry_name
        final_mode = mode
        if mode == "auto":
            final_mode = "symlink" if adapter.supports_symlink else "snapshot"

        if target.exists() or target.is_symlink():
            if _same_symlink_target(target, source_dir):
                return Plan(
                    title="Exposure 已存在",
                    confirmation="目标 Host Entry 已经指向同一个 Library Entry。",
                    operations=[],
                    payload={"kind": "noop"},
                )
            if on_conflict == "skip":
                return Plan(
                    title="跳过 Host Entry Conflict",
                    confirmation="检测到冲突，将不写入 Host 目录。",
                    operations=[],
                    payload={"kind": "noop"},
                )
            if on_conflict == "recommended":
                host_entry_name = f"{entry['namespace']}-{entry['name']}"
                target = host_dir / host_entry_name
                if target.exists() or target.is_symlink():
                    raise HostEntryConflict(_conflict_message(target, identity))
            elif on_conflict == "replace":
                pass
            else:
                raise HostEntryConflict(_conflict_message(target, identity))

        return Plan(
            title="创建 Exposure",
            confirmation=f"将把 {identity} 暴露到 {host}/{scope}，Host Entry Name 为 {host_entry_name}。",
            operations=[
                Operation("mkdir", str(host_dir)),
                Operation(final_mode, str(target), {"source": str(source_dir), "replace": on_conflict == "replace"}),
                Operation("sqlite_insert", "exposures", {"identity": identity, "host": host, "scope": scope}),
            ],
            payload={
                "kind": "expose",
                "library_entry_id": entry["id"],
                "identity": identity,
                "host": host,
                "scope": scope,
                "project_root": str(project_root.resolve()) if project_root else None,
                "host_entry_name": host_entry_name,
                "mode": final_mode,
                "target_path": str(target),
                "source_path": str(source_dir),
                "replace": on_conflict == "replace",
            },
        )

    def doctor(self, host_dir_override: Path | None = None) -> list[dict[str, Any]]:
        findings: list[dict[str, Any]] = []
        if not self.state.exists():
            if host_dir_override is not None and host_dir_override.exists():
                for child in host_dir_override.iterdir():
                    if child.is_dir():
                        findings.append({"type": "unmanaged_host_entry", "severity": "info", "path": str(child)})
            return findings
        exposures = [row_to_dict(row) for row in self.state.list_exposures()]
        exposure_targets = {exposure["target_path"] for exposure in exposures}
        for exposure in exposures:
            target = Path(exposure["target_path"])
            entry = self.state.conn.execute(
                "select * from library_entries where id = ?", [exposure["library_entry_id"]]
            ).fetchone()
            source_dir = self._entry_skill_dir(entry) if entry is not None else None
            if target.is_symlink():
                if not target.exists():
                    findings.append({"type": "broken_symlink", "severity": "error", "target": str(target)})
                elif source_dir is not None and target.resolve() != source_dir.resolve():
                    findings.append({"type": "host_entry_conflict", "severity": "error", "target": str(target)})
            elif not target.exists():
                findings.append({"type": "missing_host_entry", "severity": "error", "target": str(target)})
            elif exposure["mode"] == "symlink":
                findings.append({"type": "host_entry_conflict", "severity": "error", "target": str(target)})
            if exposure["mode"] == "snapshot":
                if source_dir is not None and target.exists():
                    if _tree_hash(source_dir) != _tree_hash(target):
                        findings.append({"type": "snapshot_drift", "severity": "warning", "target": str(target)})

        for source in self.state.list_source_trees():
            root = Path(source["root_path"])
            if source["kind"] == "git" and (root / ".git").exists():
                if _git_output(root, ["status", "--porcelain"], check=False):
                    findings.append({"type": "dirty_source_tree", "severity": "warning", "source_tree_id": source["id"]})
                branch = _git_output(root, ["rev-parse", "--abbrev-ref", "HEAD"], check=False)
                upstream = _git_output(root, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], check=False)
                if branch and not upstream:
                    findings.append({"type": "branch_drift", "severity": "info", "source_tree_id": source["id"], "branch": branch})

        host_dirs: set[Path] = {Path(e["target_path"]).parent for e in exposures}
        if host_dir_override is not None:
            host_dirs.add(host_dir_override)
        for host_dir in host_dirs:
            if not host_dir.exists():
                continue
            for child in host_dir.iterdir():
                if str(child) not in exposure_targets:
                    findings.append({"type": "unmanaged_host_entry", "severity": "info", "path": str(child)})
        return findings

    def update_preview(self, source_tree_id: int) -> dict[str, Any]:
        if not self.state.exists():
            raise ValueError("SkillYard is not initialized")
        source = self.state.get_source_tree(source_tree_id)
        if source is None:
            raise ValueError(f"unknown Source Tree: {source_tree_id}")
        if source["kind"] != "git":
            return {"source_tree_id": source_tree_id, "kind": source["kind"], "updatable": False, "reason": "not a Git Source Tree"}
        root = Path(source["root_path"])
        dirty = bool(_git_output(root, ["status", "--porcelain"], check=False))
        if dirty:
            return {"source_tree_id": source_tree_id, "blocked": True, "reason": "Dirty Source Tree"}
        subprocess.run(["git", "-C", str(root), "fetch"], check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        head = _git_output(root, ["rev-parse", "HEAD"], check=False)
        upstream = _git_output(root, ["rev-parse", "@{u}"], check=False)
        entries = [
            row_to_dict(row)
            for row in self.state.conn.execute("select * from library_entries where source_tree_id = ?", [source_tree_id])
        ]
        entry_ids = [entry["id"] for entry in entries]
        exposures = []
        if entry_ids:
            placeholders = ",".join("?" for _ in entry_ids)
            exposures = [
                row_to_dict(row)
                for row in self.state.conn.execute(
                    f"select * from exposures where library_entry_id in ({placeholders})", entry_ids
                )
            ]
        return {
            "source_tree_id": source_tree_id,
            "blocked": False,
            "current": head,
            "upstream": upstream,
            "changed": bool(upstream and upstream != head),
            "library_entries": entries,
            "exposures": exposures,
        }

    def update_apply(self, source_tree_id: int, preview: dict[str, Any] | None = None) -> dict[str, Any]:
        preview = preview or self.update_preview(source_tree_id)
        if preview.get("blocked") or not preview.get("changed"):
            return preview
        source = self.state.get_source_tree(source_tree_id)
        root = Path(source["root_path"])
        target = preview.get("upstream")
        if not target:
            raise ValueError("update preview does not include target revision")
        subprocess.run(
            ["git", "-C", str(root), "merge", "--ff-only", str(target)],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        new_ref = _git_output(root, ["rev-parse", "HEAD"], check=False)
        self.state.conn.execute("update source_trees set current_ref = ? where id = ?", [new_ref, source_tree_id])
        self.state.conn.commit()
        self.state.insert_event("update", f"Updated Source Tree {source_tree_id}", {"source_tree_id": source_tree_id})
        return {"ok": True, "source_tree_id": source_tree_id, "current": new_ref, "impact": preview}

    def upgrade_package_source_tree_to_git(self, source_tree_id: int, repo_url: str, namespace: str | None = None) -> dict[str, Any]:
        self.ensure_initialized()
        source = self.state.get_source_tree(source_tree_id)
        if source is None:
            raise ValueError(f"unknown Source Tree: {source_tree_id}")
        if source["kind"] != "package":
            raise ValueError("only Package Source Trees can be upgraded")
        dest = self.sources_dir / "git" / _slug_from_source(repo_url)
        if not dest.exists():
            dest.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["git", "clone", repo_url, str(dest)], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        ns = namespace or source["namespace"]
        current_ref = _git_output(dest, ["rev-parse", "HEAD"], check=False)
        existing_paths = {
            row["skill_path"]
            for row in self.state.conn.execute("select skill_path from library_entries where source_tree_id = ?", [source_tree_id])
        }
        missing_paths = [path for path in sorted(existing_paths) if not (dest / path).exists()]
        if missing_paths:
            raise ValueError(f"repository evidence does not match Package Source Tree; missing {missing_paths}")
        self.state.conn.execute(
            """
            update source_trees
               set kind = 'git',
                   namespace = ?,
                   root_path = ?,
                   repo_url = ?,
                   current_ref = ?,
                   repository_url = ?
             where id = ?
            """,
            [ns, str(dest), repo_url, current_ref, repo_url, source_tree_id],
        )
        self.state.conn.commit()
        discovered = self._discover_entries(source_tree_id, dest, ns, confirmed_provenance=True)
        self.state.conn.execute(
            "update library_entries set confirmed_provenance = 1 where source_tree_id = ?",
            [source_tree_id],
        )
        self.state.conn.commit()
        self.state.insert_event("upgrade_source_tree", f"Upgraded Package Source Tree {source_tree_id} to Git", {"repo_url": repo_url})
        return {
            "ok": True,
            "source_tree_id": source_tree_id,
            "repo_url": repo_url,
            "current_ref": current_ref,
            "existing_skill_paths": sorted(existing_paths),
            "discovered": discovered,
        }

    def state_snapshot(self) -> dict[str, Any]:
        return {
            "sourceTrees": [row_to_dict(row) for row in self.state.list_source_trees()],
            "libraryEntries": [row_to_dict(row) for row in self.state.list_library_entries()],
            "exposures": [row_to_dict(row) for row in self.state.list_exposures()],
            "events": [row_to_dict(row) for row in self.state.list_events()],
        }

    def ensure_initialized(self) -> None:
        self.state.initialize()
        self.library_dir.mkdir(parents=True, exist_ok=True)
        self.sources_dir.mkdir(parents=True, exist_ok=True)
        self.packages_dir.mkdir(parents=True, exist_ok=True)
        self.candidates_dir.mkdir(parents=True, exist_ok=True)

    def _discover_entries(self, source_tree_id: int, root: Path, namespace: str, confirmed_provenance: bool) -> list[dict[str, Any]]:
        entries: list[dict[str, Any]] = []
        for doc in discover_skill_documents(root):
            rel = doc.path.relative_to(root)
            existing = self.state.conn.execute(
                "select * from library_entries where source_tree_id = ? and skill_path = ?",
                [source_tree_id, str(rel)],
            ).fetchone()
            if existing is not None:
                identity = f"{namespace}/{doc.name}"
                if identity != existing["identity"] and self.state.get_library_entry(identity) is not None:
                    identity = _unique_identity(self.state, namespace, doc.name, rel)
                self.state.conn.execute(
                    """
                    update library_entries
                       set identity = ?, namespace = ?, name = ?, display_label = ?,
                           description = ?, confirmed_provenance = ?
                     where id = ?
                    """,
                    [
                        identity,
                        namespace,
                        doc.name,
                        f"{_title(namespace)}: {_title(doc.name)}",
                        doc.description,
                        int(confirmed_provenance),
                        existing["id"],
                    ],
                )
                self.state.conn.commit()
                entries.append({"id": existing["id"], "identity": identity, "name": doc.name, "skill_path": str(rel)})
                continue
            identity = _unique_identity(self.state, namespace, doc.name, rel)
            display_label = f"{_title(namespace)}: {_title(doc.name)}"
            entry_id = self.state.insert_library_entry(
                source_tree_id=source_tree_id,
                identity=identity,
                namespace=namespace,
                name=doc.name,
                display_label=display_label,
                skill_path=str(rel),
                description=doc.description,
                confirmed_provenance=confirmed_provenance,
            )
            entries.append({"id": entry_id, "identity": identity, "name": doc.name, "skill_path": str(rel)})
        return entries

    def _entry_skill_dir(self, entry: Any) -> Path:
        source = self.state.get_source_tree(int(entry["source_tree_id"]))
        if source is None:
            raise ValueError("Library Entry points to missing Source Tree")
        return Path(source["root_path"]) / Path(entry["skill_path"]).parent

    def _apply_expose(self, payload: dict[str, Any]) -> dict[str, Any]:
        mode = write_symlink_or_snapshot(
            Path(payload["source_path"]),
            Path(payload["target_path"]),
            payload["mode"],
            replace=bool(payload.get("replace")),
        )
        if payload.get("replace"):
            self.state.conn.execute(
                """
                delete from exposures
                 where host = ?
                   and scope = ?
                   and ifnull(project_root, '') = ifnull(?, '')
                   and host_entry_name = ?
                """,
                [
                    payload["host"],
                    payload["scope"],
                    payload.get("project_root"),
                    payload["host_entry_name"],
                ],
            )
            self.state.conn.commit()
        exposure_id = self.state.insert_exposure(
            library_entry_id=payload["library_entry_id"],
            host=payload["host"],
            scope=payload["scope"],
            project_root=payload.get("project_root"),
            host_entry_name=payload["host_entry_name"],
            mode=mode,
            target_path=payload["target_path"],
        )
        self.state.insert_event("expose", f"Created Exposure for {payload['identity']}", {"exposure_id": exposure_id})
        return {"ok": True, "exposure_id": exposure_id, "mode": mode}


class HostEntryConflict(RuntimeError):
    pass


def _slug_from_source(source: str) -> str:
    text = source.removesuffix(".git").rstrip("/")
    parts = re.split(r"[:/]+", text)
    if len(parts) >= 2:
        return _safe(parts[-2]) + "__" + _safe(parts[-1])
    return _safe(text)


def _namespace_from_source(source: str) -> str:
    text = source.removesuffix(".git").rstrip("/")
    parts = re.split(r"[:/]+", text)
    if "github.com" in parts and len(parts) >= 2:
        return _safe(parts[-2])
    return _safe(Path(text).name)


def _safe(value: str) -> str:
    value = value.strip().replace(" ", "-")
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-") or "source"


def _title(value: str) -> str:
    return value.replace("-", " ").replace("_", " ").title()


def _unique_identity(state: State, namespace: str, name: str, rel: Path) -> str:
    base = f"{namespace}/{name}"
    if state.get_library_entry(base) is None:
        return base
    suffix = _safe(str(rel.parent))
    candidate = f"{base}-{suffix}"
    counter = 2
    while state.get_library_entry(candidate) is not None:
        candidate = f"{base}-{suffix}-{counter}"
        counter += 1
    return candidate


def _git_output(root: Path, args: list[str], check: bool = True) -> str:
    result = subprocess.run(["git", "-C", str(root), *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if check and result.returncode != 0:
        raise RuntimeError(result.stderr.strip())
    if result.returncode != 0:
        return ""
    return result.stdout.strip()


def _same_symlink_target(path: Path, target: Path) -> bool:
    if not path.is_symlink():
        return False
    try:
        return path.resolve() == target.resolve()
    except FileNotFoundError:
        return False


def _conflict_message(target: Path, identity: str) -> str:
    return (
        f"Host Entry Conflict：{target} 已存在，不能直接暴露 {identity}。\n"
        "请选择：跳过、使用推荐名、自定义 Host Entry Name，或替换已有 entry。"
    )


def _tree_hash(root: Path) -> str:
    h = hashlib.sha256()
    if not root.exists():
        return ""
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        if ".git" in path.parts:
            continue
        h.update(str(path.relative_to(root)).encode())
        h.update(path.read_bytes())
    return h.hexdigest()


def write_json(data: Any) -> str:
    return json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True)
