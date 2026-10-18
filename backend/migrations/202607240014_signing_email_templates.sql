ALTER TABLE registration_requests
  ADD COLUMN verification_attempts integer NOT NULL DEFAULT 0,
  ADD COLUMN verification_sent_at timestamptz;

UPDATE registration_requests
SET verification_expires_at = now(),
    verification_sent_at = COALESCE(updated_at, created_at)
WHERE status = 'pending_verification';

ALTER TABLE registration_requests
  ADD CONSTRAINT registration_requests_verification_attempts_range
    CHECK (verification_attempts BETWEEN 0 AND 5),
  ADD CONSTRAINT registration_requests_verification_delivery_state
    CHECK (
      (status = 'pending_verification' AND verification_sent_at IS NOT NULL)
      OR (status <> 'pending_verification' AND verification_sent_at IS NULL)
    );

ALTER TABLE email_outbox
  ADD COLUMN body_content_type text NOT NULL DEFAULT 'text/plain',
  ADD CONSTRAINT email_outbox_body_content_type_valid
    CHECK (body_content_type IN ('text/plain', 'text/html'));

CREATE TABLE organization_email_templates (
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  template_key text NOT NULL,
  subject text NOT NULL,
  html_body text NOT NULL,
  updated_by uuid REFERENCES users(id) ON DELETE SET NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (organization_id, template_key),
  CONSTRAINT organization_email_templates_key_valid CHECK (
    template_key IN (
      'registration_verification',
      'registration_approved',
      'registration_rejected',
      'account_invitation'
    )
  ),
  CONSTRAINT organization_email_templates_subject_length
    CHECK (char_length(subject) BETWEEN 1 AND 300),
  CONSTRAINT organization_email_templates_html_length
    CHECK (char_length(html_body) BETWEEN 1 AND 200000)
);

CREATE TABLE user_seals (
  id uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  owner_id uuid NOT NULL,
  name text NOT NULL,
  stored_path text NOT NULL UNIQUE,
  width_px integer NOT NULL,
  height_px integer NOT NULL,
  size_bytes bigint NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT user_seals_owner_organization_fkey
    FOREIGN KEY (owner_id, organization_id)
    REFERENCES users(id, organization_id) ON DELETE CASCADE,
  CONSTRAINT user_seals_name_length CHECK (char_length(name) BETWEEN 1 AND 120),
  CONSTRAINT user_seals_dimensions CHECK (
    width_px BETWEEN 32 AND 4096 AND height_px BETWEEN 32 AND 4096
  ),
  CONSTRAINT user_seals_size CHECK (size_bytes BETWEEN 1 AND 5242880)
);

CREATE INDEX user_seals_owner_created
  ON user_seals (organization_id, owner_id, created_at DESC);

CREATE TABLE signing_documents (
  id uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  owner_id uuid NOT NULL,
  original_filename text NOT NULL,
  source_format text NOT NULL,
  source_pdf_path text NOT NULL UNIQUE,
  source_size_bytes bigint NOT NULL,
  page_count integer NOT NULL,
  signed_pdf_path text UNIQUE,
  placements jsonb,
  signed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT signing_documents_owner_organization_fkey
    FOREIGN KEY (owner_id, organization_id)
    REFERENCES users(id, organization_id) ON DELETE CASCADE,
  CONSTRAINT signing_documents_filename_length
    CHECK (char_length(original_filename) BETWEEN 1 AND 240),
  CONSTRAINT signing_documents_source_format_valid
    CHECK (source_format IN ('pdf', 'doc', 'docx')),
  CONSTRAINT signing_documents_source_size
    CHECK (source_size_bytes BETWEEN 1 AND 31457280),
  CONSTRAINT signing_documents_page_count CHECK (page_count BETWEEN 1 AND 200),
  CONSTRAINT signing_documents_result_state CHECK (
    (signed_pdf_path IS NULL AND placements IS NULL AND signed_at IS NULL)
    OR (signed_pdf_path IS NOT NULL AND placements IS NOT NULL AND signed_at IS NOT NULL)
  )
);

CREATE INDEX signing_documents_owner_created
  ON signing_documents (organization_id, owner_id, created_at DESC);
