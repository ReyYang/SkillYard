-- Project、Source 与 Bundle 共用一份不可变 Removal Plan。
CREATE TABLE removal_plans (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('project', 'source', 'bundle')),
    target_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at > created_at)
);

CREATE UNIQUE INDEX removal_plan_single_pending_target
ON removal_plans (kind, target_id)
WHERE status = 'pending';

CREATE INDEX removal_plans_status_expiry
ON removal_plans (status, expires_at);

-- Source 删除不伪造文件系统事务；只有 Project 与 Bundle 进入本表。
CREATE TABLE removal_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE REFERENCES removal_plans(id),
    kind TEXT NOT NULL CHECK (kind IN ('project', 'bundle')),
    target_id TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN (
            'journal_pending', 'journal_ready', 'mounts_isolated',
            'bundle_isolated', 'state_committed'
        )
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (kind = 'bundle' OR phase != 'bundle_isolated'),
    CHECK (status != 'completed' OR phase = 'state_committed')
);

CREATE UNIQUE INDEX removal_transaction_single_active
ON removal_transactions ((1))
WHERE status = 'in_progress';

CREATE INDEX removal_transaction_blocked_target
ON removal_transactions (kind, target_id)
WHERE status = 'blocked';

-- Removal 与现有五类事务共享产品级单写者边界。
CREATE TRIGGER removal_transaction_reject_active_writer
BEFORE INSERT ON removal_transactions
WHEN NEW.status = 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM source_association_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER install_transaction_reject_active_removal
BEFORE INSERT ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM removal_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER mount_transaction_reject_active_removal
BEFORE INSERT ON mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM removal_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_removal
BEFORE INSERT ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM removal_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER takeover_transaction_reject_active_removal
BEFORE INSERT ON takeover_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM removal_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER source_association_transaction_reject_active_removal
BEFORE INSERT ON source_association_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM removal_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;
