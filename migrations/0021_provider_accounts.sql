CREATE TABLE provider_accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    record_json TEXT NOT NULL,
    protected_secret BLOB
);
CREATE INDEX provider_accounts_provider ON provider_accounts(provider);
CREATE TABLE provider_account_policies (
    provider TEXT PRIMARY KEY,
    policy_json TEXT NOT NULL
);
CREATE TABLE provider_account_bindings (
    session_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    routing_mode TEXT NOT NULL,
    account_id TEXT REFERENCES provider_accounts(id),
    upstream_account_id TEXT REFERENCES provider_accounts(id),
    native_session_id TEXT
);
CREATE INDEX provider_account_native_owner ON provider_account_bindings(provider, native_session_id);
