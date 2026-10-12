-- A message is `normal` or `priority`. Every incident notice to a lead is a priority message, and
-- one dead-lettered priority message is a priority incident (brief section 14.2). Priority does
-- not change delivery order.
ALTER TABLE messages
    ADD COLUMN priority TEXT NOT NULL DEFAULT 'normal'
    CHECK (priority IN ('normal', 'priority'));

-- Incident notices written before this schema were already priority in substance.
UPDATE messages
SET priority = 'priority'
WHERE kind = 'status' AND json_extract(body_json, '$.kind') = 'incident-notice';
