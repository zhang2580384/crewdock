ALTER TABLE organization_auth_settings
  ADD COLUMN allow_all_email_domains boolean NOT NULL DEFAULT false;
