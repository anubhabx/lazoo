-- Raw provider frames are retained per completed turn as a second archive kind.  Both archive
-- tables gain `source_kind` so one turn owns at most one canonical ('turn') archive and one
-- 'raw-frames' archive; the content-addressed uniqueness and the recoverable intents are kept.
CREATE TABLE memory_archives_v19 (
    source_turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    source_kind TEXT NOT NULL DEFAULT 'turn' CHECK (source_kind IN ('turn','raw-frames')),
    source_hash TEXT NOT NULL, project_id TEXT REFERENCES projects(id), scope_key TEXT NOT NULL, archive_name TEXT NOT NULL,
    compressed_bytes INTEGER NOT NULL CHECK (compressed_bytes >= 0), uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
    truncated INTEGER NOT NULL CHECK (truncated IN (0,1)), created_at TEXT NOT NULL,
    PRIMARY KEY (source_turn_id, source_kind), UNIQUE(scope_key, source_hash), UNIQUE(archive_name)
);
INSERT INTO memory_archives_v19 (source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at)
SELECT source_turn_id,'turn',source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at FROM memory_archives;

CREATE TABLE memory_archive_intents_v19 (
    source_turn_id TEXT NOT NULL REFERENCES conversation_turns(id),
    source_kind TEXT NOT NULL DEFAULT 'turn' CHECK (source_kind IN ('turn','raw-frames')),
    source_hash TEXT NOT NULL, project_id TEXT REFERENCES projects(id), scope_key TEXT NOT NULL, archive_name TEXT NOT NULL,
    compressed_bytes INTEGER NOT NULL CHECK (compressed_bytes >= 0), uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
    truncated INTEGER NOT NULL CHECK (truncated IN (0,1)), state TEXT NOT NULL CHECK (state IN ('write-pending','cleanup-pending','error')),
    created_at TEXT NOT NULL,
    PRIMARY KEY (source_turn_id, source_kind)
);
INSERT INTO memory_archive_intents_v19 (source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at)
SELECT source_turn_id,'turn',source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at FROM memory_archive_intents;

DROP TABLE memory_archives;
DROP TABLE memory_archive_intents;
ALTER TABLE memory_archives_v19 RENAME TO memory_archives;
ALTER TABLE memory_archive_intents_v19 RENAME TO memory_archive_intents;
CREATE UNIQUE INDEX memory_archive_intents_name ON memory_archive_intents(archive_name);
