-- Update Check 只保存上游查询结果，不创建文件系统事务或候选内容。
ALTER TABLE source_bundle_links
ADD COLUMN update_check_status TEXT NOT NULL DEFAULT 'not_checked'
    CHECK (
        update_check_status IN (
            'not_checked',
            'available',
            'up_to_date',
            'unable_to_check',
            'source_unavailable'
        )
    );

ALTER TABLE source_bundle_links
ADD COLUMN update_checked_marker TEXT;

ALTER TABLE source_bundle_links
ADD COLUMN update_checked_at INTEGER;

ALTER TABLE source_bundle_links
ADD COLUMN update_check_error TEXT;
