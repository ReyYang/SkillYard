-- 一个扫描根可以包含多个损坏 Skill；问题身份必须精确到路径和类型。
CREATE TABLE inventory_scan_issues_replacement (
    root_id TEXT NOT NULL,
    root_key TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    PRIMARY KEY (root_id, path, code)
);

INSERT INTO inventory_scan_issues_replacement (
    root_id, root_key, project_id, path, code, message
)
SELECT root_id, root_key, project_id, path, code, message
FROM inventory_scan_issues;

DROP TABLE inventory_scan_issues;
ALTER TABLE inventory_scan_issues_replacement RENAME TO inventory_scan_issues;

CREATE INDEX inventory_scan_issues_project_root
ON inventory_scan_issues(project_id, root_key);
