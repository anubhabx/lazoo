CREATE TABLE tasks (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    contract_version INTEGER NOT NULL CHECK (contract_version = 1),
    root_task_id TEXT NOT NULL REFERENCES tasks(id),
    parent_task_id TEXT REFERENCES tasks(id),
    depth INTEGER NOT NULL CHECK (depth BETWEEN 0 AND 4),
    project_id TEXT REFERENCES projects(id),
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    creation_turn_id TEXT,
    delegation_message_id TEXT,
    creator_kind TEXT NOT NULL CHECK (creator_kind IN ('employee', 'user')),
    creator_employee_id TEXT REFERENCES employees(id),
    creator_session_id TEXT REFERENCES employee_sessions(id),
    assigned_employee_id TEXT NOT NULL REFERENCES employees(id),
    role TEXT NOT NULL CHECK (role IN ('lead', 'worker')),
    objective TEXT NOT NULL,
    references_json TEXT NOT NULL,
    requested_authority TEXT NOT NULL CHECK (requested_authority IN (
        'ask', 'self-decision', 'full-autonomy'
    )),
    effective_authority TEXT NOT NULL CHECK (effective_authority IN (
        'ask', 'self-decision', 'full-autonomy'
    )),
    employee_configuration_revision INTEGER NOT NULL CHECK (
        employee_configuration_revision > 0
    ),
    roster_revision INTEGER NOT NULL CHECK (roster_revision > 0),
    state TEXT NOT NULL CHECK (state IN (
        'proposed', 'accepted', 'running', 'cancel-requested',
        'completed', 'failed', 'cancelled', 'uncertain'
    )),
    revision INTEGER NOT NULL CHECK (revision > 0),
    active_turn_id TEXT,
    active_session_id TEXT REFERENCES employee_sessions(id),
    failure_code TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    terminal_at TEXT,
    CHECK (
        (creator_kind = 'employee' AND creator_employee_id IS NOT NULL) OR
        (creator_kind = 'user' AND creator_employee_id IS NULL AND creator_session_id IS NULL)
    ),
    CHECK (
        (depth = 0 AND id = root_task_id AND parent_task_id IS NULL AND role = 'lead') OR
        (depth > 0 AND id <> root_task_id AND parent_task_id IS NOT NULL AND role = 'worker')
    )
);

CREATE INDEX tasks_root_parent_order
ON tasks(root_task_id, parent_task_id, sequence);

CREATE INDEX tasks_conversation_order
ON tasks(conversation_id, sequence);

CREATE INDEX tasks_assignee_state_order
ON tasks(assigned_employee_id, state, sequence);

CREATE TABLE task_evidence (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    task_revision INTEGER NOT NULL CHECK (task_revision > 0),
    kind TEXT NOT NULL CHECK (kind IN (
        'proposed', 'accepted', 'started', 'progress', 'completed',
        'failed', 'cancel-requested', 'cancelled', 'uncertain'
    )),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('employee', 'user')),
    actor_employee_id TEXT REFERENCES employees(id),
    actor_session_id TEXT REFERENCES employee_sessions(id),
    message_id TEXT,
    turn_id TEXT,
    session_id TEXT REFERENCES employee_sessions(id),
    detail_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (
        (actor_kind = 'employee' AND actor_employee_id IS NOT NULL) OR
        (actor_kind = 'user' AND actor_employee_id IS NULL AND actor_session_id IS NULL)
    )
);

CREATE INDEX task_evidence_task_order
ON task_evidence(task_id, sequence);

CREATE TABLE task_commands (
    scope TEXT NOT NULL CHECK (scope IN (
        'task.delegate', 'task.accept', 'task.start', 'task.report',
        'task.cancel', 'task.resolve'
    )),
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);

INSERT INTO tasks (
    id, contract_version, root_task_id, parent_task_id, depth, project_id,
    conversation_id, creation_turn_id, delegation_message_id, creator_kind,
    creator_employee_id, creator_session_id, assigned_employee_id, role,
    objective, references_json, requested_authority, effective_authority,
    employee_configuration_revision, roster_revision, state, revision,
    active_turn_id, active_session_id, failure_code, failure_message,
    created_at, updated_at, terminal_at
)
SELECT
    turn.id, 1, turn.id, NULL, 0, conversation.project_id,
    turn.conversation_id, turn.id, NULL, 'user', NULL, NULL,
    turn.recipient_employee_id, 'lead', turn.text, '[]', 'ask', 'ask',
    turn.recipient_employee_configuration_revision,
    MAX(1, (SELECT revision FROM roster_state WHERE singleton = 1)),
    CASE turn.state
        WHEN 'recorded' THEN 'accepted'
        WHEN 'retryable-failed' THEN 'accepted'
        WHEN 'submitting' THEN 'running'
        WHEN 'accepted' THEN 'running'
        WHEN 'completed' THEN 'completed'
        WHEN 'terminal-failed' THEN 'failed'
        WHEN 'cancelled' THEN 'cancelled'
        WHEN 'uncertain' THEN 'uncertain'
    END,
    1,
    CASE WHEN turn.state IN ('completed', 'terminal-failed', 'cancelled')
        THEN NULL ELSE turn.id END,
    turn.recipient_session_id,
    turn.failure_code,
    turn.failure_message,
    turn.created_at,
    turn.updated_at,
    turn.terminal_at
