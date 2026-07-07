from __future__ import annotations

import json
import stat
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

from skillyard.app import SkillYardApp
from skillyard.captured_install import capture_install


class CapturedInstallTests(unittest.TestCase):
    def test_npx_install_creates_package_source_tree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = SkillYardApp(root / "home", host_home=root / "host-home")
            host_dir = root / "host-skills"
            npx = _fake_npx(root)

            with mock.patch.dict(
                "os.environ",
                {
                    "SKILLYARD_NPM_VIEW_JSON": json.dumps(
                        {
                            "version": "1.2.3",
                            "repository": {"url": "git+https://github.com/scope/fake-package.git"},
                            "dist": {
                                "tarball": "https://registry.npmjs.org/@scope/fake-package/-/fake-package-1.2.3.tgz",
                                "integrity": "sha512-test",
                            },
                        }
                    )
                },
            ):
                result = capture_install(
                    app,
                    "codex",
                    host_dir,
                    [str(npx), "-y", "@scope/fake-package@1.2.3", str(host_dir), "pkg-skill"],
                )

            self.assertEqual(result["package"], {"name": "@scope/fake-package", "version": "1.2.3"})
            self.assertEqual(result["source_tree_kind"], "package")
            self.assertEqual(result["changed_entries"][0]["name"], "pkg-skill")
            self.assertEqual(result["changed_entries"][0]["status"], "added")

            source = app.state.get_source_tree(result["source_tree_id"])
            self.assertIsNotNone(source)
            assert source is not None
            self.assertEqual(source["kind"], "package")
            self.assertEqual(source["package_name"], "@scope/fake-package")
            self.assertEqual(source["package_version"], "1.2.3")
            self.assertEqual(source["repository_url"], "https://github.com/scope/fake-package")
            self.assertEqual(source["tarball_url"], "https://registry.npmjs.org/@scope/fake-package/-/fake-package-1.2.3.tgz")
            self.assertEqual(source["integrity"], "sha512-test")
            self.assertTrue(Path(source["root_path"]).is_relative_to(app.packages_dir))
            self.assertTrue((Path(source["root_path"]) / "pkg-skill" / "SKILL.md").exists())

            entries = app.state.list_library_entries()
            self.assertEqual(len(entries), 1)
            self.assertEqual(entries[0]["confirmed_provenance"], 1)
            self.assertEqual(entries[0]["skill_path"], "pkg-skill/SKILL.md")

            receipt = _receipt(app, result["install_receipt_id"])
            self.assertEqual(receipt["package_name"], "@scope/fake-package")
            self.assertEqual(receipt["package_version"], "1.2.3")
            self.assertEqual(json.loads(receipt["before_snapshot_json"]), {})
            self.assertIn("pkg-skill", json.loads(receipt["after_snapshot_json"]))

    def test_unknown_install_creates_candidate_with_unconfirmed_entries(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = SkillYardApp(root / "home", host_home=root / "host-home")
            host_dir = root / "host-skills"
            installer = _fake_installer(root)

            result = capture_install(
                app,
                "codex",
                host_dir,
                [sys.executable, str(installer), str(host_dir), "candidate-skill"],
            )

            self.assertIsNone(result["package"])
            self.assertEqual(result["source_tree_kind"], "candidate")
            self.assertEqual(result["changed_entries"][0]["status"], "added")

            source = app.state.get_source_tree(result["source_tree_id"])
            self.assertIsNotNone(source)
            assert source is not None
            self.assertEqual(source["kind"], "candidate")
            self.assertTrue(Path(source["root_path"]).is_relative_to(app.candidates_dir))
            self.assertTrue((Path(source["root_path"]) / "candidate-skill" / "SKILL.md").exists())

            entries = app.state.list_library_entries()
            self.assertEqual(len(entries), 1)
            self.assertEqual(entries[0]["confirmed_provenance"], 0)

            inference = app.state.conn.execute("select * from provenance_inferences").fetchone()
            self.assertIsNotNone(inference)
            assert inference is not None
            self.assertEqual(inference["library_entry_id"], entries[0]["id"])
            self.assertEqual(inference["source_candidate_path"], str(Path(source["root_path"])))

            receipt = _receipt(app, result["install_receipt_id"])
            self.assertIsNone(receipt["package_name"])
            self.assertEqual(json.loads(receipt["changed_entries_json"])[0]["name"], "candidate-skill")

    def test_capture_detects_changed_host_entry(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = SkillYardApp(root / "home", host_home=root / "host-home")
            host_dir = root / "host-skills"
            skill_dir = host_dir / "existing-skill"
            skill_dir.mkdir(parents=True)
            (skill_dir / "SKILL.md").write_text(
                "---\nname: existing-skill\ndescription: old\n---\n",
                encoding="utf-8",
            )
            installer = _fake_installer(root, description="new")

            result = capture_install(
                app,
                "codex",
                host_dir,
                [sys.executable, str(installer), str(host_dir), "existing-skill"],
            )

            self.assertEqual(result["changed_entries"][0]["name"], "existing-skill")
            self.assertEqual(result["changed_entries"][0]["status"], "changed")
            source = app.state.get_source_tree(result["source_tree_id"])
            self.assertIsNotNone(source)
            assert source is not None
            self.assertIn("description: new", (Path(source["root_path"]) / "existing-skill" / "SKILL.md").read_text())

    def test_capture_records_deleted_host_entry(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = SkillYardApp(root / "home", host_home=root / "host-home")
            host_dir = root / "host-skills"
            skill_dir = host_dir / "deleted-skill"
            skill_dir.mkdir(parents=True)
            (skill_dir / "SKILL.md").write_text("---\nname: deleted-skill\n---\n", encoding="utf-8")
            remover = root / "remove.py"
            remover.write_text(
                "from pathlib import Path\nimport shutil, sys\nshutil.rmtree(Path(sys.argv[1]) / 'deleted-skill')\n",
                encoding="utf-8",
            )

            result = capture_install(app, "codex", host_dir, [sys.executable, str(remover), str(host_dir)])

            self.assertEqual("deleted", result["changed_entries"][0]["status"])
            receipt = _receipt(app, result["install_receipt_id"])
            self.assertIn("deleted-skill", json.loads(receipt["before_snapshot_json"]))
            self.assertEqual(json.loads(receipt["after_snapshot_json"]), {})


def _fake_installer(root: Path, description: str = "installed") -> Path:
    script = root / f"fake_installer_{description}.py"
    script.write_text(
        textwrap.dedent(
            f"""
            from pathlib import Path
            import sys

            host_dir = Path(sys.argv[1])
            entry_name = sys.argv[2]
            skill_dir = host_dir / entry_name
            skill_dir.mkdir(parents=True, exist_ok=True)
            (skill_dir / "SKILL.md").write_text(
                "---\\nname: " + entry_name + "\\ndescription: {description}\\n---\\n",
                encoding="utf-8",
            )
            """
        ),
        encoding="utf-8",
    )
    return script


def _fake_npx(root: Path) -> Path:
    script = root / "npx"
    script.write_text(
        textwrap.dedent(
            """
            #!/usr/bin/env python3
            from pathlib import Path
            import sys

            host_dir = Path(sys.argv[-2])
            entry_name = sys.argv[-1]
            skill_dir = host_dir / entry_name
            skill_dir.mkdir(parents=True, exist_ok=True)
            (skill_dir / "SKILL.md").write_text(
                "---\\nname: " + entry_name + "\\ndescription: package install\\n---\\n",
                encoding="utf-8",
            )
            """
        ).lstrip(),
        encoding="utf-8",
    )
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return script


def _receipt(app: SkillYardApp, receipt_id: int):
    receipt = app.state.conn.execute("select * from install_receipts where id = ?", [receipt_id]).fetchone()
    assert receipt is not None
    return receipt


if __name__ == "__main__":
    unittest.main()
