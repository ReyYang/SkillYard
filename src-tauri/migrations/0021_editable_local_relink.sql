-- 重新关联只保存用户确认所需的 metadata，不创建文件系统事务或内容快照。
CREATE TABLE editable_local_relink_plans (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    expected_canonical_identity TEXT NOT NULL,
    expected_source_display_name TEXT NOT NULL,
    expected_locator TEXT NOT NULL,
    expected_device INTEGER NOT NULL,
    expected_inode INTEGER NOT NULL,
    expected_catalog_generation INTEGER NOT NULL CHECK (expected_catalog_generation > 0),
    expected_catalog_marker TEXT NOT NULL,
    expected_bundle_id TEXT,
    expected_bundle_display_name TEXT,
    candidate_path TEXT NOT NULL,
    candidate_display_name TEXT NOT NULL,
    candidate_marker TEXT NOT NULL,
    candidate_members_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK (expires_at > created_at),
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    CHECK (
        (expected_bundle_id IS NULL AND expected_bundle_display_name IS NULL)
        OR
        (expected_bundle_id IS NOT NULL AND expected_bundle_display_name IS NOT NULL)
    )
);

-- 桌面应用一次只展示一个确认页；新计划会明确消费旧的未确认计划。
CREATE UNIQUE INDEX editable_local_relink_one_pending
ON editable_local_relink_plans(status)
WHERE status = 'pending';
