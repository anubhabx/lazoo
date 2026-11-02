-- Public observations only. Empty account_key denotes the unmanaged personal login.
CREATE TABLE provider_usage_windows (
    provider TEXT NOT NULL,
    account_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    window_json TEXT NOT NULL,
    PRIMARY KEY (provider, account_key, kind)
);
CREATE TABLE provider_usage_hourly (
    start INTEGER NOT NULL,
    provider TEXT NOT NULL,
    account_key TEXT NOT NULL,
    sessions INTEGER NOT NULL DEFAULT 0,
    turns_completed INTEGER NOT NULL DEFAULT 0,
    turns_failed INTEGER NOT NULL DEFAULT 0,
    requests INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER,
    output_tokens INTEGER,
    tokens_reported INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (start, provider, account_key)
);
CREATE TABLE provider_usage_watermarks (
    provider TEXT NOT NULL,
    scope TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (provider, scope)
);
CREATE INDEX provider_usage_watermarks_age ON provider_usage_watermarks(updated_at);
-- Recover creation from the earliest retained account event, falling back to metadata
-- for installations whose audit retention has already removed that event.
UPDATE provider_accounts SET record_json = json_set(record_json, '$.createdAt',
    COALESCE((SELECT MIN(created_at) FROM audit_events
        WHERE event_type = 'provider.account.updated'
        AND json_extract(payload_json, '$.account.spec.id') = provider_accounts.id),
        json_extract(record_json, '$.updatedAt')));
