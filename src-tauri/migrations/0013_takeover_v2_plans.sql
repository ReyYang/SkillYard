-- v2 Plan 与 v1 接管表并存；0010-0012 的事务与恢复契约保持原样。
CREATE TABLE takeover_v2_plans (
    id TEXT PRIMARY KEY,
    identity_basis TEXT NOT NULL
        CHECK (identity_basis IN ('single_origin', 'user_confirmed')),
    selected_origin_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    content_id TEXT NOT NULL,
    bundle_display_name TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    managed_directory TEXT NOT NULL,
    content_directory TEXT NOT NULL,
    expected_target TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed')),
    seal TEXT NOT NULL CHECK (
        length(seal) = 64 AND seal NOT GLOB '*[^0-9a-f]*'
    ),
    CHECK (expires_at > created_at),
    UNIQUE (bundle_id),
    UNIQUE (member_id),
    UNIQUE (content_id),
    FOREIGN KEY (id, selected_origin_id)
        REFERENCES takeover_v2_origins(plan_id, origin_id)
        DEFERRABLE INITIALLY DEFERRED
);

-- Origin 是执行前冻结的本机副本；shared 根不会被伪装成某个 Supported App。
CREATE TABLE takeover_v2_origins (
    plan_id TEXT NOT NULL REFERENCES takeover_v2_plans(id) ON DELETE CASCADE,
    origin_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    observation_skill_name TEXT NOT NULL,
    observation_declared_name TEXT,
    observation_skill_file TEXT NOT NULL,
    observation_location_kind TEXT NOT NULL CHECK (
        observation_location_kind IN ('app_global', 'app_project', 'shared_read_only')
    ),
    observation_metadata_status TEXT NOT NULL CHECK (observation_metadata_status = 'valid'),
    observation_observed_by_json TEXT NOT NULL,
    observation_fingerprint TEXT NOT NULL,
    observation_stale INTEGER NOT NULL CHECK (observation_stale = 0),
    observation_management_kind TEXT NOT NULL
        CHECK (observation_management_kind = 'takeover_candidate'),
    observation_management_evidence_empty INTEGER NOT NULL
        CHECK (observation_management_evidence_empty = 1),
    root_key TEXT NOT NULL CHECK (root_key IN (
        'codex_global', 'claude_code_global', 'github_copilot_global', 'shared_agents',
        'codex_project', 'claude_code_project', 'github_copilot_project',
        'shared_agents_project'
    )),
    app_id TEXT CHECK (app_id IS NULL OR app_id IN (
        'codex', 'claude_code', 'github_copilot'
    )),
    scope TEXT CHECK (scope IS NULL OR scope IN ('global', 'project')),
    project_id TEXT REFERENCES projects(id),
    project_display_name TEXT,
    project_root_path TEXT,
    project_root_device INTEGER CHECK (
        project_root_device IS NULL OR project_root_device >= 0
    ),
    project_root_inode INTEGER CHECK (
        project_root_inode IS NULL OR project_root_inode >= 0
    ),
    original_path TEXT NOT NULL,
    parent_device INTEGER NOT NULL CHECK (parent_device >= 0),
    parent_inode INTEGER NOT NULL CHECK (parent_inode >= 0),
    parent_mode INTEGER NOT NULL CHECK (parent_mode >= 0),
    original_device INTEGER NOT NULL CHECK (original_device >= 0),
    original_inode INTEGER NOT NULL CHECK (original_inode >= 0),
    original_mode INTEGER NOT NULL CHECK (original_mode >= 0),
    content_fingerprint TEXT NOT NULL CHECK (
        length(content_fingerprint) = 64
        AND content_fingerprint NOT GLOB '*[^0-9a-f]*'
    ),
    skill_description TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    final_disposition TEXT NOT NULL CHECK (final_disposition IN ('mount', 'remove')),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (plan_id, origin_id),
    UNIQUE (plan_id, observation_id),
    UNIQUE (plan_id, original_path),
    UNIQUE (plan_id, original_device, original_inode),
    UNIQUE (plan_id, sort_order),
    CHECK (
        (app_id IS NULL AND scope IS NULL)
        OR (app_id IS NOT NULL AND scope IS NOT NULL)
    ),
    CHECK (COALESCE((
        (app_id = 'codex' AND scope = 'global' AND root_key = 'codex_global'
            AND observation_location_kind = 'app_global'
            AND project_id IS NULL AND project_display_name IS NULL
            AND project_root_path IS NULL AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR
        (app_id = 'claude_code' AND scope = 'global' AND root_key = 'claude_code_global'
            AND observation_location_kind = 'app_global'
            AND project_id IS NULL AND project_display_name IS NULL
            AND project_root_path IS NULL AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR
        (app_id = 'github_copilot' AND scope = 'global'
            AND root_key = 'github_copilot_global'
            AND observation_location_kind = 'app_global'
            AND project_id IS NULL AND project_display_name IS NULL
            AND project_root_path IS NULL AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR
        (app_id = 'codex' AND scope = 'project' AND root_key = 'codex_project'
            AND observation_location_kind = 'app_project'
            AND project_id IS NOT NULL AND project_display_name IS NOT NULL
            AND project_root_path IS NOT NULL AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
        OR
        (app_id = 'claude_code' AND scope = 'project'
            AND root_key = 'claude_code_project'
            AND observation_location_kind = 'app_project'
            AND project_id IS NOT NULL AND project_display_name IS NOT NULL
            AND project_root_path IS NOT NULL AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
        OR
        (app_id = 'github_copilot' AND scope = 'project'
            AND root_key = 'github_copilot_project'
            AND observation_location_kind = 'app_project'
            AND project_id IS NOT NULL AND project_display_name IS NOT NULL
            AND project_root_path IS NOT NULL AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
        OR
        (app_id IS NULL AND scope IS NULL AND root_key = 'shared_agents'
            AND observation_location_kind = 'shared_read_only'
            AND project_id IS NULL AND project_display_name IS NULL
            AND project_root_path IS NULL AND project_root_device IS NULL
            AND project_root_inode IS NULL)
        OR
        (app_id IS NULL AND scope IS NULL AND root_key = 'shared_agents_project'
            AND observation_location_kind = 'shared_read_only'
            AND project_id IS NOT NULL AND project_display_name IS NOT NULL
            AND project_root_path IS NOT NULL AND project_root_device IS NOT NULL
            AND project_root_inode IS NOT NULL)
    ), 0)),
    CHECK (app_id IS NOT NULL OR final_disposition = 'remove')
);

-- Target 是最终 Mount 集合；occupied Origin 必须属于同一个原子 Plan。
CREATE TABLE takeover_v2_targets (
    plan_id TEXT NOT NULL REFERENCES takeover_v2_plans(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL,
    mount_id TEXT NOT NULL,
    app_id TEXT NOT NULL CHECK (app_id IN ('codex', 'claude_code', 'github_copilot')),
    scope TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project_id TEXT REFERENCES projects(id),
    project_display_name TEXT,
    project_root_path TEXT,
    project_root_device INTEGER CHECK (
        project_root_device IS NULL OR project_root_device >= 0
    ),
    project_root_inode INTEGER CHECK (
        project_root_inode IS NULL OR project_root_inode >= 0
    ),
    target_path TEXT NOT NULL,
    expected_target TEXT NOT NULL,
    parent_device INTEGER NOT NULL CHECK (parent_device >= 0),
    parent_inode INTEGER NOT NULL CHECK (parent_inode >= 0),
    parent_mode INTEGER NOT NULL CHECK (parent_mode >= 0),
    initial_state TEXT NOT NULL CHECK (initial_state IN ('absent', 'occupied_by_origin')),
    occupied_origin_id TEXT,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (plan_id, target_id),
    UNIQUE (plan_id, mount_id),
    UNIQUE (plan_id, target_path),
    UNIQUE (plan_id, sort_order),
    FOREIGN KEY (plan_id, occupied_origin_id)
        REFERENCES takeover_v2_origins(plan_id, origin_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (scope = 'global' AND project_id IS NULL
            AND project_display_name IS NULL AND project_root_path IS NULL
            AND project_root_device IS NULL AND project_root_inode IS NULL)
        OR
        (scope = 'project' AND project_id IS NOT NULL
            AND project_display_name IS NOT NULL AND project_root_path IS NOT NULL
            AND project_root_device IS NOT NULL AND project_root_inode IS NOT NULL)
    ),
    CHECK (
        (initial_state = 'absent' AND occupied_origin_id IS NULL)
        OR
        (initial_state = 'occupied_by_origin' AND occupied_origin_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX takeover_v2_target_unique_global_app
ON takeover_v2_targets(plan_id, app_id)
WHERE scope = 'global';

CREATE UNIQUE INDEX takeover_v2_target_unique_project_app
ON takeover_v2_targets(plan_id, app_id, project_id)
WHERE scope = 'project';

CREATE UNIQUE INDEX takeover_v2_target_unique_occupied_origin
ON takeover_v2_targets(plan_id, occupied_origin_id)
WHERE occupied_origin_id IS NOT NULL;

-- 已冻结的 Origin 占用同一路径时，Target 不能谎报为空。
CREATE TRIGGER takeover_v2_target_reject_absent_origin
BEFORE INSERT ON takeover_v2_targets
WHEN NEW.initial_state = 'absent' AND EXISTS (
    SELECT 1 FROM takeover_v2_origins
    WHERE plan_id = NEW.plan_id AND original_path = NEW.target_path
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_initial_state_mismatch');
END;

CREATE TRIGGER takeover_v2_target_reject_absent_origin_update
BEFORE UPDATE OF plan_id, initial_state, target_path ON takeover_v2_targets
WHEN NEW.initial_state = 'absent' AND EXISTS (
    SELECT 1 FROM takeover_v2_origins
    WHERE plan_id = NEW.plan_id AND original_path = NEW.target_path
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_initial_state_mismatch');
END;

CREATE TRIGGER takeover_v2_origin_reject_absent_target
BEFORE INSERT ON takeover_v2_origins
WHEN EXISTS (
    SELECT 1 FROM takeover_v2_targets
    WHERE plan_id = NEW.plan_id AND target_path = NEW.original_path
      AND initial_state = 'absent'
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_initial_state_mismatch');
END;

CREATE TRIGGER takeover_v2_origin_reject_absent_target_update
BEFORE UPDATE OF plan_id, original_path ON takeover_v2_origins
WHEN EXISTS (
    SELECT 1 FROM takeover_v2_targets
    WHERE plan_id = NEW.plan_id AND target_path = NEW.original_path
      AND initial_state = 'absent'
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_initial_state_mismatch');
END;

CREATE TRIGGER takeover_v2_target_validate_occupied_origin
BEFORE INSERT ON takeover_v2_targets
WHEN NEW.occupied_origin_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM takeover_v2_origins
    WHERE plan_id = NEW.plan_id
      AND origin_id = NEW.occupied_origin_id
      AND final_disposition = 'mount'
      AND original_path = NEW.target_path
      AND app_id = NEW.app_id
      AND scope = NEW.scope
      AND project_id IS NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_occupied_origin_mismatch');
END;

CREATE TRIGGER takeover_v2_target_validate_occupied_origin_update
BEFORE UPDATE OF plan_id, occupied_origin_id, target_path, app_id, scope, project_id
ON takeover_v2_targets
WHEN NEW.occupied_origin_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM takeover_v2_origins
    WHERE plan_id = NEW.plan_id
      AND origin_id = NEW.occupied_origin_id
      AND final_disposition = 'mount'
      AND original_path = NEW.target_path
      AND app_id = NEW.app_id
      AND scope = NEW.scope
      AND project_id IS NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_occupied_origin_mismatch');
END;

-- 一个 Skill Identity 在同一应用中不能同时产生 global 与 project Mount。
CREATE TRIGGER takeover_v2_target_reject_project_when_global_exists
BEFORE INSERT ON takeover_v2_targets
WHEN NEW.scope = 'project' AND EXISTS (
    SELECT 1 FROM takeover_v2_targets
    WHERE plan_id = NEW.plan_id AND app_id = NEW.app_id AND scope = 'global'
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_scope_conflict');
END;

CREATE TRIGGER takeover_v2_target_reject_global_when_project_exists
BEFORE INSERT ON takeover_v2_targets
WHEN NEW.scope = 'global' AND EXISTS (
    SELECT 1 FROM takeover_v2_targets
    WHERE plan_id = NEW.plan_id AND app_id = NEW.app_id AND scope = 'project'
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_scope_conflict');
END;

CREATE TRIGGER takeover_v2_target_reject_scope_change
BEFORE UPDATE OF app_id, scope, plan_id ON takeover_v2_targets
WHEN EXISTS (
    SELECT 1 FROM takeover_v2_targets
    WHERE plan_id = NEW.plan_id AND app_id = NEW.app_id
      AND NOT (plan_id = OLD.plan_id AND target_id = OLD.target_id)
      AND scope != NEW.scope
)
BEGIN
    SELECT RAISE(ABORT, 'takeover_v2_target_scope_conflict');
END;
