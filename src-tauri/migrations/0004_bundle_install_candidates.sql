CREATE TABLE install_plan_candidates (
    plan_id TEXT NOT NULL REFERENCES install_plans(id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    source_relative_path TEXT NOT NULL,
    skill_name TEXT,
    skill_description TEXT,
    content_fingerprint TEXT,
    selectable INTEGER NOT NULL CHECK (selectable IN (0, 1)),
    validation_errors_json TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    default_selected INTEGER NOT NULL CHECK (default_selected IN (0, 1)),
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (plan_id, candidate_id),
    UNIQUE (plan_id, source_relative_path),
    UNIQUE (plan_id, sort_order)
);

-- 已由 0003 签发的单成员 Plan 仍可在升级后确认和恢复。
INSERT INTO install_plan_candidates (
    plan_id,
    candidate_id,
    source_relative_path,
    skill_name,
    skill_description,
    content_fingerprint,
    selectable,
    validation_errors_json,
    warnings_json,
    default_selected,
    selected,
    sort_order
)
SELECT
    id,
    member_id,
    '',
    skill_name,
    skill_description,
    input_fingerprint,
    1,
    '[]',
    warnings_json,
    1,
    1,
    0
FROM install_plans;
