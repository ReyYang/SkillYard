PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS app_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    initial_scan_completed_at INTEGER
);

CREATE TABLE IF NOT EXISTS supported_app_status (
    app_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    detected INTEGER NOT NULL CHECK (detected IN (0, 1)),
    sort_order INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS inventory_observations (
    id TEXT PRIMARY KEY,
    skill_name TEXT NOT NULL,
    declared_name TEXT,
    skill_root TEXT NOT NULL,
    skill_file TEXT NOT NULL,
    location_kind TEXT NOT NULL,
    metadata_status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS inventory_observation_apps (
    observation_id TEXT NOT NULL REFERENCES inventory_observations(id) ON DELETE CASCADE,
    app_id TEXT NOT NULL,
    PRIMARY KEY (observation_id, app_id)
);

INSERT OR IGNORE INTO schema_migrations (version, applied_at)
VALUES (1, unixepoch());

INSERT OR IGNORE INTO app_state (singleton, initial_scan_completed_at)
VALUES (1, NULL);
