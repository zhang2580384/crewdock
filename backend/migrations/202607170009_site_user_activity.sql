CREATE TABLE site_user_activity (
  site_id uuid NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  open_count bigint NOT NULL DEFAULT 0 CHECK (open_count >= 0),
  last_opened_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (site_id, user_id)
);

CREATE INDEX idx_site_user_activity_user_recent
  ON site_user_activity(user_id, last_opened_at DESC, site_id);
