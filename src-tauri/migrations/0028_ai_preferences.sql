-- API Key 只保存在 macOS Keychain；SQLite 仅保存非敏感全局选择与验证状态。
CREATE TABLE ai_preferences (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    disclosure_accepted INTEGER NOT NULL DEFAULT 0
        CHECK (disclosure_accepted IN (0, 1)),
    provider TEXT NOT NULL
        CHECK (provider IN ('openai', 'glm', 'deepseek')),
    model TEXT NOT NULL CHECK (length(model) > 0),
    verified INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1))
);

INSERT INTO ai_preferences (
    singleton_id,
    enabled,
    disclosure_accepted,
    provider,
    model,
    verified
)
VALUES (1, 0, 0, 'openai', 'gpt-5.6-terra', 0);
