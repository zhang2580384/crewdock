ALTER TABLE sites
  ADD COLUMN resource_type text NOT NULL DEFAULT 'website',
  ADD COLUMN connection_config jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD CONSTRAINT sites_resource_type_valid
    CHECK (resource_type IN ('website', 'ssh', 'rdp')),
  ADD CONSTRAINT sites_connection_config_object
    CHECK (jsonb_typeof(connection_config) = 'object');

CREATE INDEX idx_sites_resource_type
  ON sites(organization_id, resource_type, category);
