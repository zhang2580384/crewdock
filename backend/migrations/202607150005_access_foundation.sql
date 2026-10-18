CREATE TYPE system_role AS ENUM ('system_admin', 'member');
CREATE TYPE project_role AS ENUM ('viewer', 'editor', 'administrator');

ALTER TABLE users
  ADD COLUMN system_role system_role,
  ADD COLUMN is_active boolean NOT NULL DEFAULT true;

UPDATE users
SET system_role = CASE
  WHEN role = 'system_admin'::user_role THEN 'system_admin'::system_role
  ELSE 'member'::system_role
END;

CREATE FUNCTION sync_users_role_columns()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_OP = 'INSERT' THEN
    IF NEW.system_role IS NULL THEN
      NEW.system_role := CASE
        WHEN NEW.role = 'system_admin'::user_role THEN 'system_admin'::system_role
        ELSE 'member'::system_role
      END;
    ELSE
      NEW.role := CASE NEW.system_role
        WHEN 'system_admin'::system_role THEN 'system_admin'::user_role
        WHEN 'member'::system_role THEN 'project_member'::user_role
      END;
    END IF;
  ELSIF NEW.system_role IS DISTINCT FROM OLD.system_role THEN
    IF NEW.system_role IS NULL THEN
      RAISE EXCEPTION 'users.system_role cannot be null';
    END IF;

    NEW.role := CASE NEW.system_role
      WHEN 'system_admin'::system_role THEN 'system_admin'::user_role
      WHEN 'member'::system_role THEN 'project_member'::user_role
    END;
  ELSIF NEW.role IS DISTINCT FROM OLD.role THEN
    IF NEW.role IS NULL THEN
      RAISE EXCEPTION 'users.role cannot be null';
    END IF;

    NEW.system_role := CASE
      WHEN NEW.role = 'system_admin'::user_role THEN 'system_admin'::system_role
      ELSE 'member'::system_role
    END;
  END IF;

  IF NEW.role IS NULL OR NEW.system_role IS NULL THEN
    RAISE EXCEPTION 'users role columns cannot be null';
  END IF;

  RETURN NEW;
END;
$$;

CREATE TRIGGER users_sync_role_columns
BEFORE INSERT OR UPDATE ON users
FOR EACH ROW
EXECUTE FUNCTION sync_users_role_columns();

ALTER TABLE users ALTER COLUMN system_role SET NOT NULL;

ALTER TABLE project_members ADD COLUMN project_role project_role;

UPDATE project_members
SET project_role = CASE role
  WHEN 'readonly_member'::user_role THEN 'viewer'::project_role
  WHEN 'project_member'::user_role THEN 'editor'::project_role
  ELSE 'administrator'::project_role
END;

CREATE FUNCTION sync_project_member_role_columns()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_OP = 'INSERT' THEN
    IF NEW.project_role IS NULL THEN
      NEW.project_role := CASE NEW.role
        WHEN 'readonly_member'::user_role THEN 'viewer'::project_role
        WHEN 'project_member'::user_role THEN 'editor'::project_role
        ELSE 'administrator'::project_role
      END;
    ELSE
      NEW.role := CASE NEW.project_role
        WHEN 'administrator'::project_role THEN 'project_admin'::user_role
        WHEN 'editor'::project_role THEN 'project_member'::user_role
        WHEN 'viewer'::project_role THEN 'readonly_member'::user_role
      END;
    END IF;
  ELSIF NEW.project_role IS DISTINCT FROM OLD.project_role THEN
    IF NEW.project_role IS NULL THEN
      RAISE EXCEPTION 'project_members.project_role cannot be null';
    END IF;

    NEW.role := CASE NEW.project_role
      WHEN 'administrator'::project_role THEN 'project_admin'::user_role
      WHEN 'editor'::project_role THEN 'project_member'::user_role
      WHEN 'viewer'::project_role THEN 'readonly_member'::user_role
    END;
  ELSIF NEW.role IS DISTINCT FROM OLD.role THEN
    IF NEW.role IS NULL THEN
      RAISE EXCEPTION 'project_members.role cannot be null';
    END IF;

    NEW.project_role := CASE NEW.role
      WHEN 'readonly_member'::user_role THEN 'viewer'::project_role
      WHEN 'project_member'::user_role THEN 'editor'::project_role
      ELSE 'administrator'::project_role
    END;
  END IF;

  IF NEW.role IS NULL OR NEW.project_role IS NULL THEN
    RAISE EXCEPTION 'project_members role columns cannot be null';
  END IF;

  RETURN NEW;
