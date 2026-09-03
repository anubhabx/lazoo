CREATE TABLE messages_v5 (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    envelope_version INTEGER NOT NULL CHECK (envelope_version IN (1, 2)),
    idempotency_key TEXT NOT NULL UNIQUE,
    correlation_id TEXT NOT NULL,
    causation_id TEXT REFERENCES messages_v5(id),
    sender_kind TEXT NOT NULL CHECK (sender_kind IN ('employee', 'user')),
    sender_employee_id TEXT REFERENCES employees(id),
    sender_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_id TEXT NOT NULL REFERENCES employees(id),
    recipient_session_id TEXT REFERENCES employee_sessions(id),
    project_id TEXT REFERENCES projects(id),
    conversation_id TEXT,
    task_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('delegation', 'peer', 'reply', 'result', 'status', 'system')),
    body_json TEXT NOT NULL,
    references_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (
        (envelope_version = 1 AND sender_kind = 'employee') OR
        envelope_version = 2
    ),
    CHECK (
        (sender_kind = 'employee' AND sender_employee_id IS NOT NULL) OR
        (sender_kind = 'user' AND sender_employee_id IS NULL AND sender_session_id IS NULL)
    )
);

INSERT INTO messages_v5 (
    sequence, id, envelope_version, idempotency_key, correlation_id, causation_id,
    sender_kind, sender_employee_id, sender_session_id, recipient_employee_id,
    recipient_session_id, project_id, conversation_id, task_id, kind, body_json,
    references_json, created_at
)
SELECT
    sequence, id, envelope_version, idempotency_key, correlation_id, causation_id,
    'employee', sender_employee_id, sender_session_id, recipient_employee_id,
    recipient_session_id, project_id, conversation_id, task_id, kind, body_json,
    references_json, created_at
FROM messages;

CREATE TABLE message_deliveries_v5 (
    message_id TEXT PRIMARY KEY REFERENCES messages_v5(id),
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

INSERT INTO message_deliveries_v5
SELECT * FROM message_deliveries;

CREATE TABLE message_delivery_attempts_v5 (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages_v5(id),
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

INSERT INTO message_delivery_attempts_v5
SELECT * FROM message_delivery_attempts;

CREATE TABLE message_dead_letters_v5 (
    message_id TEXT PRIMARY KEY REFERENCES messages_v5(id),
    attempt_id TEXT NOT NULL REFERENCES message_delivery_attempts_v5(id),
    failure_code TEXT NOT NULL,
    failure_message TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT INTO message_dead_letters_v5
SELECT * FROM message_dead_letters;

CREATE TABLE message_commands_v5 (
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages_v5(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);

INSERT INTO message_commands_v5
SELECT * FROM message_commands;

DROP TABLE message_dead_letters;
DROP TABLE message_delivery_attempts;
DROP TABLE message_deliveries;
DROP TABLE message_commands;
DROP TABLE messages;

ALTER TABLE messages_v5 RENAME TO messages;
ALTER TABLE message_deliveries_v5 RENAME TO message_deliveries;
ALTER TABLE message_delivery_attempts_v5 RENAME TO message_delivery_attempts;
ALTER TABLE message_dead_letters_v5 RENAME TO message_dead_letters;
ALTER TABLE message_commands_v5 RENAME TO message_commands;

CREATE INDEX messages_correlation_order
ON messages(correlation_id, sequence);

CREATE INDEX messages_recipient_order
ON messages(recipient_employee_id, sequence);

CREATE TABLE conversations (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    contract_version INTEGER NOT NULL CHECK (contract_version = 1),
    project_id TEXT REFERENCES projects(id),
    lead_employee_id TEXT NOT NULL REFERENCES employees(id),
    lead_employee_configuration_revision INTEGER NOT NULL CHECK (
        lead_employee_configuration_revision > 0
    ),
    title TEXT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX conversations_project_order
ON conversations(project_id, sequence);

CREATE TABLE conversation_turns (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    contract_version INTEGER NOT NULL CHECK (contract_version = 1),
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    author_kind TEXT NOT NULL CHECK (author_kind IN ('employee', 'user')),
    author_employee_id TEXT REFERENCES employees(id),
    author_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_id TEXT NOT NULL REFERENCES employees(id),
    recipient_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_configuration_revision INTEGER NOT NULL CHECK (
        recipient_employee_configuration_revision > 0
    ),
    text TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id),
    message_attempt_id TEXT REFERENCES message_delivery_attempts(id),
    correlation_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'recorded', 'submitting', 'accepted', 'completed', 'retryable-failed',
        'terminal-failed', 'cancelled', 'uncertain'
    )),
    revision INTEGER NOT NULL CHECK (revision > 0),
    owner_boot_id TEXT,
    provider TEXT,
    native_session_id TEXT,
    native_turn_id TEXT,
    failure_code TEXT,
    failure_message TEXT,
    transcript_bytes INTEGER NOT NULL DEFAULT 0 CHECK (transcript_bytes >= 0),
    transcript_truncated INTEGER NOT NULL DEFAULT 0 CHECK (transcript_truncated IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    submitted_at TEXT,
    accepted_at TEXT,
    completed_at TEXT,
    cancel_requested_at TEXT,
    terminal_at TEXT,
    CHECK (
        (author_kind = 'employee' AND author_employee_id IS NOT NULL) OR
        (author_kind = 'user' AND author_employee_id IS NULL AND author_session_id IS NULL)
    )
);

CREATE INDEX conversation_turns_conversation_order
ON conversation_turns(conversation_id, sequence);

CREATE INDEX conversation_turns_state_order
ON conversation_turns(state, sequence);

CREATE TABLE conversation_turn_events (
    turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    kind TEXT NOT NULL,
    provider TEXT NOT NULL,
    native_session_id TEXT,
    native_turn_id TEXT,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (turn_id, sequence)
);

CREATE TABLE conversation_commands (
    scope TEXT NOT NULL CHECK (scope = 'conversation.create'),
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);

CREATE TABLE turn_commands (
    scope TEXT NOT NULL CHECK (scope IN (
        'turn.record', 'turn.begin-submission', 'turn.observe', 'turn.fail',
        'turn.retry', 'turn.request-cancel', 'turn.finish-cancel', 'turn.uncertain'
    )),
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
