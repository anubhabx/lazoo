-- 0 keeps a warmed session until the local user stops it, as before schema 11; any other value
-- lets the daemon suspend the employee's idle session once no turn has touched it for that long.
ALTER TABLE employees
    ADD COLUMN idle_timeout_seconds INTEGER NOT NULL DEFAULT 0
    CHECK (idle_timeout_seconds = 0 OR idle_timeout_seconds BETWEEN 60 AND 86400);
