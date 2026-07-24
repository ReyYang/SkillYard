-- “全部更新”只保存顺序与每个普通 Install Plan 的结果，不建立跨 Bundle 事务或 Journal。
CREATE TABLE bundle_update_batches (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'completed', 'blocked')),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    confirmed_at INTEGER,
    updated_at INTEGER NOT NULL,
    CHECK (expires_at > created_at),
    CHECK (
        (status = 'pending' AND confirmed_at IS NULL)
        OR (status IN ('running', 'completed', 'blocked') AND confirmed_at IS NOT NULL)
    )
);

-- 协调记录在确认、放弃或确认结果前一直是唯一打开的批次。
CREATE UNIQUE INDEX bundle_update_batch_single_open
ON bundle_update_batches ((1));

CREATE TABLE bundle_update_batch_items (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES bundle_update_batches(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    install_plan_id TEXT REFERENCES install_plans(id) ON DELETE SET NULL,
    target_marker TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'ready',
            'preparation_failed',
            'succeeded',
            'failed',
            'blocked',
            'not_executed'
        )
    ),
    error TEXT,
    display_order INTEGER NOT NULL CHECK (display_order >= 0),
    confirmed_order INTEGER CHECK (confirmed_order >= 0),
    UNIQUE (batch_id, bundle_id),
    UNIQUE (batch_id, display_order),
    UNIQUE (batch_id, confirmed_order),
    CHECK (
        -- child 成功清理 Plan 后会先触发 ON DELETE SET NULL，再由协调器按 adopted marker 收敛结果。
        (status = 'ready' AND error IS NULL)
        OR (status = 'preparation_failed' AND install_plan_id IS NULL AND error IS NOT NULL)
        OR (
            status IN ('succeeded', 'not_executed')
            AND error IS NULL
        )
        OR (
            status IN ('failed', 'blocked')
            AND error IS NOT NULL
        )
    )
);
