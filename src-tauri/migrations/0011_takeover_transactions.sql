-- Takeover 使用唯一的 canonical 事务表；逐路径进度由同一份 Filesystem Journal 保存。
CREATE TABLE takeover_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE REFERENCES takeover_plans(id),
    -- 事务在领域行创建前也必须能标识受影响对象，供 blocked 写入隔离使用。
    bundle_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    -- 路径按绝对路径排序去重后保存；用于领域 Member 尚未创建时继续隔离写入。
    reserved_paths_json TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN (
            'journal_pending', 'journal_ready', 'candidate_ready',
            'current_activated', 'origins_applied', 'state_committed'
        )
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- completed 只表示领域状态已经提交；提交后的人工恢复仍可标记为 blocked。
    CHECK (status != 'completed' OR phase = 'state_committed'),
    CHECK (phase != 'state_committed' OR status IN ('completed', 'blocked'))
);

CREATE UNIQUE INDEX takeover_transaction_single_active
ON takeover_transactions ((1))
WHERE status = 'in_progress';

CREATE INDEX takeover_transaction_blocked_member
ON takeover_transactions (member_id)
WHERE status = 'blocked';

-- Takeover 与安装、单 Mount、Batch Mount 共用同一个产品级单写者边界。
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
