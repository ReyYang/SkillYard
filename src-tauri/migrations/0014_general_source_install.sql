-- Stage 6 将 GitHub 专用来源原地泛化为唯一 Source 协议，并保留未完成的 Plan 与事务。
CREATE TEMP TABLE migration_0014_sources AS SELECT * FROM sources;
CREATE TEMP TABLE migration_0014_source_catalog_members AS SELECT * FROM source_catalog_members;
CREATE TEMP TABLE migration_0014_source_bundle_links AS SELECT * FROM source_bundle_links;
CREATE TEMP TABLE migration_0014_source_member_links AS SELECT * FROM source_member_links;
CREATE TEMP TABLE migration_0014_source_ref_change_plans AS SELECT * FROM source_ref_change_plans;
CREATE TEMP TABLE migration_0014_install_plans AS SELECT * FROM install_plans;
CREATE TEMP TABLE migration_0014_install_plan_candidates AS SELECT * FROM install_plan_candidates;
CREATE TEMP TABLE migration_0014_lifecycle_transactions AS SELECT * FROM lifecycle_transactions;

-- 重建被 lifecycle_transactions 引用的单写者约束，不能让迁移产生第二套事务协议。
DROP TRIGGER mount_transaction_reject_active_install;
DROP TRIGGER install_transaction_reject_active_mount;
DROP TRIGGER batch_mount_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_batch_mount;
DROP TRIGGER takeover_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_takeover;

DROP TABLE lifecycle_transactions;
DROP TABLE install_plan_candidates;
DROP TABLE install_plans;
DROP TABLE source_ref_change_plans;
DROP TABLE source_member_links;
DROP TABLE source_bundle_links;
DROP TABLE source_catalog_members;
DROP TABLE sources;

CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('github', 'archive', 'direct_url', 'editable_local')),
    canonical_identity TEXT NOT NULL UNIQUE,
    owner TEXT,
    repository TEXT,
    display_name TEXT NOT NULL,
    locator TEXT NOT NULL,
    tracked_ref TEXT,
    member_path_hint TEXT,
    filesystem_device INTEGER,
    filesystem_inode INTEGER,
    catalog_status TEXT NOT NULL DEFAULT 'unloaded'
        CHECK (catalog_status IN ('unloaded', 'fresh', 'stale')),
    catalog_generation INTEGER NOT NULL DEFAULT 0 CHECK (catalog_generation >= 0),
    catalog_marker TEXT,
    catalog_fetched_at INTEGER,
    last_reload_at INTEGER,
    last_reload_error TEXT,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (kind <> 'github' OR canonical_identity = lower(canonical_identity)),
    CHECK (
        (
            kind = 'github'
            AND owner IS NOT NULL
            AND repository IS NOT NULL
            AND tracked_ref IS NOT NULL
            AND filesystem_device IS NULL
            AND filesystem_inode IS NULL
        )
        OR
        (
            kind IN ('archive', 'direct_url')
            AND owner IS NULL
            AND repository IS NULL
            AND tracked_ref IS NULL
            AND filesystem_device IS NULL
            AND filesystem_inode IS NULL
        )
        OR
        (
            kind = 'editable_local'
            AND owner IS NULL
            AND repository IS NULL
            AND tracked_ref IS NULL
            AND filesystem_device IS NOT NULL
            AND filesystem_inode IS NOT NULL
        )
    ),
    CHECK (
        (catalog_status = 'unloaded'
            AND catalog_generation = 0
            AND catalog_marker IS NULL
            AND catalog_fetched_at IS NULL)
        OR (catalog_status = 'fresh'
            AND catalog_generation > 0
            AND catalog_marker IS NOT NULL
            AND catalog_fetched_at IS NOT NULL
            AND last_reload_error IS NULL)
        OR (catalog_status = 'stale'
            AND catalog_generation > 0
            AND catalog_marker IS NOT NULL
            AND catalog_fetched_at IS NOT NULL
            AND last_reload_error IS NOT NULL)
    )
);

CREATE UNIQUE INDEX sources_sort_order ON sources(sort_order);

