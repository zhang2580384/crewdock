CREATE TYPE registration_mode AS ENUM ('closed', 'application');

CREATE TYPE registration_request_status AS ENUM (
  'pending_verification',
  'pending_review',
  'approved',
  'rejected',
  'activated'
);

CREATE TYPE smtp_tls_mode AS ENUM ('none', 'starttls', 'tls');

DO $$
BEGIN
  IF EXISTS (
    SELECT lower(email)
    FROM users
    GROUP BY lower(email)
    HAVING count(*) > 1
  ) THEN
    RAISE EXCEPTION
      'cannot enforce normalized user email uniqueness: case-insensitive duplicates exist';
  END IF;
END;
$$;

CREATE UNIQUE INDEX users_email_lower_unique
  ON users (lower(email));

CREATE TABLE organization_auth_settings (
  organization_id uuid PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
  registration_mode registration_mode NOT NULL DEFAULT 'closed',
  allowed_email_domains text[] NOT NULL DEFAULT '{}',
  updated_by uuid REFERENCES users(id) ON DELETE SET NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT organization_auth_settings_domains_nonempty CHECK (
    array_position(allowed_email_domains, '') IS NULL
  )
);

CREATE TABLE organization_email_settings (
  organization_id uuid PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
  enabled boolean NOT NULL DEFAULT false,
  smtp_host text NOT NULL DEFAULT '',
  smtp_port integer NOT NULL DEFAULT 587 CHECK (smtp_port BETWEEN 1 AND 65535),
  tls_mode smtp_tls_mode NOT NULL DEFAULT 'starttls',
  smtp_username text,
  smtp_password_encrypted bytea,
  from_email text NOT NULL DEFAULT '',
  from_name text NOT NULL DEFAULT '',
  reply_to text,
  last_test_at timestamptz,
  last_test_succeeded boolean,
  last_test_error text,
  updated_by uuid REFERENCES users(id) ON DELETE SET NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT organization_email_settings_host_length CHECK (char_length(smtp_host) <= 253),
  CONSTRAINT organization_email_settings_username_length CHECK (
    smtp_username IS NULL OR char_length(smtp_username) <= 320
  ),
  CONSTRAINT organization_email_settings_from_email_length CHECK (char_length(from_email) <= 254),
  CONSTRAINT organization_email_settings_from_name_length CHECK (char_length(from_name) <= 240),
  CONSTRAINT organization_email_settings_reply_to_length CHECK (
    reply_to IS NULL OR char_length(reply_to) <= 254
  )
);

INSERT INTO organization_auth_settings (organization_id)
SELECT id FROM organizations
ON CONFLICT DO NOTHING;

INSERT INTO organization_email_settings (organization_id)
SELECT id FROM organizations
ON CONFLICT DO NOTHING;

