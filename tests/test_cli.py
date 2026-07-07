from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class CliTests(unittest.TestCase):
    def test_import_local_prints_plan_until_yes_applies(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source" / "skill"
            source.mkdir(parents=True)
            (source / "SKILL.md").write_text("---\nname: cli-skill\n---\n", encoding="utf-8")
            home = root / "home"

            preview = _cli(root, "--home", str(home), "--json", "import", "--local", str(source.parent), "--namespace", "personal")
            self.assertEqual("import_local", preview["plan"]["payload"]["kind"])
            self.assertEqual([], _state_entries(home))

            applied = _cli(
                root,
                "--home",
                str(home),
                "--json",
                "import",
                "--local",
                str(source.parent),
                "--namespace",
                "personal",
                "--yes",
            )
            self.assertTrue(applied["result"]["entries"])
            self.assertEqual(["personal/cli-skill"], _state_entries(home))

    def test_ai_assist_cli_does_not_create_state_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            skill = root / "skill"
            skill.mkdir()
            (skill / "SKILL.md").write_text("---\nname: ai-only\n---\n", encoding="utf-8")
            home = root / "home"

            result = _cli(root, "--home", str(home), "--json", "doctor", "--explain-skill", str(skill))

            self.assertEqual("ai-only", result["aiAssist"]["name"])
            self.assertFalse((home / "state.sqlite3").exists())

    def test_init_preview_and_doctor_are_read_only_before_apply(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home = root / "home"
            preview = _cli(root, "--home", str(home), "--json", "init")
            self.assertEqual("init", preview["plan"]["payload"]["kind"])
            self.assertFalse((home / "state.sqlite3").exists())

            doctor = _cli(root, "--home", str(home), "--json", "doctor")
            self.assertEqual([], doctor["findings"])
            self.assertFalse((home / "state.sqlite3").exists())

            failed = subprocess.run(
                [sys.executable, "-m", "skillyard", "--home", str(home), "--json", "expose", "missing/skill"],
                cwd=root,
                env={**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1])},
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            self.assertNotEqual(0, failed.returncode)
            self.assertFalse((home / "state.sqlite3").exists())


def _cli(cwd: Path, *args: str) -> dict:
    repo_root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env["PYTHONPATH"] = str(repo_root)
    result = subprocess.run(
        [sys.executable, "-m", "skillyard", *args],
        cwd=cwd,
        env=env,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return json.loads(result.stdout)


def _state_entries(home: Path) -> list[str]:
    import sqlite3

    db = home / "state.sqlite3"
    if not db.exists():
        return []
    conn = sqlite3.connect(db)
    try:
        rows = conn.execute("select identity from library_entries order by identity").fetchall()
    except sqlite3.OperationalError:
        return []
    finally:
        conn.close()
    return [row[0] for row in rows]


if __name__ == "__main__":
    unittest.main()
