-- Bounded lead remediation. A promoted incident on a lead's task tree opens a five-minute window
-- in which that lead may apply at most two remedies; whatever is still unresolved after it is
-- escalated to the local user. An incident with no lead to remediate goes to the user at once.
ALTER TABLE incidents
    ADD COLUMN escalation TEXT NOT NULL DEFAULT 'none'
    CHECK (escalation IN ('none', 'lead', 'remediated', 'user'));
ALTER TABLE incidents ADD COLUMN remediation_deadline_at TEXT;
ALTER TABLE incidents ADD COLUMN escalated_at TEXT;
ALTER TABLE incidents
    ADD COLUMN escalation_reason TEXT
    CHECK (escalation_reason IS NULL OR escalation_reason IN (
        'no-lead', 'no-remedy', 'remedies-exhausted', 'recurred-after-remedy', 'schema-upgrade'
    ));

-- Priority incidents raised before this schema were never offered to a lead's remediation
-- window, so they go straight to the user.
UPDATE incidents
SET escalation = 'user', escalated_at = updated_at, escalation_reason = 'schema-upgrade'
WHERE priority = 'priority' AND state != 'resolved';

CREATE INDEX incidents_escalation_order ON incidents(escalation, sequence);

-- One row per remedy a lead applied. The attempt bound is enforced by the schema as well as the
-- store: a remedy row exists only for an action that committed in the same transaction.
CREATE TABLE incident_remedies (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    incident_id TEXT NOT NULL REFERENCES incidents(id),
    attempt INTEGER NOT NULL CHECK (attempt BETWEEN 1 AND 2),
    kind TEXT NOT NULL CHECK (kind IN ('cancel-task', 'redelegate-task')),
    idempotency_key TEXT NOT NULL UNIQUE,
    request_id TEXT NOT NULL,
    lead_employee_id TEXT NOT NULL REFERENCES employees(id),
    lead_task_id TEXT NOT NULL REFERENCES tasks(id),
    turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    target_task_id TEXT NOT NULL REFERENCES tasks(id),
    result_task_id TEXT REFERENCES tasks(id),
    occurrences_at_remedy INTEGER NOT NULL CHECK (occurrences_at_remedy > 0),
    note TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (incident_id, attempt)
);
