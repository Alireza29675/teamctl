-- Only SHA-256 token hashes and expiry times are stored. No email addresses.
CREATE TABLE IF NOT EXISTS release_signup_tokens (
  token_hash TEXT PRIMARY KEY NOT NULL,
  expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS release_signup_tokens_expiry
  ON release_signup_tokens (expires_at);
