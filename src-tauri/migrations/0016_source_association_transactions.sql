-- Bundle Merge 只使用这一张 canonical 事务表；逐路径进度由同一份 Journal 保存。
CREATE TABLE source_association_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE REFERENCES source_association_plans(id),
    source_id TEXT NOT NULL,
    target_bundle_id TEXT NOT NULL,
    retiring_bundle_id TEXT NOT NULL,
    -- 用户确认的内容选择必须与被消费 Plan 一起原子封存。
    content_choices_json TEXT NOT NULL,
    -- Journal 将要提交的最终 Source mapping 必须在文件系统写入前封存。
    source_mappings_json TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN (
            'journal_pending', 'journal_ready', 'candidate_ready',
            'current_activated', 'mounts_applied', 'state_committed'
        )
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (source_id <> ''),
    CHECK (target_bundle_id <> retiring_bundle_id),
    CHECK (status != 'completed' OR phase = 'state_committed'),
    CHECK (phase != 'state_committed' OR status IN ('completed', 'blocked'))
);

CREATE UNIQUE INDEX source_association_transaction_single_active
ON source_association_transactions ((1))
WHERE status = 'in_progress';

CREATE INDEX source_association_transaction_blocked_objects
ON source_association_transactions (source_id, target_bundle_id, retiring_bundle_id)
WHERE status = 'blocked';

-- Merge 与安装、单 Mount、Batch Mount、Takeover 共用产品级单写者边界。
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

CREATE TRIGGER mount_transaction_reject_active_source_association
BEFORE INSERT ON mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM source_association_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_source_association
BEFORE INSERT ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM source_association_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER takeover_transaction_reject_active_source_association
BEFORE INSERT ON takeover_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM source_association_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;
