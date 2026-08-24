PRAGMA foreign_keys = ON;

UPDATE app_state
SET initial_scan_completed_at = 1700000000000
WHERE singleton = 1;

INSERT INTO supported_app_status (app_id, display_name, detected, sort_order) VALUES
    ('codex', 'Codex', 1, 0),
    ('claude_code', 'Claude Code', 0, 1),
    ('github_copilot', 'GitHub Copilot', 0, 2);

INSERT INTO bundles (id, display_name, managed_directory, current_target, created_at)
VALUES (
    'bundle-v101',
    'Release Fixture Bundle',
    'bundles/bundle-v101',
    'contents/release-content',
    1700000000000
);

INSERT INTO skill_members (
    id, bundle_id, skill_name, description, stable_relative_path,
    content_fingerprint, created_at
)
VALUES (
    'member-release-fixture',
    'bundle-v101',
    'release-fixture',
    'Deterministic v1.0.1 release-SQL fixture',
    'members/release-fixture',
    '0f82cb05d9819c12d0704a21f0940e51a3f9f53864903a1dbb61ac02163c23fa',
    1700000000000
);

INSERT INTO member_selections (bundle_id, member_id, selected_at)
VALUES ('bundle-v101', 'member-release-fixture', 1700000000000);

-- 模拟用户已删除 release 默认推荐项，只保留本验收需要的一条 Source 真值。
DELETE FROM sources;

INSERT INTO sources (
    id, kind, canonical_identity, owner, repository, display_name, locator,
    tracked_ref, member_path_hint, filesystem_device, filesystem_inode,
    catalog_status, catalog_generation, catalog_marker, catalog_fetched_at,
    last_reload_at, last_reload_error, sort_order, created_at, updated_at
)
VALUES (
    'source-release-fixture',
    'github',
    'github:skillyard-fixture/release-fixture',
    'skillyard-fixture',
    'release-fixture',
    'skillyard-fixture/release-fixture',
    'https://github.com/skillyard-fixture/release-fixture',
    'main',
    'skills/release-fixture',
    NULL,
    NULL,
    'fresh',
    1,
    '1010101010101010101010101010101010101010',
    1700000000000,
    1700000000000,
    NULL,
    0,
    1700000000000,
    1700000000000
);

INSERT INTO source_catalog_members (
    id, source_id, catalog_generation, relative_path, skill_name, description,
    content_fingerprint, selectable, validation_errors_json, warnings_json, sort_order
)
VALUES (
    'catalog-member-release-fixture',
    'source-release-fixture',
    1,
    'skills/release-fixture',
    'release-fixture',
    'Deterministic v1.0.1 release-SQL fixture',
    '0f82cb05d9819c12d0704a21f0940e51a3f9f53864903a1dbb61ac02163c23fa',
    1,
    '[]',
    '[]',
    0
);

INSERT INTO source_bundle_links (source_id, bundle_id, adopted_marker, linked_at)
VALUES (
    'source-release-fixture',
    'bundle-v101',
    '1010101010101010101010101010101010101010',
    1700000000000
);

INSERT INTO source_member_links (source_id, source_relative_path, member_id, linked_at)
VALUES (
    'source-release-fixture',
    'skills/release-fixture',
    'member-release-fixture',
    1700000000000
);

INSERT INTO mounts (
    id, member_id, app_id, scope, project_id, target_path, expected_target,
    health, created_at, updated_at
)
VALUES (
    'mount-release-fixture',
    'member-release-fixture',
    'codex',
    'global',
    NULL,
    '/__skillyard_fixture__/home/.codex/skills/release-fixture',
    '/__skillyard_fixture__/data/bundles/bundle-v101/current/members/release-fixture',
    'healthy',
    1700000000000,
    1700000000000
);
