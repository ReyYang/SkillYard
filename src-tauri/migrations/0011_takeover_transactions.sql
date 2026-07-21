-- 接管拥有独立事务记录；Plan 的单次消费与文件系统 Journal 由这张表绑定。
CREATE TABLE takeover_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE REFERENCES takeover_plans(id),
    bundle_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    content_id TEXT NOT NULL,
    path_id TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    preserve_mount INTEGER NOT NULL CHECK (preserve_mount IN (0, 1)),
    phase TEXT NOT NULL CHECK (
        phase IN (
            'journal_pending', 'journal_ready', 'candidate_ready',
            'replacement_staged', 'host_swapped', 'state_committed',
            'original_discarded'
        )
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (plan_id, path_id)
        REFERENCES takeover_plan_paths(plan_id, path_id)
);

CREATE UNIQUE INDEX takeover_transaction_single_active
ON takeover_transactions ((1))
WHERE status = 'in_progress';

-- 四种高保证写事务共享 SQLite 单写者边界，不能只依赖单进程 Mutex。
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

CREATE TRIGGER takeover_transaction_reject_active_writer_update
BEFORE UPDATE OF status ON takeover_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND (
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

CREATE TRIGGER mount_transaction_reject_active_takeover
BEFORE INSERT ON mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_takeover
BEFORE INSERT ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

-- 终态不会被产品 API 重新激活；这些 UPDATE trigger 仍保护数据库自身的不变量。
CREATE TRIGGER install_transaction_reject_active_writer_update
BEFORE UPDATE OF status ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND (
    EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER mount_transaction_reject_active_writer_update
BEFORE UPDATE OF status ON mount_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_writer_update
BEFORE UPDATE OF status ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;
