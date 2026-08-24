-- Theme Preset 只是本地界面偏好，不参与 Bundle、Skill 或生命周期状态。
ALTER TABLE app_preferences
ADD COLUMN theme_preset TEXT NOT NULL DEFAULT 'ledger'
    CHECK (theme_preset IN ('layers', 'ledger'));
