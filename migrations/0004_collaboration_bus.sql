CREATE TABLE messages (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    envelope_version INTEGER NOT NULL CHECK (envelope_version = 1),
    idempotency_key TEXT NOT NULL UNIQUE,
    correlation_id TEXT NOT NULL,
    causation_id TEXT REFERENCES messages(id),
    sender_employee_id TEXT NOT NULL REFERENCES employees(id),
    sender_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_id TEXT NOT NULL REFERENCES employees(id),
    recipient_session_id TEXT REFERENCES employee_sessions(id),
    project_id TEXT REFERENCES projects(id),
    conversation_id TEXT,
    task_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('delegation', 'peer', 'reply', 'result', 'status', 'system')),
    body_json TEXT NOT NULL,
    references_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX messages_correlation_order
ON messages(correlation_id, sequence);

CREATE INDEX messages_recipient_order
ON messages(recipient_employee_id, sequence);

CREATE TABLE message_deliveries (
    message_id TEXT PRIMARY KEY REFERENCES messages(id),
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'delivering', 'acknowledged', 'retryable-failed',
        'terminal-failed', 'cancelled'
    )),
    revision INTEGER NOT NULL CHECK (revision > 0),
    attempt_count INTEGER NOT NULL CHECK (attempt_count >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 5),
    next_attempt_at TEXT,
    last_failure_code TEXT,
    last_failure_message TEXT,
    acknowledged_at TEXT,
    terminal_at TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE message_delivery_attempts (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages(id),
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    state TEXT NOT NULL CHECK (state IN (
        'delivering', 'acknowledged', 'retryable-failed',
        'terminal-failed', 'cancelled'
    )),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    failure_code TEXT,
    failure_message TEXT,
    acknowledgement_key TEXT,
    acknowledgement_evidence_json TEXT,
    UNIQUE (message_id, attempt_number)
);

CREATE TABLE message_dead_letters (
    message_id TEXT PRIMARY KEY REFERENCES messages(id),
    attempt_id TEXT NOT NULL REFERENCES message_delivery_attempts(id),
    failure_code TEXT NOT NULL,
    failure_message TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE message_commands (
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
