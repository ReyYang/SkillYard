-- 1.1 发布前的三主题 schema 30，用于覆盖已有开发数据库的 forward migration。
ALTER TABLE app_preferences
ADD COLUMN theme_preset TEXT NOT NULL DEFAULT 'ledger'
    CHECK (theme_preset IN ('archive', 'layers', 'ledger'));

UPDATE app_preferences
SET interface_language = 'en', theme_preset = 'archive'
WHERE singleton_id = 1;
