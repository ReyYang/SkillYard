-- 早期接管可能先用首个 Skill 名创建 Bundle，随后才补入同一 lock 来源的其他成员。
-- 仅当无已登记 Source、全部成员都有同一 GitHub lock 来源时，才修正这个已核验的旧状态。
UPDATE bundles AS bundle
SET display_name = (
    SELECT MIN(chain.source)
    FROM skill_members AS member
    JOIN member_installation_chains AS chain ON chain.member_id = member.id
    WHERE member.bundle_id = bundle.id
)
WHERE NOT EXISTS (
    SELECT 1
    FROM source_bundle_links AS source_link
    WHERE source_link.bundle_id = bundle.id
)
AND (
    SELECT COUNT(*)
    FROM skill_members AS member
    WHERE member.bundle_id = bundle.id
) = (
    SELECT COUNT(*)
    FROM skill_members AS member
    JOIN member_installation_chains AS chain ON chain.member_id = member.id
    WHERE member.bundle_id = bundle.id
      AND chain.kind = 'lock_v3'
      AND lower(chain.source_type) = 'github'
      AND trim(chain.source) <> ''
)
AND 1 = (
    SELECT COUNT(DISTINCT lower(chain.source_locator))
    FROM skill_members AS member
    JOIN member_installation_chains AS chain ON chain.member_id = member.id
    WHERE member.bundle_id = bundle.id
)
AND 1 = (
    SELECT COUNT(DISTINCT lower(trim(chain.source)))
    FROM skill_members AS member
    JOIN member_installation_chains AS chain ON chain.member_id = member.id
    WHERE member.bundle_id = bundle.id
);
