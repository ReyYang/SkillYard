from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path
from typing import Any

from .skills import _frontmatter


def explain_skill(path: str | Path) -> dict[str, Any]:
    skill_path = _resolve_skill_path(path)
    text = skill_path.read_text(encoding="utf-8")
    frontmatter = _frontmatter(text)
    heading = _first_heading(text)
    name = frontmatter.get("name") or heading or skill_path.parent.name
    description = frontmatter.get("description", "")
    evidence: list[dict[str, str]] = []

    if frontmatter.get("name"):
        evidence.append({"source": "frontmatter", "key": "name", "value": frontmatter["name"]})
    if frontmatter.get("description"):
        evidence.append({"source": "frontmatter", "key": "description", "value": frontmatter["description"]})
    if heading:
        evidence.append({"source": "content", "key": "heading", "value": heading})
    evidence.append({"source": "path", "key": "directory", "value": skill_path.parent.name})

    confidence = 0.2
    if frontmatter.get("name"):
        confidence += 0.3
    if frontmatter.get("description"):
        confidence += 0.25
    if heading:
        confidence += 0.1
    if skill_path.name == "SKILL.md":
        confidence += 0.05

    return {
        "skill_path": str(skill_path),
        "name": name,
        "description": description,
        "frontmatter": frontmatter,
        "evidence": evidence,
        "confidence": round(min(confidence, 1.0), 2),
    }


def infer_provenance(path: str | Path) -> dict[str, Any]:
    skill_path = _resolve_skill_path(path)
    explanation = explain_skill(skill_path)
    root = _metadata_root(skill_path)
    evidence: list[dict[str, str]] = list(explanation["evidence"])
    candidates: list[dict[str, Any]] = []

    git_url = _git_origin(root)
    if git_url:
        evidence.append({"source": "git", "key": "origin", "value": git_url})
        candidates.append(
            {
                "source_type": "git",
                "source": git_url,
                "confidence": 0.9,
                "evidence": ["git.origin"],
            }
        )

    package_source = _package_source(root)
    if package_source:
        evidence.append({"source": "package", "key": "name", "value": package_source})
        candidates.append(
            {
                "source_type": "package",
                "source": package_source,
                "confidence": 0.75,
                "evidence": ["package.name"],
            }
        )

    readme_title = _readme_title(root)
    if readme_title:
        evidence.append({"source": "readme", "key": "title", "value": readme_title})

    local_name = _local_candidate_name(skill_path, root)
    candidates.append(
        {
            "source_type": "local-path",
            "source": local_name,
            "confidence": 0.25,
            "evidence": ["path.directory"],
        }
    )
    candidates.sort(key=lambda candidate: candidate["confidence"], reverse=True)
    confidence = max(candidate["confidence"] for candidate in candidates)

    return {
        "skill_path": str(skill_path),
        "root_path": str(root),
        "evidence": evidence,
        "confidence": round(confidence, 2),
        "candidates": candidates,
    }


def _resolve_skill_path(path: str | Path) -> Path:
    resolved = Path(path)
    if resolved.is_dir():
        resolved = resolved / "SKILL.md"
    if not resolved.exists():
        raise FileNotFoundError(f"Skill file not found: {resolved}")
    if resolved.name != "SKILL.md":
        raise ValueError(f"Expected SKILL.md or a skill directory: {resolved}")
    return resolved


def _metadata_root(skill_path: Path) -> Path:
    for parent in [skill_path.parent, *skill_path.parents]:
        if _has_metadata(parent):
            return parent
    return skill_path.parent


def _has_metadata(path: Path) -> bool:
    return any((path / name).exists() for name in [".git", "package.json", "pyproject.toml", "README.md", "README"])


def _git_origin(root: Path) -> str | None:
    config = root / ".git" / "config"
    if not config.exists():
        return None
    remote_origin = False
    for line in config.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            remote_origin = stripped == '[remote "origin"]'
            continue
        if remote_origin and stripped.startswith("url") and "=" in stripped:
            _, value = stripped.split("=", 1)
            return _canonical_git_url(value.strip())
    return None


def _canonical_git_url(url: str) -> str:
    ssh_match = re.fullmatch(r"git@github\.com:([^/]+)/(.+?)(?:\.git)?", url)
    if ssh_match:
        return f"https://github.com/{ssh_match.group(1)}/{ssh_match.group(2)}"
    if url.startswith("https://github.com/") and url.endswith(".git"):
        return url[:-4]
    return url


def _package_source(root: Path) -> str | None:
    package_json = root / "package.json"
    if package_json.exists():
        data = json.loads(package_json.read_text(encoding="utf-8"))
        name = data.get("name")
        if not name:
            return None
        version = data.get("version")
        return f"npm:{name}@{version}" if version else f"npm:{name}"

    pyproject = root / "pyproject.toml"
    if pyproject.exists():
        data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
        project = data.get("project", {})
        name = project.get("name")
        if not name:
            return None
        version = project.get("version")
        return f"python:{name}@{version}" if version else f"python:{name}"
    return None


def _readme_title(root: Path) -> str | None:
    for name in ["README.md", "README"]:
        readme = root / name
        if not readme.exists():
            continue
        title = _first_heading(readme.read_text(encoding="utf-8", errors="replace"))
        return title or readme.name
    return None


def _first_heading(text: str) -> str | None:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("# "):
            return stripped[2:].strip()
    return None


def _local_candidate_name(skill_path: Path, root: Path) -> str:
    try:
        relative = skill_path.parent.relative_to(root)
    except ValueError:
        return skill_path.parent.name
    return str(relative) if str(relative) != "." else skill_path.parent.name