CREATE TABLE source_catalog_members (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    catalog_generation INTEGER NOT NULL CHECK (catalog_generation > 0),
    relative_path TEXT NOT NULL,
    skill_name TEXT,
    description TEXT,
    content_fingerprint TEXT,
    selectable INTEGER NOT NULL CHECK (selectable IN (0, 1)),
    validation_errors_json TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    UNIQUE (source_id, relative_path),
    UNIQUE (source_id, sort_order)
);

CREATE TABLE source_bundle_links (
    source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    bundle_id TEXT NOT NULL UNIQUE REFERENCES bundles(id) ON DELETE CASCADE,
    adopted_marker TEXT,
    linked_at INTEGER NOT NULL
);

CREATE TABLE source_member_links (
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    source_relative_path TEXT NOT NULL,
    member_id TEXT NOT NULL UNIQUE REFERENCES skill_members(id) ON DELETE CASCADE,
    linked_at INTEGER NOT NULL,
    PRIMARY KEY (source_id, source_relative_path)
);

-- Ref 变更仍是 GitHub 专属的 metadata 确认，不参与文件系统事务。
CREATE TABLE source_ref_change_plans (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    current_ref TEXT NOT NULL,
    candidate_ref TEXT NOT NULL,
    candidate_commit_sha TEXT NOT NULL,
    member_path_hint TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK (expires_at > created_at),
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed'))
);

CREATE TABLE install_plans (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('folder_snapshot', 'source_snapshot')),
    install_mode TEXT NOT NULL CHECK (install_mode IN ('create', 'supplement')),
    input_path TEXT,
    input_device INTEGER NOT NULL,
    input_inode INTEGER NOT NULL,
    input_fingerprint TEXT NOT NULL,
    snapshot_relative_path TEXT,
    source_id TEXT REFERENCES sources(id),
    source_tracked_ref TEXT,
    source_catalog_generation INTEGER CHECK (source_catalog_generation > 0),
    source_marker TEXT,
    expected_current_target TEXT,
    expected_adopted_marker TEXT,
    bundle_id TEXT NOT NULL,
    bundle_display_name TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    CHECK (
        (
            kind = 'folder_snapshot'
            AND install_mode = 'create'
            AND input_path IS NOT NULL
            AND snapshot_relative_path IS NULL
            AND source_id IS NULL
            AND source_tracked_ref IS NULL
            AND source_catalog_generation IS NULL
            AND source_marker IS NULL
            AND expected_current_target IS NULL
            AND expected_adopted_marker IS NULL
        )
        OR
        (
            kind = 'source_snapshot'
            AND input_path IS NULL
            AND snapshot_relative_path IS NOT NULL
            AND source_id IS NOT NULL
            AND source_catalog_generation IS NOT NULL
            AND source_marker IS NOT NULL
            AND (
                (
                    install_mode = 'create'
                    AND expected_current_target IS NULL
                    AND expected_adopted_marker IS NULL
                )
                OR
                (
                    install_mode = 'supplement'
                    AND expected_current_target IS NOT NULL
                )
            )
        )
    )
);

