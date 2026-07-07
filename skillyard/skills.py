from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class SkillDocument:
    path: Path
    name: str
    description: str


def parse_skill_document(path: Path) -> SkillDocument:
    text = path.read_text(encoding="utf-8")
    frontmatter = _frontmatter(text)
    name = frontmatter.get("name") or path.parent.name
    description = frontmatter.get("description", "")
    return SkillDocument(path=path, name=name.strip(), description=description.strip())


def discover_skill_documents(root: Path) -> list[SkillDocument]:
    docs: list[SkillDocument] = []
    for path in sorted(root.rglob("SKILL.md")):
        if ".git" in path.parts:
            continue
        docs.append(parse_skill_document(path))
    return docs


def _frontmatter(text: str) -> dict[str, str]:
    if not text.startswith("---"):
        return {}
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    data: dict[str, str] = {}
    for line in lines[1:]:
        if line.strip() == "---":
            break
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        data[key.strip()] = value.strip().strip('"').strip("'")
    return data
