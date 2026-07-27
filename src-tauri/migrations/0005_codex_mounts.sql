-- Project 只保存用户明确登记且已经 canonicalize 的目录身份。
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    root_device INTEGER NOT NULL CHECK (root_device >= 0),
    root_inode INTEGER NOT NULL CHECK (root_inode >= 0),
    created_at INTEGER NOT NULL,
    UNIQUE (root_device, root_inode)
);

-- Mount Plan 同时绑定受管成员、Project 身份和目标路径的只读前置快照。
CREATE TABLE mount_plans (
    id TEXT PRIMARY KEY,
    operation TEXT NOT NULL CHECK (operation IN ('create', 'remove')),
    mount_id TEXT NOT NULL,
    member_id TEXT NOT NULL REFERENCES skill_members(id),
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
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    CHECK (
        (scope = 'global' AND project_id IS NULL
            AND project_root_path IS NULL
            AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR (scope = 'project' AND project_id IS NOT NULL
            AND project_root_path IS NOT NULL
            AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
    )
);

-- Mount 是受管成员在一个 Supported App 使用位置中的唯一事实记录。
CREATE TABLE mounts (
    id TEXT PRIMARY KEY,
    member_id TEXT NOT NULL REFERENCES skill_members(id),
    app_id TEXT NOT NULL CHECK (app_id IN ('codex', 'claude_code', 'github_copilot')),
    scope TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project_id TEXT REFERENCES projects(id),
    target_path TEXT NOT NULL UNIQUE,
    expected_target TEXT NOT NULL,
    health TEXT NOT NULL CHECK (health IN ('healthy', 'missing', 'conflict')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (scope = 'global' AND project_id IS NULL)
        OR (scope = 'project' AND project_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX mount_unique_global_member_app
ON mounts (member_id, app_id)
WHERE scope = 'global';

CREATE UNIQUE INDEX mount_unique_project_member_app
ON mounts (member_id, app_id, project_id)
WHERE scope = 'project';

-- 一个成员在同一应用中不能同时使用 global 和 project scope。
CREATE TRIGGER mount_reject_project_when_global_exists
BEFORE INSERT ON mounts
WHEN NEW.scope = 'project' AND EXISTS (
    SELECT 1 FROM mounts
    WHERE member_id = NEW.member_id
      AND app_id = NEW.app_id
      AND scope = 'global'
)
BEGIN
    SELECT RAISE(ABORT, 'mount_scope_conflict');
END;

CREATE TRIGGER mount_reject_global_when_project_exists
BEFORE INSERT ON mounts
WHEN NEW.scope = 'global' AND EXISTS (
    SELECT 1 FROM mounts
    WHERE member_id = NEW.member_id
      AND app_id = NEW.app_id
      AND scope = 'project'
)
BEGIN
    SELECT RAISE(ABORT, 'mount_scope_conflict');
END;

-- Mount 拥有独立 Journal 和可重放阶段，不复用 Bundle 安装事务。
CREATE TABLE mount_transactions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE REFERENCES mount_plans(id),
    mount_id TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN ('journal_pending', 'journal_ready', 'target_applied', 'state_committed')
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX mount_transaction_single_active
ON mount_transactions ((1))
WHERE status = 'in_progress';

-- 安装和 Mount 共用产品级单写者边界，不能各自同时修改文件系统。
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
