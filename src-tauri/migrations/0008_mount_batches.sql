-- Batch Mount Plan 固定 Bundle、成员、Project 与目标快照；冲突项只参与预览，不能进入事务。
CREATE TABLE batch_mount_plans (
    id TEXT PRIMARY KEY,
    bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    UNIQUE (id, bundle_id),
    CHECK (expires_at > created_at)
);

CREATE TABLE batch_mount_plan_items (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    mount_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    app_id TEXT NOT NULL CHECK (app_id IN ('codex', 'claude_code', 'github_copilot')),
    scope TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project_id TEXT REFERENCES projects(id),
    project_root_path TEXT,
    project_root_device INTEGER CHECK (project_root_device IS NULL OR project_root_device >= 0),
    project_root_inode INTEGER CHECK (project_root_inode IS NULL OR project_root_inode >= 0),
    target_path TEXT NOT NULL,
    expected_target TEXT NOT NULL,
    member_fingerprint TEXT NOT NULL,
    target_observation TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (
        disposition IN ('ready', 'path_conflict', 'scope_conflict', 'already_mounted')
    ),
    selectable INTEGER NOT NULL CHECK (selectable IN (0, 1)),
    default_selected INTEGER NOT NULL CHECK (default_selected IN (0, 1)),
    conflict_reason TEXT,
    target_health TEXT NOT NULL CHECK (target_health IN ('healthy', 'missing', 'conflict')),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    FOREIGN KEY (plan_id, bundle_id)
        REFERENCES batch_mount_plans(id, bundle_id) ON DELETE CASCADE,
    FOREIGN KEY (member_id, bundle_id)
        REFERENCES skill_members(id, bundle_id) ON DELETE CASCADE,
    UNIQUE (plan_id, id),
    UNIQUE (plan_id, mount_id),
    UNIQUE (plan_id, sort_order),
    CHECK (
        (scope = 'global' AND project_id IS NULL
            AND project_root_path IS NULL
            AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR (scope = 'project' AND project_id IS NOT NULL
            AND project_root_path IS NOT NULL
            AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
    ),
    -- 只有无冲突、尚未登记的 Mount 才允许用户选择。
    CHECK (
        (disposition = 'ready' AND selectable = 1 AND conflict_reason IS NULL)
        OR (disposition = 'already_mounted' AND selectable = 0 AND conflict_reason IS NULL)
        OR (disposition IN ('path_conflict', 'scope_conflict')
            AND selectable = 0 AND conflict_reason IS NOT NULL)
    ),
    CHECK (default_selected = 0 OR selectable = 1)
);

CREATE UNIQUE INDEX batch_mount_plan_unique_global_request
ON batch_mount_plan_items (plan_id, member_id, app_id)
WHERE scope = 'global';

CREATE UNIQUE INDEX batch_mount_plan_unique_project_request
ON batch_mount_plan_items (plan_id, member_id, app_id, project_id)
WHERE scope = 'project';

-- 文件系统逐项进度保存在 Journal；SQLite 只保存事务总阶段与冻结的确认集合。
CREATE TABLE batch_mount_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE,
    bundle_id TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN (
            'journal_pending', 'journal_ready', 'applying',
            'targets_applied', 'rolling_back', 'state_committed'
        )
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (plan_id, bundle_id)
        REFERENCES batch_mount_plans(id, bundle_id),
    UNIQUE (id, plan_id)
);

CREATE TABLE batch_mount_transaction_items (
    transaction_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (transaction_id, item_id),
    UNIQUE (transaction_id, sort_order),
    FOREIGN KEY (transaction_id, plan_id)
        REFERENCES batch_mount_transactions(id, plan_id) ON DELETE CASCADE,
    FOREIGN KEY (plan_id, item_id)
        REFERENCES batch_mount_plan_items(plan_id, id)
);

CREATE UNIQUE INDEX batch_mount_transaction_single_active
ON batch_mount_transactions ((1))
WHERE status = 'in_progress';

-- 三种会改写本地生命周期的事务共享同一个产品级单写者边界。
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

CREATE TRIGGER mount_transaction_reject_active_batch_mount
BEFORE INSERT ON mount_transactions
WHEN NEW.status = 'in_progress' AND EXISTS (
    SELECT 1 FROM batch_mount_transactions WHERE status = 'in_progress'
)
BEGIN
    SELECT RAISE(ABORT, 'active_lifecycle_transaction');
END;