FROM conversation_turns turn
JOIN conversations conversation ON conversation.id = turn.conversation_id
WHERE turn.author_kind = 'user';

INSERT INTO task_evidence (
    id, task_id, task_revision, kind, actor_kind, actor_employee_id,
    actor_session_id, message_id, turn_id, session_id, detail_json, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))), 2) || '-8' ||
        substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))),
    turn.id,
    1,
    CASE turn.state
        WHEN 'recorded' THEN 'accepted'
        WHEN 'retryable-failed' THEN 'accepted'
        WHEN 'submitting' THEN 'started'
        WHEN 'accepted' THEN 'started'
        WHEN 'completed' THEN 'completed'
        WHEN 'terminal-failed' THEN 'failed'
        WHEN 'cancelled' THEN 'cancelled'
        WHEN 'uncertain' THEN 'uncertain'
    END,
    'user', NULL, NULL, turn.message_id, turn.id, turn.recipient_session_id,
    json_object('source', 'schema-5-turn-backfill', 'turnState', turn.state),
    turn.updated_at
FROM conversation_turns turn
WHERE turn.author_kind = 'user';

CREATE TABLE messages_v6 (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    envelope_version INTEGER NOT NULL CHECK (envelope_version IN (1, 2)),
    idempotency_key TEXT NOT NULL UNIQUE,
    correlation_id TEXT NOT NULL,
    causation_id TEXT REFERENCES messages_v6(id),
    sender_kind TEXT NOT NULL CHECK (sender_kind IN ('employee', 'user')),
    sender_employee_id TEXT REFERENCES employees(id),
    sender_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_id TEXT NOT NULL REFERENCES employees(id),
    recipient_session_id TEXT REFERENCES employee_sessions(id),
    project_id TEXT REFERENCES projects(id),
    conversation_id TEXT,
    task_id TEXT REFERENCES tasks(id),
    kind TEXT NOT NULL CHECK (kind IN ('delegation', 'peer', 'reply', 'result', 'status', 'system')),
    body_json TEXT NOT NULL,
    references_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK ((envelope_version = 1 AND sender_kind = 'employee') OR envelope_version = 2),
    CHECK (
        (sender_kind = 'employee' AND sender_employee_id IS NOT NULL) OR
        (sender_kind = 'user' AND sender_employee_id IS NULL AND sender_session_id IS NULL)
    )
);

INSERT INTO messages_v6 (
    sequence, id, envelope_version, idempotency_key, correlation_id, causation_id,
    sender_kind, sender_employee_id, sender_session_id, recipient_employee_id,
    recipient_session_id, project_id, conversation_id, task_id, kind, body_json,
    references_json, created_at
)
SELECT
    message.sequence, message.id, message.envelope_version, message.idempotency_key,
    message.correlation_id, message.causation_id, message.sender_kind,
    message.sender_employee_id, message.sender_session_id,
    message.recipient_employee_id, message.recipient_session_id,
    message.project_id, message.conversation_id,
    CASE WHEN turn.author_kind = 'user' THEN turn.id ELSE NULL END,
    message.kind, message.body_json, message.references_json, message.created_at
FROM messages message
LEFT JOIN conversation_turns turn ON turn.message_id = message.id;

