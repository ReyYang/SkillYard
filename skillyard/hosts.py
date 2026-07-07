from __future__ import annotations

import os
import shutil
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class HostAdapter:
    name: str
    user_relative: Path
    project_relative: Path
    supports_symlink: bool = True

    def skill_dir(self, scope: str, home: Path, project_root: Path | None = None, override: Path | None = None) -> Path:
        if override is not None:
            return override
        if scope == "user":
            return home / self.user_relative
        if scope == "project":
            if project_root is None:
                raise ValueError("project scope requires project_root")
            return project_root / self.project_relative
        raise ValueError(f"unsupported scope: {scope}")


HOSTS: dict[str, HostAdapter] = {
    "codex": HostAdapter("codex", Path(".codex/skills"), Path(".codex/skills")),
    "claude": HostAdapter("claude", Path(".claude/skills"), Path(".claude/skills")),
    "cursor": HostAdapter("cursor", Path(".cursor/skills"), Path(".cursor/skills")),
    "copilot": HostAdapter("copilot", Path(".github/copilot/skills"), Path(".github/copilot/skills")),
}


def get_host(name: str) -> HostAdapter:
    try:
        return HOSTS[name]
    except KeyError as exc:
        raise ValueError(f"unsupported host: {name}") from exc


def write_symlink_or_snapshot(source: Path, target: Path, mode: str, replace: bool = False) -> str:
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() or target.is_symlink():
        if not replace:
            raise FileExistsError(str(target))
        if target.is_dir() and not target.is_symlink():
            shutil.rmtree(target)
        else:
            target.unlink()
    if mode == "snapshot":
        shutil.copytree(source, target)
        return "snapshot"
    try:
        os.symlink(source, target, target_is_directory=True)
        return "symlink"
    except OSError:
        shutil.copytree(source, target)
        return "snapshot"
