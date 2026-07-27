-- Stage 7 的关联确认只保存一份不可变 Plan；直接关联不创建文件系统事务。
CREATE TABLE source_association_plans (
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

CREATE INDEX source_association_plans_status_expiry
ON source_association_plans(status, expires_at);

-- “不对应”的既有成员没有 Source 路径；普通候选和新 Source 成员仍必须带路径。
CREATE TEMP TABLE migration_0015_install_plan_candidates AS
SELECT * FROM install_plan_candidates;

DROP TABLE install_plan_candidates;

CREATE TABLE install_plan_candidates (
    plan_id TEXT NOT NULL REFERENCES install_plans(id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    source_relative_path TEXT,
    skill_name TEXT,
    skill_description TEXT,
    content_fingerprint TEXT,
    selectable INTEGER NOT NULL CHECK (selectable IN (0, 1)),
    preserve_existing INTEGER NOT NULL CHECK (preserve_existing IN (0, 1)),
    validation_errors_json TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    default_selected INTEGER NOT NULL CHECK (default_selected IN (0, 1)),
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (plan_id, candidate_id),
    UNIQUE (plan_id, source_relative_path),
    UNIQUE (plan_id, sort_order),
    CHECK (source_relative_path IS NOT NULL OR preserve_existing = 1),
    CHECK (
        preserve_existing = 0
        OR (
            selectable = 0
            AND default_selected = 1
            AND selected = 1
            AND skill_name IS NOT NULL
            AND skill_description IS NOT NULL
            AND content_fingerprint IS NOT NULL
        )
    ),
    CHECK (selected = 0 OR selectable = 1 OR preserve_existing = 1),
    CHECK (default_selected = 0 OR selectable = 1 OR preserve_existing = 1)
);

INSERT INTO install_plan_candidates
SELECT * FROM migration_0015_install_plan_candidates;

DROP TABLE migration_0015_install_plan_candidates;

-- 迁移不得破坏已有 pending Plan 或 active lifecycle 的外键关系。
CREATE TEMP TABLE migration_0015_foreign_key_guard (
    issue_count INTEGER NOT NULL CHECK (issue_count = 0)
);

INSERT INTO migration_0015_foreign_key_guard (issue_count)
SELECT COUNT(*) FROM pragma_foreign_key_check;

DROP TABLE migration_0015_foreign_key_guard;