CREATE TABLE message_deliveries_v6 (
    message_id TEXT PRIMARY KEY REFERENCES messages_v6(id),
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
INSERT INTO message_deliveries_v6 SELECT * FROM message_deliveries;

CREATE TABLE message_delivery_attempts_v6 (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages_v6(id),
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
INSERT INTO message_delivery_attempts_v6 SELECT * FROM message_delivery_attempts;

CREATE TABLE message_dead_letters_v6 (
    message_id TEXT PRIMARY KEY REFERENCES messages_v6(id),
    attempt_id TEXT NOT NULL REFERENCES message_delivery_attempts_v6(id),
    failure_code TEXT NOT NULL,
    failure_message TEXT NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO message_dead_letters_v6 SELECT * FROM message_dead_letters;

CREATE TABLE message_commands_v6 (
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages_v6(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
INSERT INTO message_commands_v6 SELECT * FROM message_commands;

CREATE TABLE conversation_turns_v6 (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    contract_version INTEGER NOT NULL CHECK (contract_version = 1),
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    task_id TEXT REFERENCES tasks(id),
    author_kind TEXT NOT NULL CHECK (author_kind IN ('employee', 'user')),
    author_employee_id TEXT REFERENCES employees(id),
    author_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_id TEXT NOT NULL REFERENCES employees(id),
    recipient_session_id TEXT REFERENCES employee_sessions(id),
    recipient_employee_configuration_revision INTEGER NOT NULL CHECK (
        recipient_employee_configuration_revision > 0
    ),
    text TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages_v6(id),
    message_attempt_id TEXT REFERENCES message_delivery_attempts_v6(id),
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

INSERT INTO conversation_turns_v6 (
    sequence, id, contract_version, conversation_id, task_id, author_kind,
    author_employee_id, author_session_id, recipient_employee_id,
    recipient_session_id, recipient_employee_configuration_revision, text,
    message_id, message_attempt_id, correlation_id, state, revision,
    owner_boot_id, provider, native_session_id, native_turn_id, failure_code,
    failure_message, transcript_bytes, transcript_truncated, created_at,
    updated_at, submitted_at, accepted_at, completed_at, cancel_requested_at,
    terminal_at
)
SELECT
    sequence, id, contract_version, conversation_id,
    CASE WHEN author_kind = 'user' THEN id ELSE NULL END,
    author_kind, author_employee_id, author_session_id, recipient_employee_id,
    recipient_session_id, recipient_employee_configuration_revision, text,
    message_id, message_attempt_id, correlation_id, state, revision,
    owner_boot_id, provider, native_session_id, native_turn_id, failure_code,
    failure_message, transcript_bytes, transcript_truncated, created_at,
    updated_at, submitted_at, accepted_at, completed_at, cancel_requested_at,
    terminal_at
FROM conversation_turns;

CREATE TABLE conversation_turn_events_v6 (
    turn_id TEXT NOT NULL REFERENCES conversation_turns_v6(id),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    kind TEXT NOT NULL,
    provider TEXT NOT NULL,
    native_session_id TEXT,
    native_turn_id TEXT,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (turn_id, sequence)
);
INSERT INTO conversation_turn_events_v6 SELECT * FROM conversation_turn_events;

CREATE TABLE turn_commands_v6 (
    scope TEXT NOT NULL CHECK (scope IN (
        'turn.record', 'turn.begin-submission', 'turn.observe', 'turn.fail',
        'turn.retry', 'turn.request-cancel', 'turn.finish-cancel', 'turn.uncertain'
    )),
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    turn_id TEXT NOT NULL REFERENCES conversation_turns_v6(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
INSERT INTO turn_commands_v6 SELECT * FROM turn_commands;

DROP TABLE conversation_turn_events;
DROP TABLE turn_commands;
DROP TABLE conversation_turns;
DROP TABLE message_dead_letters;
DROP TABLE message_delivery_attempts;
DROP TABLE message_deliveries;
DROP TABLE message_commands;
DROP TABLE messages;

ALTER TABLE messages_v6 RENAME TO messages;
ALTER TABLE message_deliveries_v6 RENAME TO message_deliveries;
ALTER TABLE message_delivery_attempts_v6 RENAME TO message_delivery_attempts;
ALTER TABLE message_dead_letters_v6 RENAME TO message_dead_letters;
ALTER TABLE message_commands_v6 RENAME TO message_commands;
ALTER TABLE conversation_turns_v6 RENAME TO conversation_turns;
ALTER TABLE conversation_turn_events_v6 RENAME TO conversation_turn_events;
ALTER TABLE turn_commands_v6 RENAME TO turn_commands;

CREATE INDEX messages_correlation_order ON messages(correlation_id, sequence);
CREATE INDEX messages_recipient_order ON messages(recipient_employee_id, sequence);
CREATE INDEX conversation_turns_conversation_order
ON conversation_turns(conversation_id, sequence);
CREATE INDEX conversation_turns_state_order ON conversation_turns(state, sequence);
CREATE INDEX conversation_turns_task_order ON conversation_turns(task_id, sequence);
