CREATE TABLE markdown_folders (
  id uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  parent_id uuid,
  name text NOT NULL,
  sort_order integer NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
  created_by uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (id, project_id),
  FOREIGN KEY (parent_id, project_id)
    REFERENCES markdown_folders(id, project_id) ON DELETE RESTRICT
);

ALTER TABLE markdown_docs
  ADD COLUMN folder_id uuid,
  ADD COLUMN sort_order integer NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
  ADD FOREIGN KEY (folder_id, project_id)
    REFERENCES markdown_folders(id, project_id) ON DELETE RESTRICT;

WITH ordered_docs AS (
  SELECT id,
         row_number() OVER (PARTITION BY project_id ORDER BY updated_at DESC, id) - 1 AS position
  FROM markdown_docs
)
UPDATE markdown_docs
SET sort_order = ordered_docs.position
FROM ordered_docs
WHERE markdown_docs.id = ordered_docs.id;

ALTER TABLE markdown_folders
  ADD CONSTRAINT markdown_folders_sibling_sort_unique
  UNIQUE NULLS NOT DISTINCT (project_id, parent_id, sort_order)
  DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE markdown_docs
  ADD CONSTRAINT markdown_docs_sibling_sort_unique
  UNIQUE NULLS NOT DISTINCT (project_id, folder_id, sort_order)
  DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_markdown_folders_project_parent
  ON markdown_folders(project_id, parent_id, sort_order, name);
CREATE INDEX idx_markdown_docs_project_folder
  ON markdown_docs(project_id, folder_id, sort_order, title);
