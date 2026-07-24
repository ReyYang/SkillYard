-- Bundle Update 原地扩展唯一 Install Plan 与 lifecycle transaction 协议。
CREATE TEMP TABLE migration_0018_install_plans AS SELECT * FROM install_plans;
CREATE TEMP TABLE migration_0018_install_plan_candidates AS SELECT * FROM install_plan_candidates;
CREATE TEMP TABLE migration_0018_lifecycle_transactions AS SELECT * FROM lifecycle_transactions;

-- 重建生命周期表前必须移除所有引用它的单写者 trigger。
DROP TRIGGER mount_transaction_reject_active_install;
DROP TRIGGER install_transaction_reject_active_mount;
DROP TRIGGER batch_mount_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_batch_mount;
DROP TRIGGER takeover_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_takeover;
DROP TRIGGER source_association_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_source_association;

DROP TABLE lifecycle_transactions;
DROP TABLE install_plan_candidates;
DROP TABLE install_plans;

CREATE TABLE install_plans (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('folder_snapshot', 'source_snapshot')),
    install_mode TEXT NOT NULL CHECK (install_mode IN ('create', 'supplement', 'update')),
    input_path TEXT,
    input_device INTEGER NOT NULL,
    input_inode INTEGER NOT NULL,
    input_fingerprint TEXT NOT NULL,
    snapshot_relative_path TEXT,
    source_id TEXT REFERENCES sources(id),
    source_tracked_ref TEXT,
    source_catalog_generation INTEGER CHECK (source_catalog_generation > 0),
    -- source_marker 是本次候选内容的标识；Update 确认成功后才采用。
    source_marker TEXT,
    -- Update 创建 Plan 时的 Catalog 标识，用于阻止确认覆盖后来刷新出的状态。
    expected_source_marker TEXT,
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
            AND expected_source_marker IS NULL
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
                    AND expected_source_marker IS NULL
                    AND expected_current_target IS NULL
                    AND expected_adopted_marker IS NULL
                )
                OR
                (
                    install_mode = 'supplement'
                    AND expected_source_marker IS NULL
                    AND expected_current_target IS NOT NULL
                )
                OR
                (
                    install_mode = 'update'
                    AND expected_source_marker IS NOT NULL
                    AND expected_current_target IS NOT NULL
                )
            )
        )
    )
);

CREATE TABLE install_plan_candidates (
    plan_id TEXT NOT NULL REFERENCES install_plans(id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    source_relative_path TEXT,
    skill_name TEXT,
    skill_description TEXT,
    content_fingerprint TEXT,
    -- 已安装成员的旧 fingerprint 只用于确认 current 在生效前没有变化。
    previous_content_fingerprint TEXT,
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
    CHECK (source_relative_path IS NOT NULL OR preserve_existing = 1),
    CHECK (
        preserve_existing = 0
        OR (
            selectable = 0
            AND default_selected = 1
            AND selected = 1
            AND skill_name IS NOT NULL
            AND skill_description IS NOT NULL
            AND content_fingerprint IS NOT NULL
            AND previous_content_fingerprint IS NOT NULL
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

INSERT INTO install_plans (
    id, kind, install_mode, input_path, input_device, input_inode,
    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
    source_catalog_generation, source_marker, expected_source_marker,
    expected_current_target, expected_adopted_marker, bundle_id,
    bundle_display_name, warnings_json, created_at, expires_at, status
)
SELECT
    id, kind, install_mode, input_path, input_device, input_inode,
    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
    source_catalog_generation, source_marker, NULL,
    expected_current_target, expected_adopted_marker, bundle_id,
    bundle_display_name, warnings_json, created_at, expires_at, status
FROM migration_0018_install_plans;

INSERT INTO install_plan_candidates (
    plan_id, candidate_id, source_relative_path, skill_name, skill_description,
    content_fingerprint, previous_content_fingerprint, selectable,
    preserve_existing, validation_errors_json, warnings_json, default_selected,
    selected, sort_order
)
SELECT
    plan_id, candidate_id, source_relative_path, skill_name, skill_description,
    content_fingerprint,
    CASE WHEN preserve_existing = 1 THEN content_fingerprint ELSE NULL END,
    selectable, preserve_existing, validation_errors_json, warnings_json,
    default_selected, selected, sort_order
FROM migration_0018_install_plan_candidates;

INSERT INTO lifecycle_transactions SELECT * FROM migration_0018_lifecycle_transactions;

CREATE UNIQUE INDEX lifecycle_single_active
ON lifecycle_transactions ((1))
WHERE status = 'in_progress';

-- 安装、Mount、Batch Mount、Takeover 与 Source Association 继续共用单写者边界。
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

CREATE TRIGGER source_association_transaction_reject_active_writer
BEFORE INSERT ON source_association_transactions
WHEN NEW.status = 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER install_transaction_reject_active_source_association
BEFORE INSERT ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM source_association_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

-- 任一 pending、blocked 或 active 行若无法映射，migration 必须整体回滚。
CREATE TEMP TABLE migration_0018_foreign_key_guard (
    issue_count INTEGER NOT NULL CHECK (issue_count = 0)
);

INSERT INTO migration_0018_foreign_key_guard (issue_count)
SELECT COUNT(*) FROM pragma_foreign_key_check;

DROP TABLE migration_0018_foreign_key_guard;
DROP TABLE migration_0018_lifecycle_transactions;
DROP TABLE migration_0018_install_plan_candidates;
DROP TABLE migration_0018_install_plans;