END;
$$;

CREATE TRIGGER project_members_sync_role_columns
BEFORE INSERT OR UPDATE ON project_members
FOR EACH ROW
EXECUTE FUNCTION sync_project_member_role_columns();

ALTER TABLE project_members ALTER COLUMN project_role SET NOT NULL;

CREATE TABLE department_admins (
  department_id uuid NOT NULL REFERENCES departments(id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (department_id, user_id)
);

INSERT INTO department_admins (department_id, user_id)
SELECT department_id, id
FROM users
WHERE role = 'department_admin'::user_role AND department_id IS NOT NULL
ON CONFLICT DO NOTHING;

CREATE TABLE project_department_grants (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  department_id uuid NOT NULL REFERENCES departments(id) ON DELETE CASCADE,
  project_role project_role NOT NULL,
  include_descendants boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, department_id)
);

ALTER TABLE departments
  ADD COLUMN sort_order integer NOT NULL DEFAULT 0,
  ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();

WITH ordered_departments AS (
  SELECT id,
         row_number() OVER (
           PARTITION BY organization_id, parent_id
           ORDER BY created_at, id
         ) - 1 AS position
  FROM departments
)
UPDATE departments
SET sort_order = ordered_departments.position
FROM ordered_departments
WHERE departments.id = ordered_departments.id;

ALTER TABLE departments
  DROP CONSTRAINT departments_parent_id_fkey;

ALTER TABLE departments
  ADD CONSTRAINT departments_id_organization_unique
  UNIQUE (id, organization_id);

ALTER TABLE departments
  ADD CONSTRAINT departments_parent_organization_fkey
  FOREIGN KEY (parent_id, organization_id)
  REFERENCES departments(id, organization_id)
  ON DELETE RESTRICT,
  ADD CONSTRAINT departments_sort_order_nonnegative
  CHECK (sort_order >= 0),
  ADD CONSTRAINT departments_sibling_sort_unique
  UNIQUE NULLS NOT DISTINCT (organization_id, parent_id, sort_order)
  DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE projects ADD COLUMN archived_at timestamptz;

CREATE TABLE project_file_cleanup_jobs (
  project_id uuid PRIMARY KEY,
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
  last_error text,
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  lease_owner uuid,
  lease_until timestamptz,
  terminal_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK ((lease_owner IS NULL) = (lease_until IS NULL))
);

CREATE TABLE file_storage_jobs (
  id uuid PRIMARY KEY,
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  project_id uuid NOT NULL,
  file_asset_id uuid,
  operation_owner uuid,
  operation_state text,
  kind text NOT NULL CHECK (kind IN ('upload_cleanup', 'delete')),
  source_path text NOT NULL,
  destination_path text,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
  last_error text,
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  lease_owner uuid,
  lease_until timestamptz,
  terminal_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK ((lease_owner IS NULL) = (lease_until IS NULL)),
  CHECK (
    (kind = 'upload_cleanup' AND file_asset_id IS NOT NULL AND destination_path IS NOT NULL
      AND operation_owner IS NOT NULL AND operation_state IN ('receiving', 'cancelled'))
    OR (kind = 'delete' AND file_asset_id IS NULL AND destination_path IS NULL
      AND operation_owner IS NULL AND operation_state IS NULL)
  )
);

CREATE INDEX idx_department_admins_user ON department_admins(user_id);
CREATE INDEX idx_project_members_user_project ON project_members(user_id, project_id);
CREATE INDEX idx_project_department_grants_department
  ON project_department_grants(department_id, project_id);
CREATE INDEX idx_departments_parent_order
  ON departments(organization_id, parent_id, sort_order, name);
CREATE INDEX idx_project_file_cleanup_jobs_due
  ON project_file_cleanup_jobs(next_attempt_at, lease_until, updated_at)
  WHERE terminal_at IS NULL;
CREATE INDEX idx_share_links_target
  ON share_links(organization_id, target_type, target_id);
CREATE INDEX idx_file_storage_jobs_due
  ON file_storage_jobs(next_attempt_at, lease_until, updated_at)
  WHERE terminal_at IS NULL;
CREATE INDEX idx_file_storage_jobs_project
  ON file_storage_jobs(organization_id, project_id);
