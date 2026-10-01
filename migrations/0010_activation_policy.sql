-- Stored as a JSON string like authority_mode and checkout_policy; every employee written before
-- schema 10 keeps activation with the local user.
ALTER TABLE employees
    ADD COLUMN activation_policy TEXT NOT NULL DEFAULT '"manual"';
