-- The local user's overrides of section 14.2's persistent-failure thresholds, one row per code:
-- `occurrences` equivalent failures inside `window_seconds` promote its incident. A code with no
-- row keeps its default. Only the user writes this table, through the daemon.
CREATE TABLE incident_thresholds (
    code TEXT PRIMARY KEY,
    occurrences INTEGER NOT NULL CHECK (occurrences BETWEEN 1 AND 100),
    window_seconds INTEGER NOT NULL CHECK (window_seconds BETWEEN 60 AND 86400),
    updated_at TEXT NOT NULL
);
