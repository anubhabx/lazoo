-- Durable, opt-in configuration and audit links for the memory-summary worker.  A row is absent
-- until the local user selects an existing native employee and an explicit request budget.
CREATE TABLE memory_worker_settings (
    scope_key TEXT PRIMARY KEY,
    project_id TEXT UNIQUE REFERENCES projects(id),
    revision INTEGER NOT NULL CHECK (revision > 0),
    employee_id TEXT NOT NULL REFERENCES employees(id),
    model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 256),
    requests_per_minute INTEGER NOT NULL CHECK (requests_per_minute BETWEEN 1 AND 60),
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    -- Kept in the durable settings row so a restart cannot bypass the worker's
    -- conservative one-request-per-scope admission spacing.
    last_admitted_at TEXT,
    updated_at TEXT NOT NULL
);

-- A claimed job can have at most one internal conversation/turn.  The lease token and attempt
-- make a restart with an uncertain native turn observable rather than safe to resend blindly.
CREATE TABLE memory_summary_attempts (
    job_id TEXT PRIMARY KEY REFERENCES memory_summary_jobs(id),
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    lease_token TEXT NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    session_id TEXT UNIQUE REFERENCES employee_sessions(id),
    state TEXT NOT NULL CHECK (state IN ('submitted','uncertain','completed','failed')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    UNIQUE(conversation_id),
    UNIQUE(turn_id)
);
CREATE INDEX memory_summary_attempts_state ON memory_summary_attempts(state, started_at);
