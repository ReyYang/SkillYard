ALTER TABLE inventory_observations
ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE CASCADE;

CREATE INDEX inventory_observations_project_root
ON inventory_observations(project_id, root_key);

-- 扫描问题必须按“物理根 + Project”隔离，不能让两个 Project 相互覆盖。
CREATE TABLE inventory_scan_issues_v2 (
    root_id TEXT PRIMARY KEY,
    root_key TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL
);

INSERT INTO inventory_scan_issues_v2 (root_id, root_key, project_id, path, code, message)
SELECT 'global:' || root_key, root_key, NULL, path, code, message
FROM inventory_scan_issues;

DROP TABLE inventory_scan_issues;
ALTER TABLE inventory_scan_issues_v2 RENAME TO inventory_scan_issues;

CREATE INDEX inventory_scan_issues_project_root
ON inventory_scan_issues(project_id, root_key);
