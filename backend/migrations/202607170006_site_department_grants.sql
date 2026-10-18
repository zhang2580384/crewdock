ALTER TABLE sites
  ADD CONSTRAINT sites_id_organization_unique UNIQUE (id, organization_id);

CREATE TABLE site_department_grants (
  site_id uuid NOT NULL,
  department_id uuid NOT NULL,
  organization_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (site_id, department_id),
  FOREIGN KEY (site_id, organization_id)
    REFERENCES sites(id, organization_id)
    ON DELETE CASCADE,
  FOREIGN KEY (department_id, organization_id)
    REFERENCES departments(id, organization_id)
    ON DELETE CASCADE
);

CREATE INDEX idx_site_department_grants_department
  ON site_department_grants(organization_id, department_id, site_id);
