CREATE TABLE native_workspace_trust_requests (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    idempotency_key TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL CHECK (length(CAST(provider AS BLOB)) BETWEEN 1 AND 256),
    canonical_workspace_path TEXT NOT NULL CHECK (length(CAST(canonical_workspace_path AS BLOB)) BETWEEN 1 AND 32768),
    managed_workspace INTEGER NOT NULL CHECK (managed_workspace IN (0,1)),
    session_id TEXT NOT NULL,
    employee_id TEXT NOT NULL,
    owner_boot_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','approved','declined','cancelled')),
    decider TEXT CHECK (decider IS NULL OR decider IN ('user','policy','system')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    notification_state TEXT NOT NULL CHECK (notification_state IN ('pending','claimed','settled')),
    notification_claim_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    decided_at TEXT,
    CHECK ((notification_state = 'claimed') = (notification_claim_id IS NOT NULL))
);
CREATE INDEX native_workspace_trust_exact_approved
    ON native_workspace_trust_requests(provider, canonical_workspace_path)
    WHERE state = 'approved';
CREATE INDEX native_workspace_trust_pending
    ON native_workspace_trust_requests(state, sequence);
CREATE TABLE native_workspace_trust_commands (
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    trust_id TEXT NOT NULL REFERENCES native_workspace_trust_requests(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
CREATE TABLE native_workspace_trust_policies (
    provider TEXT PRIMARY KEY CHECK (length(CAST(provider AS BLOB)) BETWEEN 1 AND 256),
    auto_accept_managed_next_time INTEGER NOT NULL CHECK (auto_accept_managed_next_time IN (0,1)),
    revision INTEGER NOT NULL CHECK (revision > 0),
    updated_at TEXT NOT NULL
);
