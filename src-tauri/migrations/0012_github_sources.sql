-- GitHub Source 是可独立存在的远端来源；Catalog 只保存最近一次成功发现的成员 metadata。
CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind = 'github'),
    canonical_identity TEXT NOT NULL UNIQUE,
    owner TEXT NOT NULL,
    repository TEXT NOT NULL,
    display_name TEXT NOT NULL,
    repository_url TEXT NOT NULL,
    tracked_ref TEXT NOT NULL,
    member_path_hint TEXT,
    catalog_status TEXT NOT NULL DEFAULT 'unloaded'
        CHECK (catalog_status IN ('unloaded', 'fresh', 'stale')),
    catalog_generation INTEGER NOT NULL DEFAULT 0 CHECK (catalog_generation >= 0),
    catalog_commit_sha TEXT,
    catalog_fetched_at INTEGER,
    last_reload_at INTEGER,
    last_reload_error TEXT,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (canonical_identity = lower(canonical_identity)),
    CHECK (
        (catalog_status = 'unloaded'
            AND catalog_generation = 0
            AND catalog_commit_sha IS NULL
            AND catalog_fetched_at IS NULL)
        OR (catalog_status = 'fresh'
            AND catalog_generation > 0
            AND catalog_commit_sha IS NOT NULL
            AND catalog_fetched_at IS NOT NULL
            AND last_reload_error IS NULL)
        OR (catalog_status = 'stale'
            AND catalog_generation > 0
            AND catalog_commit_sha IS NOT NULL
            AND catalog_fetched_at IS NOT NULL
            AND last_reload_error IS NOT NULL)
    )
);

CREATE UNIQUE INDEX sources_sort_order ON sources(sort_order);

CREATE TABLE source_catalog_members (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    catalog_generation INTEGER NOT NULL CHECK (catalog_generation > 0),
    relative_path TEXT NOT NULL,
    skill_name TEXT,
    description TEXT,
    content_fingerprint TEXT,
    selectable INTEGER NOT NULL CHECK (selectable IN (0, 1)),
    validation_errors_json TEXT NOT NULL,
    warnings_json TEXT NOT NULL,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    UNIQUE (source_id, relative_path),
    UNIQUE (source_id, sort_order)
);

-- Source 删除只删除关联；Bundle 与其当前受管内容继续存在。
CREATE TABLE source_bundle_links (
    source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    bundle_id TEXT NOT NULL UNIQUE REFERENCES bundles(id) ON DELETE CASCADE,
    adopted_commit_sha TEXT,
    linked_at INTEGER NOT NULL
);

CREATE TABLE source_member_links (
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    source_relative_path TEXT NOT NULL,
    member_id TEXT NOT NULL UNIQUE REFERENCES skill_members(id) ON DELETE CASCADE,
    linked_at INTEGER NOT NULL,
    PRIMARY KEY (source_id, source_relative_path)
);

-- Tracked Ref 变更只修改 Source metadata，但仍由有时效的确认 Plan 绑定旧状态。
CREATE TABLE source_ref_change_plans (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    current_ref TEXT NOT NULL,
    candidate_ref TEXT NOT NULL,
    candidate_commit_sha TEXT NOT NULL,
    member_path_hint TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK (expires_at > created_at),
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed'))
);

-- 推荐来源只在 migration 首次执行时写入；后续启动不会让用户已经删除的记录复活。
INSERT INTO sources (
    id, kind, canonical_identity, owner, repository, display_name,
    repository_url, tracked_ref, sort_order, created_at, updated_at
) VALUES
    ('source-anthropics-skills', 'github', 'github:anthropics/skills',
     'anthropics', 'skills', 'anthropics/skills',
     'https://github.com/anthropics/skills', 'main', 0, unixepoch() * 1000, unixepoch() * 1000),
    ('source-composiohq-awesome-claude-skills', 'github',
     'github:composiohq/awesome-claude-skills', 'ComposioHQ', 'awesome-claude-skills',
     'ComposioHQ/awesome-claude-skills',
     'https://github.com/ComposioHQ/awesome-claude-skills', 'master', 1,
     unixepoch() * 1000, unixepoch() * 1000),
    ('source-cexll-myclaude', 'github', 'github:cexll/myclaude',
     'cexll', 'myclaude', 'cexll/myclaude',
     'https://github.com/cexll/myclaude', 'master', 2,
     unixepoch() * 1000, unixepoch() * 1000),
    ('source-jimliu-baoyu-skills', 'github', 'github:jimliu/baoyu-skills',
     'JimLiu', 'baoyu-skills', 'JimLiu/baoyu-skills',
     'https://github.com/JimLiu/baoyu-skills', 'main', 3,
     unixepoch() * 1000, unixepoch() * 1000);
