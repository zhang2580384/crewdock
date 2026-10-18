ALTER TABLE users
  ADD CONSTRAINT users_id_organization_unique UNIQUE (id, organization_id);

CREATE TABLE user_department_memberships (
  user_id uuid NOT NULL,
  department_id uuid NOT NULL,
  organization_id uuid NOT NULL,
  is_primary boolean NOT NULL DEFAULT false,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, department_id),
  FOREIGN KEY (user_id, organization_id)
    REFERENCES users(id, organization_id)
    ON DELETE CASCADE,
  FOREIGN KEY (department_id, organization_id)
    REFERENCES departments(id, organization_id)
    ON DELETE RESTRICT
);

CREATE UNIQUE INDEX user_department_memberships_one_primary
  ON user_department_memberships(user_id)
  WHERE is_primary;

CREATE INDEX user_department_memberships_department
  ON user_department_memberships(organization_id, department_id, user_id);

INSERT INTO user_department_memberships (
  user_id, department_id, organization_id, is_primary
)
SELECT id, department_id, organization_id, TRUE
FROM users
WHERE department_id IS NOT NULL
ON CONFLICT (user_id, department_id) DO UPDATE
SET is_primary = TRUE;
