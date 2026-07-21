-- 修复 Plan 必须保留独立意图，防止已被正式移除的 Mount 被旧 Plan 重新创建。
ALTER TABLE mount_plans
ADD COLUMN purpose TEXT NOT NULL DEFAULT 'create'
CHECK (purpose IN ('create', 'repair', 'remove'));

-- 迁移现有移除 Plan；旧创建 Plan 继续使用默认的 create。
UPDATE mount_plans
SET purpose = 'remove'
WHERE operation = 'remove';
