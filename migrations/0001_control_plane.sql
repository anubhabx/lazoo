CREATE TABLE installations (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    repository_identity TEXT NOT NULL UNIQUE,
    canonical_path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    checkout_mode TEXT NOT NULL,
    configuration_revision INTEGER NOT NULL CHECK (configuration_revision > 0),
    created_at TEXT NOT NULL
);

CREATE TABLE processed_commands (
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    response_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);

CREATE TABLE audit_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE outbox (
    id TEXT PRIMARY KEY,
    audit_sequence INTEGER NOT NULL UNIQUE REFERENCES audit_events(sequence),
    created_at TEXT NOT NULL,
    published_at TEXT
);

CREATE TABLE recovery_runs (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    summary_json TEXT
);

CREATE TABLE emergency_incident_imports (
    incident_id TEXT PRIMARY KEY,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    imported_at TEXT NOT NULL
);
