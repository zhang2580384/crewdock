CREATE TABLE user_seal_generator_preferences (
  organization_id uuid NOT NULL,
  owner_id uuid NOT NULL,
  settings jsonb NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (organization_id, owner_id),
  CONSTRAINT user_seal_generator_preferences_owner_fkey
    FOREIGN KEY (owner_id, organization_id)
    REFERENCES users(id, organization_id) ON DELETE CASCADE,
  CONSTRAINT user_seal_generator_preferences_settings_object
    CHECK (jsonb_typeof(settings) = 'object')
);
