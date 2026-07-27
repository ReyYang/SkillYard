CREATE TABLE install_plans (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind = 'folder_snapshot'),
    input_path TEXT NOT NULL,
    input_device INTEGER NOT NULL,
    input_inode INTEGER NOT NULL,
    input_fingerprint TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    bundle_display_name TEXT NOT NULL,
    member_id TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    skill_description TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed'))
);

CREATE TABLE lifecycle_transactions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind = 'install_folder'),
    plan_id TEXT NOT NULL UNIQUE REFERENCES install_plans(id),
    bundle_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    journal_path TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN ('journal_pending', 'journal_ready', 'candidate_ready', 'activated', 'state_committed')
    ),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted', 'blocked')),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 即使出现第二个应用实例，SQLite 也只允许一个生命周期写事务。
CREATE UNIQUE INDEX lifecycle_single_active
ON lifecycle_transactions ((1))
WHERE status = 'in_progress';

CREATE TABLE bundles (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    managed_directory TEXT NOT NULL UNIQUE,
    current_target TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE skill_members (
    id TEXT PRIMARY KEY,
    bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
    skill_name TEXT NOT NULL,
    description TEXT NOT NULL,
    stable_relative_path TEXT NOT NULL,
    content_fingerprint TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (id, bundle_id),
    UNIQUE (bundle_id, skill_name),
    UNIQUE (bundle_id, stable_relative_path)
);

CREATE TABLE member_selections (
    bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
    member_id TEXT NOT NULL,
    selected_at INTEGER NOT NULL,
    PRIMARY KEY (bundle_id, member_id),
    FOREIGN KEY (member_id, bundle_id)
        REFERENCES skill_members(id, bundle_id) ON DELETE CASCADE
);
