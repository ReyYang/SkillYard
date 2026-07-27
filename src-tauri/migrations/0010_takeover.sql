-- Plan 只保存不可变合同；文件效果要等用户确认后才建立事务。
CREATE TABLE takeover_plans (
    id TEXT PRIMARY KEY NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= created_at)
);

CREATE INDEX takeover_plans_status_expiry
ON takeover_plans(status, expires_at);