CREATE TABLE install_plan_candidates (
    plan_id TEXT NOT NULL REFERENCES install_plans(id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    source_relative_path TEXT NOT NULL,
    skill_name TEXT,
    skill_description TEXT,
    content_fingerprint TEXT,
    selectable INTEGER NOT NULL CHECK (selectable IN (0, 1)),
    preserve_existing INTEGER NOT NULL CHECK (preserve_existing IN (0, 1)),
    validation_errors_json TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    default_selected INTEGER NOT NULL CHECK (default_selected IN (0, 1)),
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (plan_id, candidate_id),
    UNIQUE (plan_id, source_relative_path),
    UNIQUE (plan_id, sort_order),
    CHECK (
        preserve_existing = 0
        OR (
            selectable = 0
            AND default_selected = 1
            AND selected = 1
            AND skill_name IS NOT NULL
            AND skill_description IS NOT NULL
            AND content_fingerprint IS NOT NULL
        )
    ),
    CHECK (selected = 0 OR selectable = 1 OR preserve_existing = 1),
    CHECK (default_selected = 0 OR selectable = 1 OR preserve_existing = 1)
);

CREATE TABLE lifecycle_transactions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind = 'install_bundle'),
    plan_id TEXT NOT NULL UNIQUE REFERENCES install_plans(id),
    bundle_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN ('journal_pending', 'journal_ready', 'candidate_ready', 'activated', 'state_committed')
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 现有 GitHub 数据逐列映射到通用 Source 字段。
INSERT INTO sources (
    id, kind, canonical_identity, owner, repository, display_name, locator,
    tracked_ref, member_path_hint, filesystem_device, filesystem_inode,
    catalog_status, catalog_generation, catalog_marker, catalog_fetched_at,
    last_reload_at, last_reload_error, sort_order, created_at, updated_at
)
SELECT
    id, kind, canonical_identity, owner, repository, display_name, repository_url,
    tracked_ref, member_path_hint, NULL, NULL,
    catalog_status, catalog_generation, catalog_commit_sha, catalog_fetched_at,
    last_reload_at, last_reload_error, sort_order, created_at, updated_at
FROM migration_0014_sources;

INSERT INTO source_catalog_members SELECT * FROM migration_0014_source_catalog_members;

INSERT INTO source_bundle_links (source_id, bundle_id, adopted_marker, linked_at)
SELECT source_id, bundle_id, adopted_commit_sha, linked_at
FROM migration_0014_source_bundle_links;

INSERT INTO source_member_links SELECT * FROM migration_0014_source_member_links;
INSERT INTO source_ref_change_plans SELECT * FROM migration_0014_source_ref_change_plans;

INSERT INTO install_plans (
    id, kind, install_mode, input_path, input_device, input_inode,
    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
    source_catalog_generation, source_marker, expected_current_target,
    expected_adopted_marker, bundle_id, bundle_display_name,
    warnings_json, created_at, expires_at, status
)
SELECT
    id,
    CASE kind WHEN 'github_snapshot' THEN 'source_snapshot' ELSE kind END,
    install_mode, input_path, input_device, input_inode,
    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
    source_catalog_generation, source_commit_sha, expected_current_target,
    expected_adopted_commit_sha, bundle_id, bundle_display_name,
    warnings_json, created_at, expires_at, status
FROM migration_0014_install_plans;

INSERT INTO install_plan_candidates SELECT * FROM migration_0014_install_plan_candidates;
INSERT INTO lifecycle_transactions SELECT * FROM migration_0014_lifecycle_transactions;

CREATE UNIQUE INDEX lifecycle_single_active
ON lifecycle_transactions ((1))
WHERE status = 'in_progress';

CREATE TRIGGER mount_transaction_reject_active_install
BEFORE INSERT ON mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER install_transaction_reject_active_mount
BEFORE INSERT ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM mount_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_writer
BEFORE INSERT ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER install_transaction_reject_active_batch_mount
BEFORE INSERT ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER takeover_transaction_reject_active_writer
BEFORE INSERT ON takeover_transactions
WHEN NEW.status = 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER install_transaction_reject_active_takeover
BEFORE INSERT ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

-- 映射后的任意悬空外键都会让 migration 整体回滚，不能带病启动。
CREATE TEMP TABLE migration_0014_foreign_key_guard (
    issue_count INTEGER NOT NULL CHECK (issue_count = 0)
);

INSERT INTO migration_0014_foreign_key_guard (issue_count)
SELECT COUNT(*) FROM pragma_foreign_key_check;

DROP TABLE migration_0014_foreign_key_guard;

DROP TABLE migration_0014_lifecycle_transactions;
DROP TABLE migration_0014_install_plan_candidates;
DROP TABLE migration_0014_install_plans;
DROP TABLE migration_0014_source_ref_change_plans;
DROP TABLE migration_0014_source_member_links;
DROP TABLE migration_0014_source_bundle_links;
DROP TABLE migration_0014_source_catalog_members;
DROP TABLE migration_0014_sources;
