from __future__ import annotations

import json
import os
import stat
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import mock

from skillyard.ai import explain_skill, infer_provenance
from skillyard.gh_discovery import discover_with_gh, discovery_result_to_import_source


class AIAssistTests(unittest.TestCase):
    def test_explain_skill_returns_local_evidence_without_writing_state(self) -> None:
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            skill_dir = root / "skills" / "reviewer"
            skill_dir.mkdir(parents=True)
            skill_path = skill_dir / "SKILL.md"
            skill_path.write_text(
                textwrap.dedent(
                    """\
                    ---
                    name: reviewer
                    description: Reviews pull requests for regressions.
                    ---

                    # Reviewer

                    Use when checking diffs before merge.
                    """
                ),
                encoding="utf-8",
            )

            result = explain_skill(skill_dir)

            self.assertEqual(result["skill_path"], str(skill_path))
            self.assertEqual(result["frontmatter"]["name"], "reviewer")
            self.assertEqual(result["frontmatter"]["description"], "Reviews pull requests for regressions.")
            self.assertGreaterEqual(result["confidence"], 0.6)
            self.assertIn({"source": "frontmatter", "key": "name", "value": "reviewer"}, result["evidence"])
            self.assertFalse((root / "state.sqlite").exists())

    def test_infer_provenance_uses_package_readme_path_and_git_metadata(self) -> None:
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            skill_dir = root / "skills" / "planner"
            git_dir = root / ".git"
            skill_dir.mkdir(parents=True)
            git_dir.mkdir()
            (skill_dir / "SKILL.md").write_text(
                textwrap.dedent(
                    """\
                    ---
                    name: planner
                    description: Plans implementation work.
                    ---
                    """
                ),
                encoding="utf-8",
            )
            (root / "package.json").write_text(
                json.dumps({"name": "@example/agent-skills", "version": "1.2.3"}),
                encoding="utf-8",
            )
            (root / "README.md").write_text(
                "# Example Agent Skills\n\nSkills maintained for Codex agents.\n",
                encoding="utf-8",
            )
            (git_dir / "config").write_text(
                textwrap.dedent(
                    """\
                    [remote "origin"]
                        url = git@github.com:example/agent-skills.git
                    """
                ),
                encoding="utf-8",
            )

            result = infer_provenance(skill_dir / "SKILL.md")

            self.assertGreaterEqual(result["confidence"], 0.8)
            self.assertEqual(result["candidates"][0]["source_type"], "git")
            self.assertEqual(result["candidates"][0]["source"], "https://github.com/example/agent-skills")
            self.assertTrue(
                any(candidate["source_type"] == "package" and candidate["source"] == "npm:@example/agent-skills@1.2.3"
                    for candidate in result["candidates"])
            )
            self.assertTrue(any(item["source"] == "readme" for item in result["evidence"]))

    def test_infer_provenance_low_confidence_when_only_skill_file_exists(self) -> None:
        with TemporaryDirectory() as tmp:
            skill_dir = Path(tmp) / "solo"
            skill_dir.mkdir()
            (skill_dir / "SKILL.md").write_text("# Solo\n", encoding="utf-8")

            result = infer_provenance(skill_dir)

            self.assertLess(result["confidence"], 0.5)
            self.assertEqual(result["candidates"][0]["source_type"], "local-path")
            self.assertEqual(result["candidates"][0]["source"], "solo")


class GhDiscoveryTests(unittest.TestCase):
    def test_discover_with_gh_parses_json_output(self) -> None:
        with TemporaryDirectory() as tmp:
            bin_dir = Path(tmp)
            fake_gh = bin_dir / "gh"
            calls = bin_dir / "calls.txt"
            fake_gh.write_text(
                textwrap.dedent(
                    f"""\
                    #!/bin/sh
                    printf '%s\\n' "$@" > {str(calls)!r}
                    printf '%s\\n' '[{{"name":"reviewer","description":"Review code","url":"https://github.com/acme/reviewer"}},{{"name":"planner","description":"Plan work","url":"https://github.com/acme/planner"}}]'
                    """
                ),
                encoding="utf-8",
            )
            fake_gh.chmod(fake_gh.stat().st_mode | stat.S_IXUSR)

            with mock.patch.dict(os.environ, {"PATH": str(bin_dir)}, clear=False):
                result = discover_with_gh("code review")

            self.assertEqual(
                result,
                [
                    {"name": "reviewer", "description": "Review code", "url": "https://github.com/acme/reviewer"},
                    {"name": "planner", "description": "Plan work", "url": "https://github.com/acme/planner"},
                ],
            )
            self.assertEqual(calls.read_text(encoding="utf-8").splitlines(), ["skill", "search", "code review", "--json"])

    def test_discover_with_gh_parses_text_output(self) -> None:
        with TemporaryDirectory() as tmp:
            bin_dir = Path(tmp)
            fake_gh = bin_dir / "gh"
            fake_gh.write_text(
                textwrap.dedent(
                    """\
                    #!/bin/sh
                    printf '%s\\n' 'reviewer - Review code - https://github.com/acme/reviewer'
                    printf '%b\\n' 'planner\\tPlan work\\thttps://github.com/acme/planner'
                    """
                ),
                encoding="utf-8",
            )
            fake_gh.chmod(fake_gh.stat().st_mode | stat.S_IXUSR)

            with mock.patch.dict(os.environ, {"PATH": str(bin_dir)}, clear=False):
                result = discover_with_gh("review")

            self.assertEqual(
                result,
                [
                    {"name": "reviewer", "description": "Review code", "url": "https://github.com/acme/reviewer"},
                    {"name": "planner", "description": "Plan work", "url": "https://github.com/acme/planner"},
                ],
            )

    def test_discover_with_gh_reports_missing_gh(self) -> None:
        with mock.patch.dict(os.environ, {"PATH": ""}):
            with self.assertRaisesRegex(RuntimeError, "gh command not found"):
                discover_with_gh("review")

    def test_discovery_result_to_import_source(self) -> None:
        self.assertEqual(
            "https://github.com/acme/reviewer",
            discovery_result_to_import_source({"name": "reviewer", "url": "https://github.com/acme/reviewer"}),
        )
        with self.assertRaisesRegex(ValueError, "importable source"):
            discovery_result_to_import_source({"name": "reviewer"})


if __name__ == "__main__":
    unittest.main()
