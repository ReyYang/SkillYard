-- 已核验的开发期数据库没有生命周期事务，因此不为旧事务保留第二套恢复协议。
CREATE TABLE migration_0013_lifecycle_empty_guard (
    transaction_count INTEGER NOT NULL CHECK (transaction_count = 0)
);

INSERT INTO migration_0013_lifecycle_empty_guard (transaction_count)
SELECT COUNT(*) FROM lifecycle_transactions;

DROP TABLE migration_0013_lifecycle_empty_guard;

-- 重建被这些 trigger 引用的表前，先移除全部跨事务单写者约束。
DROP TRIGGER mount_transaction_reject_active_install;
DROP TRIGGER install_transaction_reject_active_mount;
DROP TRIGGER batch_mount_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_batch_mount;
DROP TRIGGER takeover_transaction_reject_active_writer;
DROP TRIGGER install_transaction_reject_active_takeover;

-- 旧 pending Plan 尚未发布，可以直接丢弃并以唯一的安装协议重建。
DROP TABLE lifecycle_transactions;
DROP TABLE install_plan_candidates;
DROP TABLE install_plans;

CREATE TABLE install_plans (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('folder_snapshot', 'github_snapshot')),
    install_mode TEXT NOT NULL CHECK (install_mode IN ('create', 'supplement')),
    input_path TEXT,
    input_device INTEGER NOT NULL,
    input_inode INTEGER NOT NULL,
    input_fingerprint TEXT NOT NULL,
    snapshot_relative_path TEXT,
    source_id TEXT REFERENCES sources(id),
    source_tracked_ref TEXT,
    source_catalog_generation INTEGER CHECK (source_catalog_generation > 0),
    source_commit_sha TEXT,
    expected_current_target TEXT,
    expected_adopted_commit_sha TEXT,
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
            AND source_commit_sha IS NULL
            AND expected_current_target IS NULL
            AND expected_adopted_commit_sha IS NULL
        )
        OR
        (
            kind = 'github_snapshot'
            AND input_path IS NULL
            AND snapshot_relative_path IS NOT NULL
            AND source_id IS NOT NULL
            AND source_tracked_ref IS NOT NULL
            AND source_catalog_generation IS NOT NULL
            AND source_commit_sha IS NOT NULL
            AND (
                (
                    install_mode = 'create'
                    AND expected_current_target IS NULL
                    AND expected_adopted_commit_sha IS NULL
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

-- 安装、Mount、Batch Mount 与 Takeover 继续共享产品级单写者边界。
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
