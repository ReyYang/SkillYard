from __future__ import annotations

import json
import sqlite3
import time
from pathlib import Path
from typing import Any


class State:
    def __init__(self, path: Path):
        self.path = path
        self._conn: sqlite3.Connection | None = None

    @property
    def conn(self) -> sqlite3.Connection:
        if self._conn is None:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self._conn = sqlite3.connect(self.path)
            self._conn.row_factory = sqlite3.Row
        return self._conn

    def close(self) -> None:
        if self._conn is not None:
            self._conn.close()
            self._conn = None

    def exists(self) -> bool:
        return self.path.exists()

    def initialize(self) -> None:
        self.conn.executescript(
            """
            create table if not exists source_trees (
                id integer primary key autoincrement,
                kind text not null,
                namespace text not null,
                root_path text not null,
                repo_url text,
                current_ref text,
                package_name text,
                package_version text,
                tarball_url text,
                integrity text,
                repository_url text,
                created_at integer not null
            );
            create unique index if not exists source_trees_root_path_uq
                on source_trees(root_path);

            create table if not exists library_entries (
                id integer primary key autoincrement,
                source_tree_id integer not null references source_trees(id),
                identity text not null unique,
                namespace text not null,
                name text not null,
                display_label text not null,
                skill_path text not null,
                description text not null default '',
                confirmed_provenance integer not null default 1,
                created_at integer not null
            );

            create table if not exists exposures (
                id integer primary key autoincrement,
                library_entry_id integer not null references library_entries(id),
                host text not null,
                scope text not null,
                project_root text,
                host_entry_name text not null,
                mode text not null,
                target_path text not null,
                created_at integer not null
            );
            create unique index if not exists exposures_host_entry_uq
                on exposures(host, scope, ifnull(project_root, ''), host_entry_name);

            create table if not exists events (
                id integer primary key autoincrement,
                type text not null,
                message text not null,
                data_json text not null default '{}',
                created_at integer not null
            );

            create table if not exists install_receipts (
                id integer primary key autoincrement,
                command_json text not null,
                provider text,
                package_name text,
                package_version text,
                before_snapshot_json text not null default '{}',
                after_snapshot_json text not null default '{}',
                source_metadata_json text not null default '{}',
                changed_entries_json text not null default '[]',
                created_at integer not null
            );

            create table if not exists provenance_inferences (
                id integer primary key autoincrement,
                library_entry_id integer,
                source_candidate_path text,
                evidence_json text not null,
                confidence real not null,
                accepted integer not null default 0,
                created_at integer not null
            );
            """
        )
        self.conn.commit()

    def insert_source_tree(self, **values: Any) -> int:
        values.setdefault("created_at", _now())
        keys = list(values)
        placeholders = ", ".join("?" for _ in keys)
        sql = f"insert into source_trees ({', '.join(keys)}) values ({placeholders})"
        cur = self.conn.execute(sql, [values[k] for k in keys])
        self.conn.commit()
        return int(cur.lastrowid)

    def upsert_source_tree_by_root(self, **values: Any) -> int:
        existing = self.get_source_tree_by_root(values["root_path"])
        if existing:
            return int(existing["id"])
        return self.insert_source_tree(**values)

    def insert_library_entry(self, **values: Any) -> int:
        values.setdefault("created_at", _now())
        try:
            cur = self.conn.execute(
                """
                insert into library_entries (
                    source_tree_id, identity, namespace, name, display_label,
                    skill_path, description, confirmed_provenance, created_at
                ) values (?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    values["source_tree_id"],
                    values["identity"],
                    values["namespace"],
                    values["name"],
                    values["display_label"],
                    values["skill_path"],
                    values.get("description", ""),
                    int(values.get("confirmed_provenance", 1)),
                    values["created_at"],
                ],
            )
            self.conn.commit()
            return int(cur.lastrowid)
        except sqlite3.IntegrityError:
            row = self.get_library_entry(values["identity"])
            if row is None:
                raise
            return int(row["id"])

    def insert_exposure(self, **values: Any) -> int:
        values.setdefault("created_at", _now())
        cur = self.conn.execute(
            """
            insert into exposures (
                library_entry_id, host, scope, project_root, host_entry_name,
                mode, target_path, created_at
            ) values (?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                values["library_entry_id"],
                values["host"],
                values["scope"],
                values.get("project_root"),
                values["host_entry_name"],
                values["mode"],
                values["target_path"],
                values["created_at"],
            ],
        )
        self.conn.commit()
        return int(cur.lastrowid)

    def insert_event(self, event_type: str, message: str, data: dict[str, Any] | None = None) -> None:
        self.conn.execute(
            "insert into events (type, message, data_json, created_at) values (?, ?, ?, ?)",
            [event_type, message, json.dumps(data or {}, sort_keys=True), _now()],
        )
        self.conn.commit()

    def insert_install_receipt(self, **values: Any) -> int:
        values.setdefault("created_at", _now())
        cur = self.conn.execute(
            """
            insert into install_receipts (
                command_json, provider, package_name, package_version,
                before_snapshot_json, after_snapshot_json,
                source_metadata_json, changed_entries_json, created_at
            ) values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                json.dumps(values.get("command", [])),
                values.get("provider"),
                values.get("package_name"),
                values.get("package_version"),
                json.dumps(values.get("before_snapshot", {}), sort_keys=True),
                json.dumps(values.get("after_snapshot", {}), sort_keys=True),
                json.dumps(values.get("source_metadata", {}), sort_keys=True),
                json.dumps(values.get("changed_entries", []), sort_keys=True),
                values["created_at"],
            ],
        )
        self.conn.commit()
        return int(cur.lastrowid)

    def insert_provenance_inference(self, **values: Any) -> int:
        values.setdefault("created_at", _now())
        cur = self.conn.execute(
            """
            insert into provenance_inferences (
                library_entry_id, source_candidate_path, evidence_json,
                confidence, accepted, created_at
            ) values (?, ?, ?, ?, ?, ?)
            """,
            [
                values.get("library_entry_id"),
                values.get("source_candidate_path"),
                json.dumps(values.get("evidence", {}), sort_keys=True),
                float(values.get("confidence", 0)),
                int(values.get("accepted", 0)),
                values["created_at"],
            ],
        )
        self.conn.commit()
        return int(cur.lastrowid)

    def get_source_tree(self, source_tree_id: int) -> sqlite3.Row | None:
        return self.conn.execute("select * from source_trees where id = ?", [source_tree_id]).fetchone()

    def get_source_tree_by_root(self, root_path: str) -> sqlite3.Row | None:
        return self.conn.execute("select * from source_trees where root_path = ?", [root_path]).fetchone()

    def get_library_entry(self, identity: str) -> sqlite3.Row | None:
        return self.conn.execute("select * from library_entries where identity = ?", [identity]).fetchone()

    def list_source_trees(self) -> list[sqlite3.Row]:
        if not self.exists():
            return []
        return list(self.conn.execute("select * from source_trees order by namespace, id"))

    def list_library_entries(self) -> list[sqlite3.Row]:
        if not self.exists():
            return []
        return list(self.conn.execute("select * from library_entries order by identity"))

    def list_exposures(self) -> list[sqlite3.Row]:
        if not self.exists():
            return []
        return list(self.conn.execute("select * from exposures order by host, scope, host_entry_name"))

    def list_events(self) -> list[sqlite3.Row]:
        if not self.exists():
            return []
        return list(self.conn.execute("select * from events order by id"))


def row_to_dict(row: sqlite3.Row) -> dict[str, Any]:
    return {key: row[key] for key in row.keys()}


def _now() -> int:
    return int(time.time())
