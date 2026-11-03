CREATE TABLE extension_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    state_json TEXT NOT NULL,
    pending_json TEXT
);
INSERT INTO extension_state VALUES (1, 1, '{"library":[],"exposures":[]}', NULL);
