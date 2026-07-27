-- 已完成接管的旧 Bundle 可能只保存了 lock 安装履历，却没有建立 Source 关系。
-- 仅修复全部成员都指向同一个标准 GitHub owner/repository 的确定状态。
CREATE TEMP TABLE migration_0024_takeover_sources AS
SELECT
    bundle.id AS bundle_id,
    MIN(trim(chain.source)) AS display_name,
    lower(trim(chain.source)) AS repository_identity,
    substr(
        MIN(trim(chain.source)),
        1,
        instr(MIN(trim(chain.source)), '/') - 1
    ) AS owner,
    substr(
        MIN(trim(chain.source)),
        instr(MIN(trim(chain.source)), '/') + 1
    ) AS repository,
    COALESCE(MAX(NULLIF(trim(chain.tracked_ref), '')), 'HEAD') AS tracked_ref
FROM bundles AS bundle
JOIN skill_members AS member ON member.bundle_id = bundle.id
JOIN member_installation_chains AS chain ON chain.member_id = member.id
WHERE NOT EXISTS (
    SELECT 1 FROM source_bundle_links AS link WHERE link.bundle_id = bundle.id
)
GROUP BY bundle.id
HAVING COUNT(*) = (
    SELECT COUNT(*) FROM skill_members AS all_member
    WHERE all_member.bundle_id = bundle.id
)
AND COUNT(DISTINCT lower(trim(chain.source))) = 1
AND COUNT(DISTINCT lower(trim(chain.source_locator))) = 1
AND COUNT(DISTINCT NULLIF(trim(chain.tracked_ref), '')) <= 1
AND MIN(chain.kind) = 'lock_v3'
AND MAX(chain.kind) = 'lock_v3'
AND MIN(lower(chain.source_type)) = 'github'
AND MAX(lower(chain.source_type)) = 'github'
AND instr(MIN(trim(chain.source)), '/') > 1
AND instr(
    substr(
        MIN(trim(chain.source)),
        instr(MIN(trim(chain.source)), '/') + 1
    ),
    '/'
) = 0
AND substr(MIN(trim(chain.source)), -1) <> '/'
AND lower(MIN(trim(chain.source_locator))) IN (
    'https://github.com/' || lower(MIN(trim(chain.source))),
    'https://github.com/' || lower(MIN(trim(chain.source))) || '.git'
);

INSERT INTO sources (
    id, kind, canonical_identity, owner, repository,
    display_name, locator, tracked_ref, member_path_hint,
    sort_order, created_at, updated_at
)
SELECT
    'source-takeover-' || lower(hex(randomblob(16))),
    'github',
    'github:' || candidate.repository_identity,
    candidate.owner,
    candidate.repository,
    candidate.display_name,
    'https://github.com/' || candidate.display_name,
    candidate.tracked_ref,
    NULL,
    (
        SELECT COALESCE(MAX(source.sort_order), -1) FROM sources AS source
    ) + ROW_NUMBER() OVER (ORDER BY candidate.repository_identity),
    unixepoch() * 1000,
    unixepoch() * 1000
FROM migration_0024_takeover_sources AS candidate
WHERE NOT EXISTS (
    SELECT 1 FROM sources AS source
    WHERE source.canonical_identity = 'github:' || candidate.repository_identity
)
AND candidate.bundle_id = (
    SELECT MIN(same_source.bundle_id)
    FROM migration_0024_takeover_sources AS same_source
    WHERE same_source.repository_identity = candidate.repository_identity
);

INSERT INTO source_bundle_links (
    source_id, bundle_id, adopted_marker, linked_at
)
SELECT
    source.id,
    candidate.bundle_id,
    NULL,
    unixepoch() * 1000
FROM migration_0024_takeover_sources AS candidate
JOIN sources AS source
  ON source.canonical_identity = 'github:' || candidate.repository_identity
WHERE source.kind = 'github'
AND NOT EXISTS (
    SELECT 1 FROM source_bundle_links AS link
    WHERE link.source_id = source.id OR link.bundle_id = candidate.bundle_id
)
AND candidate.bundle_id = (
    SELECT MIN(same_source.bundle_id)
    FROM migration_0024_takeover_sources AS same_source
    WHERE same_source.repository_identity = candidate.repository_identity
);

INSERT INTO source_member_links (
    source_id, source_relative_path, member_id, linked_at
)
SELECT
    link.source_id,
    CASE
        WHEN chain.skill_path = 'SKILL.md' THEN ''
        ELSE substr(chain.skill_path, 1, length(chain.skill_path) - length('/SKILL.md'))
    END,
    member.id,
    unixepoch() * 1000
FROM migration_0024_takeover_sources AS candidate
JOIN source_bundle_links AS link ON link.bundle_id = candidate.bundle_id
JOIN skill_members AS member ON member.bundle_id = candidate.bundle_id
JOIN member_installation_chains AS chain ON chain.member_id = member.id
WHERE chain.skill_path = 'SKILL.md'
   OR (
       chain.skill_path LIKE '%/SKILL.md'
       AND chain.skill_path NOT LIKE '/%'
       AND chain.skill_path NOT LIKE '../%'
       AND chain.skill_path NOT LIKE '%/../%'
       AND chain.skill_path NOT LIKE './%'
       AND chain.skill_path NOT LIKE '%/./%'
   );

DROP TABLE migration_0024_takeover_sources;
