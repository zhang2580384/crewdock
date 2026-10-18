ALTER TABLE projects
  ADD CONSTRAINT projects_id_organization_unique UNIQUE (id, organization_id);

ALTER TABLE markdown_folders
  ADD CONSTRAINT markdown_folders_id_project_unique UNIQUE (id, project_id);

ALTER TABLE markdown_docs
  ADD CONSTRAINT markdown_docs_id_project_unique UNIQUE (id, project_id);

CREATE TABLE markdown_folder_department_grants (
  folder_id uuid NOT NULL,
  project_id uuid NOT NULL,
  department_id uuid NOT NULL,
  organization_id uuid NOT NULL,
  access_role project_role NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (folder_id, department_id),
  FOREIGN KEY (folder_id, project_id)
    REFERENCES markdown_folders(id, project_id)
    ON DELETE CASCADE,
  FOREIGN KEY (project_id, organization_id)
    REFERENCES projects(id, organization_id)
    ON DELETE CASCADE,
  FOREIGN KEY (department_id, organization_id)
    REFERENCES departments(id, organization_id)
    ON DELETE CASCADE,
  CHECK (access_role IN ('viewer'::project_role, 'editor'::project_role))
);

CREATE TABLE markdown_doc_department_grants (
  doc_id uuid NOT NULL,
  project_id uuid NOT NULL,
  department_id uuid NOT NULL,
  organization_id uuid NOT NULL,
  access_role project_role NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (doc_id, department_id),
  FOREIGN KEY (doc_id, project_id)
    REFERENCES markdown_docs(id, project_id)
    ON DELETE CASCADE,
  FOREIGN KEY (project_id, organization_id)
    REFERENCES projects(id, organization_id)
    ON DELETE CASCADE,
  FOREIGN KEY (department_id, organization_id)
    REFERENCES departments(id, organization_id)
    ON DELETE CASCADE,
  CHECK (access_role IN ('viewer'::project_role, 'editor'::project_role))
);

CREATE INDEX idx_markdown_folder_grants_department
  ON markdown_folder_department_grants(organization_id, department_id, project_id);

CREATE INDEX idx_markdown_doc_grants_department
  ON markdown_doc_department_grants(organization_id, department_id, project_id);
