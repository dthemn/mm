-- Bytes of ciphertext each account stores, kept up to date by pushes, so the quota check needs no scan.
ALTER TABLE users ADD COLUMN bytes BIGINT NOT NULL DEFAULT 0;
UPDATE users u SET bytes = COALESCE((SELECT SUM(size) FROM records r WHERE r.user_id = u.id), 0);
