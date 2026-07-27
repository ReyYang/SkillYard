ALTER TABLE app_state ADD COLUMN last_local_refresh_at INTEGER;
ALTER TABLE app_state ADD COLUMN last_local_refresh_added INTEGER NOT NULL DEFAULT 0;
ALTER TABLE app_state ADD COLUMN last_local_refresh_changed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE app_state ADD COLUMN last_local_refresh_removed INTEGER NOT NULL DEFAULT 0;

ALTER TABLE inventory_observations ADD COLUMN observed_fingerprint TEXT NOT NULL DEFAULT '';
ALTER TABLE inventory_observations ADD COLUMN root_key TEXT NOT NULL DEFAULT 'shared_agents';
ALTER TABLE inventory_observations ADD COLUMN stale INTEGER NOT NULL DEFAULT 0 CHECK (stale IN (0, 1));
ALTER TABLE inventory_observations ADD COLUMN management_kind TEXT NOT NULL DEFAULT 'takeover_candidate';

-- 已有开发数据库通过明确 App 关系恢复 root key；之后每次刷新会写入完整新值。
UPDATE inventory_observations
SET root_key = 'codex_global'
WHERE location_kind = 'app_global'
  AND id IN (
    SELECT observation_id FROM inventory_observation_apps WHERE app_id = 'codex'
  );

UPDATE inventory_observations
SET root_key = 'claude_code_global'
WHERE location_kind = 'app_global'
  AND id IN (
    SELECT observation_id FROM inventory_observation_apps WHERE app_id = 'claude_code'
  );

UPDATE inventory_observations
SET root_key = 'github_copilot_global'
WHERE location_kind = 'app_global'
  AND id IN (
    SELECT observation_id FROM inventory_observation_apps WHERE app_id = 'github_copilot'
  );

CREATE TABLE inventory_scan_issues (
    root_key TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL
);
