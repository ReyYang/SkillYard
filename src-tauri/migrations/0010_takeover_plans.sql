-- 接管 Plan 自带完整观察快照；刷新只显式作废尚未消费的 Plan。
CREATE TABLE takeover_plans (
    id TEXT PRIMARY KEY,
    observation_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    content_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    bundle_display_name TEXT NOT NULL,
    source_display_name TEXT,
    source_notice TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    skill_description TEXT NOT NULL,
    content_fingerprint TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    managed_directory TEXT NOT NULL,
    content_directory TEXT NOT NULL,
    expected_target TEXT NOT NULL,
    inventory_skill_name TEXT NOT NULL,
    inventory_declared_name TEXT,
    inventory_skill_root TEXT NOT NULL,
    inventory_skill_file TEXT NOT NULL,
    inventory_location_kind TEXT NOT NULL,
    inventory_metadata_status TEXT NOT NULL,
    inventory_observed_by_json TEXT NOT NULL,
    inventory_observed_fingerprint TEXT NOT NULL,
    inventory_root_key TEXT NOT NULL,
    inventory_project_id TEXT,
    inventory_stale INTEGER NOT NULL CHECK (inventory_stale IN (0, 1)),
    inventory_management_kind TEXT NOT NULL,
    inventory_management_evidence_empty INTEGER NOT NULL
        CHECK (inventory_management_evidence_empty = 1),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    CHECK (expires_at > created_at),
    UNIQUE (bundle_id),
    UNIQUE (content_id),
    UNIQUE (member_id)
);

-- 当前只有一条路径，但 schema 不把未来的多路径接管压成单列。
CREATE TABLE takeover_plan_paths (
    plan_id TEXT NOT NULL REFERENCES takeover_plans(id) ON DELETE CASCADE,
    path_id TEXT NOT NULL,
    mount_id TEXT NOT NULL,
    original_path TEXT NOT NULL,
    app_id TEXT NOT NULL CHECK (app_id IN ('codex', 'claude_code', 'github_copilot')),
    scope TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project_id TEXT REFERENCES projects(id),
    project_display_name TEXT,
    project_root_path TEXT,
    project_root_device INTEGER CHECK (project_root_device IS NULL OR project_root_device >= 0),
    project_root_inode INTEGER CHECK (project_root_inode IS NULL OR project_root_inode >= 0),
    parent_device INTEGER NOT NULL CHECK (parent_device >= 0),
    parent_inode INTEGER NOT NULL CHECK (parent_inode >= 0),
    parent_mode INTEGER NOT NULL CHECK (parent_mode >= 0),
    original_device INTEGER NOT NULL CHECK (original_device >= 0),
    original_inode INTEGER NOT NULL CHECK (original_inode >= 0),
    original_mode INTEGER NOT NULL CHECK (original_mode >= 0),
    default_preserve_mount INTEGER NOT NULL CHECK (default_preserve_mount IN (0, 1)),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (plan_id, path_id),
    UNIQUE (plan_id, mount_id),
    UNIQUE (plan_id, sort_order),
    CHECK (
        (scope = 'global'
            AND project_id IS NULL
            AND project_display_name IS NULL
            AND project_root_path IS NULL
            AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR
        (scope = 'project'
            AND project_id IS NOT NULL
            AND project_display_name IS NOT NULL
            AND project_root_path IS NOT NULL
            AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
    )
);
