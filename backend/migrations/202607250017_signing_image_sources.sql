ALTER TABLE signing_documents
  DROP CONSTRAINT signing_documents_source_format_valid;

ALTER TABLE signing_documents
  ADD CONSTRAINT signing_documents_source_format_valid
    CHECK (source_format IN ('pdf', 'doc', 'docx', 'png', 'jpg', 'webp'));
