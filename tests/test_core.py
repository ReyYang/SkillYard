from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

from skillyard.app import HostEntryConflict, SkillYardApp


class CoreFlowTests(unittest.TestCase):
    def test_init_import_git_and_expose_codex_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "review")
            app = SkillYardApp(root / "home", host_home=root / "host-home")

            app.apply(app.init())
            imported = app.import_git(str(repo), namespace="mattpocock")
            self.assertEqual(["mattpocock/review"], [entry["identity"] for entry in imported["entries"]])

            host_dir = root / "codex-skills"
            plan = app.expose_plan("mattpocock/review", "codex", "user", host_dir_override=host_dir)
            result = app.apply(plan)

            self.assertTrue(result["ok"])
            self.assertTrue((host_dir / "review").is_symlink())
            snapshot = app.state_snapshot()
            self.assertEqual(1, len(snapshot["sourceTrees"]))
            self.assertEqual(1, len(snapshot["libraryEntries"]))
            self.assertEqual(1, len(snapshot["exposures"]))

    def test_reimport_same_source_tree_updates_namespace_and_identity(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "review")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            app.import_git(str(repo), namespace="old")
            app.import_git(str(repo), namespace="new")

            entries = app.state_snapshot()["libraryEntries"]
            self.assertEqual(1, len(entries))
            self.assertEqual("new", entries[0]["namespace"])
            self.assertEqual("new/review", entries[0]["identity"])

    def test_host_entry_conflict_requires_resolution(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "review")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            app.import_git(str(repo), namespace="openai")

            host_dir = root / "codex-skills"
            (host_dir / "review").mkdir(parents=True)
            with self.assertRaises(HostEntryConflict):
                app.expose_plan("openai/review", "codex", "user", host_dir_override=host_dir)

            plan = app.expose_plan(
                "openai/review",
                "codex",
                "user",
                host_dir_override=host_dir,
                on_conflict="recommended",
            )
            self.assertEqual("openai-review", plan.payload["host_entry_name"])
            app.apply(plan)
            self.assertTrue((host_dir / "openai-review").exists())

    def test_replace_conflicting_managed_exposure_updates_state(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo_a = _make_git_repo(root / "repo-a", "same")
            repo_b = _make_git_repo(root / "repo-b", "same")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            app.import_git(str(repo_a), namespace="a")
            app.import_git(str(repo_b), namespace="b")
            host_dir = root / "skills"
            app.apply(app.expose_plan("a/same", "codex", "user", host_dir_override=host_dir))

            plan = app.expose_plan(
                "b/same",
                "codex",
                "user",
                host_dir_override=host_dir,
                on_conflict="replace",
            )
            app.apply(plan)

            exposures = app.state_snapshot()["exposures"]
            self.assertEqual(1, len(exposures))
            self.assertEqual("same", exposures[0]["host_entry_name"])
            self.assertEqual("b/same", app.state.conn.execute(
                "select identity from library_entries where id = ?",
                [exposures[0]["library_entry_id"]],
            ).fetchone()["identity"])

    def test_snapshot_fallback_and_doctor_snapshot_drift(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "audit")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            app.import_git(str(repo), namespace="product")

            host_dir = root / "skills"
            plan = app.expose_plan("product/audit", "codex", "user", host_dir_override=host_dir, mode="snapshot")
            app.apply(plan)
            self.assertFalse((host_dir / "audit").is_symlink())

            (host_dir / "audit" / "SKILL.md").write_text("changed locally\n", encoding="utf-8")
            findings = app.doctor(host_dir)
            self.assertIn("snapshot_drift", {finding["type"] for finding in findings})

    def test_doctor_reports_unmanaged_host_entry_and_dirty_source_tree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "doctor")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            imported = app.import_git(str(repo), namespace="ops")
            source_root = Path(app.state.get_source_tree(imported["source_tree_id"])["root_path"])
            (source_root / "uncommitted.txt").write_text("dirty\n", encoding="utf-8")

            host_dir = root / "skills"
            (host_dir / "manual" / "SKILL.md").parent.mkdir(parents=True)
            (host_dir / "manual" / "SKILL.md").write_text("---\nname: manual\n---\n", encoding="utf-8")

            findings = app.doctor(host_dir)
            self.assertIn("dirty_source_tree", {finding["type"] for finding in findings})
            self.assertIn("unmanaged_host_entry", {finding["type"] for finding in findings})

    def test_doctor_reports_managed_host_entry_replaced_by_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "managed")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            app.import_git(str(repo), namespace="ops")
            host_dir = root / "skills"
            app.apply(app.expose_plan("ops/managed", "codex", "user", host_dir_override=host_dir))

            target = host_dir / "managed"
            target.unlink()
            target.mkdir()
            (target / "SKILL.md").write_text("---\nname: managed\n---\n", encoding="utf-8")

            findings = app.doctor(host_dir)
            self.assertIn("host_entry_conflict", {finding["type"] for finding in findings})

    def test_update_preview_reports_impact_without_applying(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "updatable")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            imported = app.import_git(str(repo), namespace="tools")
            app.apply(app.expose_plan("tools/updatable", "codex", "user", host_dir_override=root / "skills"))

            skill_file = repo / "skills" / "updatable" / "SKILL.md"
            skill_file.write_text("---\nname: updatable\ndescription: changed\n---\n", encoding="utf-8")
            _git(repo, "add", ".")
            _git(repo, "commit", "-m", "change skill")

            preview = app.update_preview(imported["source_tree_id"])
            self.assertFalse(preview["blocked"])
            self.assertTrue(preview["changed"])
            self.assertEqual(["tools/updatable"], [entry["identity"] for entry in preview["library_entries"]])
            self.assertEqual(1, len(preview["exposures"]))

    def test_update_apply_uses_previewed_target_revision(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "stable")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            imported = app.import_git(str(repo), namespace="tools")

            skill_file = repo / "skills" / "stable" / "SKILL.md"
            skill_file.write_text("---\nname: stable\ndescription: first\n---\n", encoding="utf-8")
            _git(repo, "add", ".")
            _git(repo, "commit", "-m", "first change")
            first_sha = _git(repo, "rev-parse", "HEAD")
            preview = app.update_preview(imported["source_tree_id"])

            skill_file.write_text("---\nname: stable\ndescription: second\n---\n", encoding="utf-8")
            _git(repo, "add", ".")
            _git(repo, "commit", "-m", "second change")

            result = app.update_apply(imported["source_tree_id"], preview)

            self.assertTrue(result["ok"])
            source = app.state.get_source_tree(imported["source_tree_id"])
            self.assertEqual(first_sha, source["current_ref"])

    def test_expose_same_entry_to_multiple_hosts_and_project_scope(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = _make_git_repo(root / "repo", "shared")
            app = SkillYardApp(root / "home", host_home=root / "host-home")
            app.apply(app.init())
            app.import_git(str(repo), namespace="team")

            app.apply(app.expose_plan("team/shared", "claude", "user"))
            app.apply(app.expose_plan("team/shared", "cursor", "project", project_root=root / "project-a"))
            app.apply(app.expose_plan("team/shared", "copilot", "project", project_root=root / "project-b"))

            self.assertTrue((root / "host-home" / ".claude" / "skills" / "shared").is_symlink())
            self.assertTrue((root / "project-a" / ".cursor" / "skills" / "shared").is_symlink())
            self.assertTrue((root / "project-b" / ".github" / "copilot" / "skills" / "shared").is_symlink())
            exposures = app.state_snapshot()["exposures"]
            self.assertEqual(3, len(exposures))
            self.assertEqual({"user", "project"}, {exposure["scope"] for exposure in exposures})

    def test_package_source_tree_can_upgrade_to_git_source_tree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            package_root = root / "package-source"
            skill_dir = package_root / "skills" / "pkg"
            skill_dir.mkdir(parents=True)
            (skill_dir / "SKILL.md").write_text("---\nname: pkg\n---\n", encoding="utf-8")
            git_repo = _make_git_repo(root / "git-repo", "pkg")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            source_tree_id = app.state.insert_source_tree(
                kind="package",
                namespace="pkg",
                root_path=str(package_root),
                package_name="pkg",
                package_version="1.0.0",
            )
            app._discover_entries(source_tree_id, package_root, "pkg", confirmed_provenance=True)

            result = app.upgrade_package_source_tree_to_git(source_tree_id, str(git_repo))

            self.assertTrue(result["ok"])
            source = app.state.get_source_tree(source_tree_id)
            self.assertEqual("git", source["kind"])
            self.assertEqual(str(git_repo), source["repo_url"])
            self.assertTrue(Path(source["root_path"]).is_relative_to(app.sources_dir / "git"))

    def test_package_source_tree_upgrade_rejects_non_matching_repo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            package_root = root / "package-source"
            skill_dir = package_root / "skills" / "pkg"
            skill_dir.mkdir(parents=True)
            (skill_dir / "SKILL.md").write_text("---\nname: pkg\n---\n", encoding="utf-8")
            git_repo = _make_git_repo(root / "git-repo", "other")
            app = SkillYardApp(root / "home")
            app.apply(app.init())
            source_tree_id = app.state.insert_source_tree(
                kind="package",
                namespace="pkg",
                root_path=str(package_root),
                package_name="pkg",
                package_version="1.0.0",
            )
            app._discover_entries(source_tree_id, package_root, "pkg", confirmed_provenance=True)

            with self.assertRaisesRegex(ValueError, "does not match"):
                app.upgrade_package_source_tree_to_git(source_tree_id, str(git_repo))


def _make_git_repo(path: Path, skill_name: str) -> Path:
    (path / "skills" / skill_name).mkdir(parents=True)
    (path / "skills" / skill_name / "SKILL.md").write_text(
        f"---\nname: {skill_name}\ndescription: Test skill\n---\n\n# {skill_name}\n",
        encoding="utf-8",
    )
    _git(path, "init")
    _git(path, "config", "user.email", "test@example.com")
    _git(path, "config", "user.name", "Test User")
    _git(path, "add", ".")
    _git(path, "commit", "-m", "initial skill")
    return path


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return result.stdout.strip()


if __name__ == "__main__":
    unittest.main()
