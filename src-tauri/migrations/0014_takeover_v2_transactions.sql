-- v2 接管把多个 Origin 与 Target 作为一个写事务；文件系统逐项进度由后续 Journal 保存。
CREATE TABLE takeover_v2_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE REFERENCES takeover_v2_plans(id),
    bundle_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    content_id TEXT NOT NULL,
    selected_origin_id TEXT NOT NULL,
    bundle_display_name TEXT NOT NULL CHECK (length(bundle_display_name) > 0),
    plan_seal TEXT NOT NULL CHECK (
        length(plan_seal) = 64
        AND plan_seal = lower(plan_seal)
        AND plan_seal NOT GLOB '*[^0-9a-f]*'
    ),
    journal_path TEXT NOT NULL UNIQUE,
    journal_contract_sha256 TEXT NOT NULL CHECK (
        length(journal_contract_sha256) = 64
        AND journal_contract_sha256 = lower(journal_contract_sha256)
        AND journal_contract_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    phase TEXT NOT NULL CHECK (
        phase IN (
            'journal_pending', 'preparing', 'prepared',
            'effect_started', 'state_committed', 'cleanup_completed'
        )
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (plan_id, selected_origin_id)
        REFERENCES takeover_v2_origins(plan_id, origin_id),
    CHECK (updated_at >= created_at),
    CHECK (
        (status = 'in_progress' AND phase <> 'cleanup_completed')
        OR (status = 'completed' AND phase = 'cleanup_completed')
        OR (status = 'aborted' AND phase IN (
            'journal_pending', 'preparing', 'prepared', 'effect_started'
        ))
        OR status = 'blocked'
    ),
    CHECK (
        (status = 'blocked' AND error_message IS NOT NULL AND length(trim(error_message)) > 0)
        OR (status = 'aborted')
        OR (status IN ('in_progress', 'completed') AND error_message IS NULL)
    )
);

CREATE UNIQUE INDEX takeover_v2_transaction_single_active
ON takeover_v2_transactions ((1))
WHERE status = 'in_progress';

-- 第五种高保证事务加入既有单写者边界；旧 trigger 保持字节级不变，只追加补充 trigger。
CREATE TRIGGER takeover_v2_transaction_reject_active_writer
BEFORE INSERT ON takeover_v2_transactions
WHEN NEW.status = 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER takeover_v2_transaction_reject_active_writer_update
BEFORE UPDATE OF status ON takeover_v2_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND (
    EXISTS (SELECT 1 FROM lifecycle_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress')
    OR EXISTS (SELECT 1 FROM takeover_transactions WHERE status = 'in_progress')
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER install_transaction_reject_active_takeover_v2
BEFORE INSERT ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER mount_transaction_reject_active_takeover_v2
BEFORE INSERT ON mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_takeover_v2
BEFORE INSERT ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER takeover_transaction_reject_active_takeover_v2
BEFORE INSERT ON takeover_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

-- 终态不会由产品正常重新激活，但数据库约束仍要覆盖直接 UPDATE。
CREATE TRIGGER install_transaction_reject_active_takeover_v2_update
BEFORE UPDATE OF status ON lifecycle_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER mount_transaction_reject_active_takeover_v2_update
BEFORE UPDATE OF status ON mount_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER batch_mount_transaction_reject_active_takeover_v2_update
BEFORE UPDATE OF status ON batch_mount_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;

CREATE TRIGGER takeover_transaction_reject_active_takeover_v2_update
BEFORE UPDATE OF status ON takeover_transactions
WHEN NEW.status = 'in_progress' AND OLD.status <> 'in_progress' AND EXISTS (
    SELECT 1 FROM takeover_v2_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;
