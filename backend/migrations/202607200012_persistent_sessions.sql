ALTER TABLE sessions
  ADD COLUMN client_ip text,
  ADD COLUMN last_seen_at timestamptz NOT NULL DEFAULT now();

CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);
