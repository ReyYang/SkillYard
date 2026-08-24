-- Archive 未进入 1.1；已有开发数据库必须与 fresh schema 收敛到同一个两主题约束。
ALTER TABLE app_preferences RENAME TO app_preferences_schema_30;

CREATE TABLE app_preferences (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    interface_language TEXT NOT NULL
        CHECK (interface_language IN ('zh_cn', 'en')),
    theme_preset TEXT NOT NULL DEFAULT 'ledger'
        CHECK (theme_preset IN ('layers', 'ledger'))
);

INSERT INTO app_preferences (singleton_id, interface_language, theme_preset)
SELECT
    singleton_id,
    interface_language,
    CASE theme_preset WHEN 'archive' THEN 'ledger' ELSE theme_preset END
FROM app_preferences_schema_30;

DROP TABLE app_preferences_schema_30;
