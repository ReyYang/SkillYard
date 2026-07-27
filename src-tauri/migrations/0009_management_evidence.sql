-- 管理证据随观察快照替换；删除观察时不能留下脱离主体的旧证据。
CREATE TABLE inventory_management_evidence (
    observation_id TEXT PRIMARY KEY
        REFERENCES inventory_observations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind = 'git_head_tracked'),
    authority_root TEXT NOT NULL,
    snapshot_commit_oid TEXT NOT NULL,
    subject_path TEXT NOT NULL
);
