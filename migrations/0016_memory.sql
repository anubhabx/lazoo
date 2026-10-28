CREATE TABLE memory_policies (
    scope_key TEXT PRIMARY KEY, project_id TEXT UNIQUE REFERENCES projects(id), revision INTEGER NOT NULL CHECK (revision > 0),
    retention_enabled INTEGER NOT NULL DEFAULT 1 CHECK (retention_enabled IN (0,1)), retention_days INTEGER NOT NULL DEFAULT 30 CHECK (retention_days BETWEEN 1 AND 3650),
    quota_bytes INTEGER NOT NULL DEFAULT 268435456 CHECK (quota_bytes BETWEEN 0 AND 1099511627776), max_source_bytes INTEGER NOT NULL DEFAULT 4194304 CHECK (max_source_bytes BETWEEN 1 AND 67108864),
    allowed_types_json TEXT NOT NULL DEFAULT '["decision","action","achievement","question","failure","constraint","fact","summary"]', inject_enabled INTEGER NOT NULL DEFAULT 1 CHECK (inject_enabled IN (0,1)), updated_at TEXT NOT NULL
);
CREATE TABLE memory_archives (
    source_turn_id TEXT PRIMARY KEY REFERENCES conversation_turns(id), source_hash TEXT NOT NULL, project_id TEXT REFERENCES projects(id), scope_key TEXT NOT NULL, archive_name TEXT NOT NULL, compressed_bytes INTEGER NOT NULL CHECK (compressed_bytes >= 0), uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0), truncated INTEGER NOT NULL CHECK (truncated IN (0,1)), created_at TEXT NOT NULL,
    UNIQUE(scope_key, source_hash), UNIQUE(archive_name)
);
CREATE TABLE memory_records (
    id TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK (revision > 0), project_id TEXT REFERENCES projects(id), scope_key TEXT NOT NULL, source_turn_id TEXT NOT NULL REFERENCES conversation_turns(id), source_hash TEXT NOT NULL, record_type TEXT NOT NULL CHECK (record_type IN ('decision','action','achievement','question','failure','constraint','fact','summary')), text TEXT NOT NULL CHECK (length(CAST(text AS BLOB)) <= 8192), confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1), importance INTEGER NOT NULL CHECK (importance BETWEEN 0 AND 100), created_at TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1)), superseded_at TEXT
);
CREATE INDEX memory_records_scope_type ON memory_records(scope_key, record_type, active, created_at DESC);
CREATE VIRTUAL TABLE memory_records_fts USING fts5(text, content='memory_records', content_rowid='rowid');
CREATE TRIGGER memory_records_ai AFTER INSERT ON memory_records BEGIN INSERT INTO memory_records_fts(rowid,text) VALUES (new.rowid,new.text); END;
CREATE TRIGGER memory_records_ad AFTER DELETE ON memory_records BEGIN INSERT INTO memory_records_fts(memory_records_fts,rowid,text) VALUES ('delete',old.rowid,old.text); END;
CREATE TRIGGER memory_records_au AFTER UPDATE OF text ON memory_records BEGIN INSERT INTO memory_records_fts(memory_records_fts,rowid,text) VALUES ('delete',old.rowid,old.text); INSERT INTO memory_records_fts(rowid,text) VALUES (new.rowid,new.text); END;
CREATE TABLE memory_summary_jobs (
    id TEXT PRIMARY KEY, source_hash TEXT NOT NULL, source_turn_id TEXT NOT NULL REFERENCES conversation_turns(id), project_id TEXT REFERENCES projects(id), scope_key TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('pending','claimed','completed','failed')), attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0), due_at TEXT NOT NULL, lease_owner_boot_id TEXT, lease_expires_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, failure_message TEXT,
    UNIQUE(scope_key, source_hash)
);
CREATE INDEX memory_summary_jobs_claim ON memory_summary_jobs(state, due_at, created_at);

CREATE TABLE memory_archive_intents (
    source_turn_id TEXT PRIMARY KEY REFERENCES conversation_turns(id), source_hash TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id), scope_key TEXT NOT NULL, archive_name TEXT NOT NULL,
    compressed_bytes INTEGER NOT NULL CHECK (compressed_bytes >= 0), uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
    truncated INTEGER NOT NULL CHECK (truncated IN (0,1)), state TEXT NOT NULL CHECK (state IN ('write-pending','cleanup-pending','error')),
    created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX memory_archive_intents_name ON memory_archive_intents(archive_name);
