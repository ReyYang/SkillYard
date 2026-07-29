-- 每个稳定 Inventory 身份只保留一份完成结果；重新整理使用原键覆盖。
CREATE TABLE skill_ai_explanations (
    inventory_id TEXT PRIMARY KEY CHECK (length(inventory_id) > 0),
    language TEXT NOT NULL CHECK (language IN ('zh_cn', 'en')),
    content_fingerprint TEXT NOT NULL CHECK (length(content_fingerprint) > 0),
    category TEXT NOT NULL CHECK (
        category IN (
            'development_engineering',
            'system_operations',
            'productivity_automation',
            'data_analytics',
            'product_business',
            'research_learning',
            'writing_communication',
            'design_creative',
            'security_compliance',
            'other'
        )
    ),
    summary TEXT NOT NULL CHECK (length(summary) > 0),
    use_cases_json TEXT NOT NULL CHECK (json_valid(use_cases_json)),
    instructions TEXT NOT NULL CHECK (length(instructions) > 0)
);
