CREATE TYPE site_visibility AS ENUM (
  'public',
  'personal'
);

CREATE TABLE sites (
  id uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  owner_id uuid REFERENCES users(id) ON DELETE CASCADE,
  visibility site_visibility NOT NULL DEFAULT 'personal',
  category text NOT NULL DEFAULT '未分组',
  title text NOT NULL,
  url text NOT NULL,
  description text,
  icon_url text,
  tags text[] NOT NULL DEFAULT '{}',
  is_pinned boolean NOT NULL DEFAULT false,
  sort_order integer NOT NULL DEFAULT 0,
  created_by uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK (
    (visibility = 'public' AND owner_id IS NULL)
    OR (visibility = 'personal' AND owner_id IS NOT NULL)
  )
);

CREATE INDEX idx_sites_org_visibility ON sites(organization_id, visibility);
CREATE INDEX idx_sites_owner ON sites(owner_id);
CREATE INDEX idx_sites_category ON sites(organization_id, category);
