ALTER TABLE sites
  ADD COLUMN IF NOT EXISTS icon_text text,
  ADD COLUMN IF NOT EXISTS auto_icon_url text,
  ADD COLUMN IF NOT EXISTS icon_fetched_at timestamptz;

ALTER TABLE sites
  DROP CONSTRAINT IF EXISTS sites_icon_text_length_check;

ALTER TABLE sites
  ADD CONSTRAINT sites_icon_text_length_check
  CHECK (icon_text IS NULL OR char_length(icon_text) <= 12);
