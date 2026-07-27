-- 扫描证据随观察快照替换；接管后改由 Skill Member 保存，不再依赖外部 lock。
CREATE TABLE inventory_installation_chains (
    observation_id TEXT PRIMARY KEY
        REFERENCES inventory_observations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind = 'lock_v3'),
    record_path TEXT NOT NULL,
    source TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_locator TEXT NOT NULL,
    skill_path TEXT,
    tracked_ref TEXT,
    content_marker TEXT NOT NULL,
    installed_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE member_installation_chains (
    member_id TEXT PRIMARY KEY
        REFERENCES skill_members(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind = 'lock_v3'),
    record_path TEXT NOT NULL,
    source TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_locator TEXT NOT NULL,
    skill_path TEXT,
    tracked_ref TEXT,
    content_marker TEXT NOT NULL,
    installed_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
