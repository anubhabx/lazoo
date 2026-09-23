CREATE TABLE incidents (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    contract_version INTEGER NOT NULL CHECK (contract_version = 1),
    code TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    title TEXT NOT NULL,
    explanation TEXT NOT NULL,
    next_action TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id),
    employee_id TEXT REFERENCES employees(id),
    provider TEXT,
    task_id TEXT REFERENCES tasks(id),
    session_id TEXT REFERENCES employee_sessions(id),
    turn_id TEXT REFERENCES conversation_turns(id),
    message_id TEXT REFERENCES messages(id),
    source TEXT NOT NULL CHECK (source IN ('daemon', 'keeper', 'employee')),
    detail_json TEXT NOT NULL,
    occurrences INTEGER NOT NULL CHECK (occurrences > 0),
    window_occurrences INTEGER NOT NULL CHECK (window_occurrences > 0),
    first_occurred_at TEXT NOT NULL,
    last_occurred_at TEXT NOT NULL,
    window_started_at TEXT NOT NULL,
    priority TEXT NOT NULL CHECK (priority IN ('normal', 'priority')),
    state TEXT NOT NULL CHECK (state IN ('open', 'acknowledged', 'resolved')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    acknowledged_at TEXT,
    resolved_at TEXT,
    CHECK ((state = 'resolved') = (resolved_at IS NOT NULL))
);

CREATE INDEX incidents_state_order ON incidents(state, sequence);
CREATE INDEX incidents_fingerprint_state ON incidents(fingerprint, state);
CREATE INDEX incidents_employee_order ON incidents(employee_id, sequence);
CREATE INDEX incidents_project_order ON incidents(project_id, sequence);

CREATE TABLE incident_commands (
    scope TEXT NOT NULL CHECK (scope IN (
        'incident.raise', 'incident.acknowledge', 'incident.resolve'
    )),
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    incident_id TEXT NOT NULL REFERENCES incidents(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
