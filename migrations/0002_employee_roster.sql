CREATE TABLE employees (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    provider TEXT NOT NULL,
    role TEXT NOT NULL,
    instructions TEXT NOT NULL,
    model TEXT,
    reasoning_effort TEXT,
    native_settings_json TEXT NOT NULL,
    authority_mode TEXT NOT NULL,
    checkout_policy TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    configuration_revision INTEGER NOT NULL CHECK (configuration_revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    disabled_at TEXT
);

CREATE TABLE provider_observations (
    provider TEXT PRIMARY KEY,
    verification TEXT NOT NULL,
    version TEXT,
    protocol TEXT,
    capabilities_json TEXT NOT NULL,
    authentication_json TEXT NOT NULL,
    degradation_reason TEXT,
    checked_at TEXT NOT NULL
);

CREATE TABLE roster_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    updated_at TEXT NOT NULL
);

INSERT INTO roster_state (singleton, revision, updated_at)
VALUES (1, 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
