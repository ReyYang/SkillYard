-- Journal 中包含原目录逐项删除授权；SQLite seal 防止只篡改 Journal 就扩大删除范围。
ALTER TABLE takeover_transactions
ADD COLUMN journal_contract_sha256 TEXT
CHECK (
    journal_contract_sha256 IS NULL
    OR (
        length(journal_contract_sha256) = 64
        AND journal_contract_sha256 = lower(journal_contract_sha256)
        AND journal_contract_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
