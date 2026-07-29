-- 界面语言是本地非敏感偏好；固定单行避免产生多份互相冲突的设置。
CREATE TABLE app_preferences (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    interface_language TEXT NOT NULL
        CHECK (interface_language IN ('zh_cn', 'en'))
);

INSERT INTO app_preferences (singleton_id, interface_language)
VALUES (1, 'zh_cn');