CREATE TABLE registration_requests (
  id uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  email text NOT NULL,
  display_name text NOT NULL,
  department_interest text,
  reason text,
  status registration_request_status NOT NULL DEFAULT 'pending_verification',
  verification_token_hash text,
  verification_expires_at timestamptz,
  email_verified_at timestamptz,
  activation_token_hash text,
  activation_expires_at timestamptz,
  reviewed_by uuid,
  reviewed_at timestamptz,
  rejection_reason text,
  approved_department_ids uuid[] NOT NULL DEFAULT '{}',
  primary_department_id uuid,
  activated_user_id uuid UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT registration_requests_email_length CHECK (
    char_length(email) BETWEEN 3 AND 254
  ),
  CONSTRAINT registration_requests_display_name_length CHECK (
    char_length(display_name) BETWEEN 1 AND 240
  ),
  CONSTRAINT registration_requests_verification_token_pair CHECK (
    (verification_token_hash IS NULL) = (verification_expires_at IS NULL)
  ),
  CONSTRAINT registration_requests_activation_token_pair CHECK (
    (activation_token_hash IS NULL) = (activation_expires_at IS NULL)
  ),
  CONSTRAINT registration_requests_reviewer_organization_fkey
    FOREIGN KEY (reviewed_by, organization_id)
    REFERENCES users(id, organization_id) ON DELETE RESTRICT,
  CONSTRAINT registration_requests_primary_department_organization_fkey
    FOREIGN KEY (primary_department_id, organization_id)
    REFERENCES departments(id, organization_id) ON DELETE RESTRICT,
  CONSTRAINT registration_requests_activated_user_organization_fkey
    FOREIGN KEY (activated_user_id, organization_id)
    REFERENCES users(id, organization_id) ON DELETE RESTRICT,
  CONSTRAINT registration_requests_approval_shape CHECK (
    status <> 'approved'
    OR (
      primary_department_id IS NOT NULL
      AND cardinality(approved_department_ids) > 0
      AND primary_department_id = ANY(approved_department_ids)
    )
  ),
  CONSTRAINT registration_requests_activation_shape CHECK (
    (status = 'activated') = (activated_user_id IS NOT NULL)
  ),
  CONSTRAINT registration_requests_status_tokens CHECK (
    (status = 'pending_verification'
      AND verification_token_hash IS NOT NULL
      AND email_verified_at IS NULL
      AND activation_token_hash IS NULL)
    OR (status = 'pending_review'
      AND verification_token_hash IS NULL
      AND email_verified_at IS NOT NULL
      AND activation_token_hash IS NULL)
    OR (status = 'approved'
      AND verification_token_hash IS NULL
      AND email_verified_at IS NOT NULL
      AND activation_token_hash IS NOT NULL)
    OR (status IN ('rejected', 'activated')
      AND verification_token_hash IS NULL
      AND activation_token_hash IS NULL)
  )
);

CREATE UNIQUE INDEX registration_requests_active_email_unique
  ON registration_requests (lower(email))
  WHERE status IN ('pending_verification', 'pending_review', 'approved');

CREATE UNIQUE INDEX registration_requests_verification_token_unique
  ON registration_requests (verification_token_hash)
  WHERE verification_token_hash IS NOT NULL;

CREATE UNIQUE INDEX registration_requests_activation_token_unique
  ON registration_requests (activation_token_hash)
  WHERE activation_token_hash IS NOT NULL;

CREATE INDEX registration_requests_review_queue
  ON registration_requests (organization_id, created_at)
  WHERE status = 'pending_review';

CREATE TABLE email_outbox (
  id uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
  organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  recipient text NOT NULL,
  subject text NOT NULL,
  body_encrypted bytea,
  idempotency_key text NOT NULL,
  test_requested_by uuid REFERENCES users(id) ON DELETE SET NULL,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts BETWEEN 1 AND 10),
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  last_error text,
  sent_at timestamptz,
  failed_at timestamptz,
  lease_owner uuid,
  lease_until timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT email_outbox_recipient_length CHECK (
    char_length(recipient) BETWEEN 3 AND 254
  ),
  CONSTRAINT email_outbox_subject_length CHECK (
    char_length(subject) BETWEEN 1 AND 500
  ),
  CONSTRAINT email_outbox_body_length CHECK (
    body_encrypted IS NULL OR octet_length(body_encrypted) BETWEEN 29 AND 1048604
  ),
  CONSTRAINT email_outbox_idempotency_key_length CHECK (
    char_length(idempotency_key) BETWEEN 1 AND 240
  ),
  CONSTRAINT email_outbox_lease_pair CHECK (
    (lease_owner IS NULL) = (lease_until IS NULL)
  ),
  CONSTRAINT email_outbox_terminal_state CHECK (
    NOT (sent_at IS NOT NULL AND failed_at IS NOT NULL)
  ),
  CONSTRAINT email_outbox_body_lifecycle CHECK (
    body_encrypted IS NOT NULL OR sent_at IS NOT NULL OR failed_at IS NOT NULL
  )
);

CREATE UNIQUE INDEX email_outbox_idempotency_unique
  ON email_outbox (organization_id, idempotency_key);

CREATE INDEX email_outbox_due
  ON email_outbox (next_attempt_at, created_at)
  WHERE sent_at IS NULL AND failed_at IS NULL;
