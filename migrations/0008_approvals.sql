CREATE TABLE approvals (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    contract_version INTEGER NOT NULL CHECK (contract_version = 1),
    turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    task_id TEXT REFERENCES tasks(id),
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    project_id TEXT REFERENCES projects(id),
    employee_id TEXT NOT NULL REFERENCES employees(id),
    session_id TEXT REFERENCES employee_sessions(id),
    owner_boot_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    native_session_id TEXT,
    native_turn_id TEXT,
    native_request_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('command-execution', 'file-change')),
    action TEXT NOT NULL,
    detail_json TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    authority TEXT NOT NULL CHECK (authority IN ('ask', 'self-decision', 'full-autonomy')),
    scope_expansion INTEGER NOT NULL CHECK (scope_expansion IN (0, 1)),
    state TEXT NOT NULL CHECK (state IN (
        'pending', 'approved', 'declined', 'expired', 'cancelled'
    )),
    decider TEXT CHECK (decider IN ('policy', 'user', 'system')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    failure_code TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    decided_at TEXT,
    CHECK (
        (state = 'pending' AND decider IS NULL AND decided_at IS NULL) OR
        (state <> 'pending' AND decider IS NOT NULL AND decided_at IS NOT NULL)
    ),
    CHECK (
        (failure_code IS NULL AND failure_message IS NULL) OR
        (failure_code IS NOT NULL AND failure_message IS NOT NULL)
    )
);

CREATE INDEX approvals_state_order ON approvals(state, sequence);
CREATE INDEX approvals_turn_order ON approvals(turn_id, sequence);
CREATE INDEX approvals_task_order ON approvals(task_id, sequence);
CREATE INDEX approvals_owner_state ON approvals(owner_boot_id, state);

CREATE TABLE approval_commands (
    scope TEXT NOT NULL CHECK (scope IN ('approval.request', 'approval.decide')),
    idempotency_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    approval_id TEXT NOT NULL REFERENCES approvals(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (scope, idempotency_key)
);
