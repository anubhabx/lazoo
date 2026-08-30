CREATE TABLE employee_sessions (
    id TEXT PRIMARY KEY,
    employee_id TEXT NOT NULL REFERENCES employees(id),
    employee_configuration_revision INTEGER NOT NULL CHECK (employee_configuration_revision > 0),
    provider TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('starting', 'idle', 'stopping', 'suspended', 'failed')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    owner_boot_id TEXT,
    native_session_id TEXT,
    native_host_pid INTEGER CHECK (native_host_pid IS NULL OR native_host_pid > 0),
    failure_code TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT,
    stopped_at TEXT
);

CREATE UNIQUE INDEX employee_sessions_one_active_per_employee
ON employee_sessions(employee_id)
WHERE state IN ('starting', 'idle', 'stopping');

CREATE INDEX employee_sessions_employee_history
ON employee_sessions(employee_id, created_at DESC, id DESC);

CREATE TABLE session_commands (
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES employee_sessions(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
