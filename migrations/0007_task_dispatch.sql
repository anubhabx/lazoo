CREATE TABLE task_dispatches (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id),
    owner_boot_id TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id),
    project_configuration_revision INTEGER CHECK (
        project_configuration_revision IS NULL OR project_configuration_revision > 0
    ),
    workspace_kind TEXT CHECK (workspace_kind IN ('projectless', 'git-worktree')),
    workspace_path TEXT,
    base_commit TEXT,
    branch_name TEXT,
    turn_id TEXT UNIQUE REFERENCES conversation_turns(id),
    session_id TEXT REFERENCES employee_sessions(id),
    state TEXT NOT NULL CHECK (state IN (
        'preparing', 'workspace-ready', 'turn-recorded', 'failed', 'cancelled', 'uncertain'
    )),
    revision INTEGER NOT NULL CHECK (revision > 0),
    failure_code TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (project_id IS NULL AND project_configuration_revision IS NULL) OR
        (project_id IS NOT NULL AND project_configuration_revision IS NOT NULL)
    ),
    CHECK (
        (failure_code IS NULL AND failure_message IS NULL) OR
        (failure_code IS NOT NULL AND failure_message IS NOT NULL)
    )
);

CREATE INDEX task_dispatches_owner_state
ON task_dispatches(owner_boot_id, state);
