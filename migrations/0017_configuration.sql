CREATE TABLE configuration_operations (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    target TEXT NOT NULL,
    preview_json TEXT NOT NULL CHECK (length(CAST(preview_json AS BLOB)) <= 65536),
    desired_json TEXT NOT NULL CHECK (length(CAST(desired_json AS BLOB)) <= 65536),
    observed_json TEXT NOT NULL CHECK (length(CAST(observed_json AS BLOB)) <= 65536),
    original_hash TEXT NOT NULL,
    candidate_hash TEXT NOT NULL,
    preview_revision TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('previewed','apply-intent','prepared','applied','recovery-needed','rolled-back','drift-refused','expired')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    backup_name TEXT,
    backup_hash TEXT,
    receipt_json TEXT CHECK (receipt_json IS NULL OR length(CAST(receipt_json AS BLOB)) <= 16384),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX configuration_operations_state_expiry ON configuration_operations(state, expires_at, created_at);
CREATE UNIQUE INDEX configuration_operations_active_target ON configuration_operations(provider, target)
WHERE state IN ('previewed','apply-intent','prepared','recovery-needed');
