-- When each account was created and last used the API, to the minute, for the owner's user counts (`mm-server stats`).
-- Accounts made before this migration have no creation time.
ALTER TABLE users ADD COLUMN created_at TIMESTAMPTZ, ADD COLUMN seen_at TIMESTAMPTZ;
ALTER TABLE users ALTER COLUMN created_at SET DEFAULT date_trunc('minute', now());
