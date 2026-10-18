ALTER TABLE user_seals
  ADD CONSTRAINT user_seals_id_organization_unique UNIQUE (id, organization_id);

CREATE TABLE seal_authorizations (
  seal_id uuid NOT NULL,
  organization_id uuid NOT NULL,
  user_id uuid NOT NULL,
  granted_by uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (seal_id, user_id),
  CONSTRAINT seal_authorizations_seal_organization_fkey
    FOREIGN KEY (seal_id, organization_id)
    REFERENCES user_seals(id, organization_id) ON DELETE CASCADE,
  CONSTRAINT seal_authorizations_user_organization_fkey
    FOREIGN KEY (user_id, organization_id)
    REFERENCES users(id, organization_id) ON DELETE CASCADE,
  CONSTRAINT seal_authorizations_granted_by_organization_fkey
    FOREIGN KEY (granted_by, organization_id)
    REFERENCES users(id, organization_id) ON DELETE RESTRICT
);

CREATE INDEX seal_authorizations_user_lookup
  ON seal_authorizations (organization_id, user_id, seal_id);

-- Preserve access to seals created before centralized administrator management.
INSERT INTO seal_authorizations (seal_id, organization_id, user_id, granted_by)
SELECT seal.id, seal.organization_id, seal.owner_id,
       COALESCE(
         (
           SELECT administrator.id
           FROM users administrator
           WHERE administrator.organization_id = seal.organization_id
             AND administrator.system_role = 'system_admin'
             AND administrator.is_active = TRUE
           ORDER BY administrator.created_at, administrator.id
           LIMIT 1
         ),
         seal.owner_id
       )
FROM user_seals seal
JOIN users owner
  ON owner.id = seal.owner_id
 AND owner.organization_id = seal.organization_id
WHERE owner.system_role = 'member'
ON CONFLICT DO NOTHING;
