-- A user-escalated incident is announced on the Windows desktop at most once. The daemon claims
-- it first (`claimed`), then records `shown` or `failed`; a keeper record is settled as `keeper`
-- because the keeper already raised its own notification. Nothing is retried.
ALTER TABLE incidents ADD COLUMN user_notified_at TEXT;
ALTER TABLE incidents
    ADD COLUMN user_notification TEXT
    CHECK (user_notification IS NULL OR user_notification IN ('claimed', 'shown', 'failed', 'keeper'));
