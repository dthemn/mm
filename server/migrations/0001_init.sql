-- This schema deliberately has no amounts, merchants, categories or purchase dates.
CREATE TABLE invites (
    code_hash  BYTEA PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ
);

CREATE TABLE users (
    id                 UUID PRIMARY KEY,
    phrase_pub         BYTEA NOT NULL UNIQUE,
    wrapped_dek_phrase BYTEA NOT NULL,
    seq                BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE credentials (
    cred_id         BYTEA PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    passkey         JSONB NOT NULL,
    wrapped_dek_prf BYTEA
);
CREATE INDEX credentials_user ON credentials(user_id);

CREATE TABLE challenges (
    id         UUID PRIMARY KEY,
    kind       TEXT NOT NULL,
    user_id    UUID,
    state      JSONB,
    nonce      BYTEA,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE records (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    id         UUID NOT NULL,
    revision   BIGINT NOT NULL,
    seq        BIGINT NOT NULL,
    size       INTEGER NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    blob       BYTEA NOT NULL,
    PRIMARY KEY (user_id, id)
);
CREATE INDEX records_seq ON records(user_id, seq);
