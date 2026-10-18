ALTER TABLE user_seals
  ADD COLUMN source_kind text NOT NULL DEFAULT 'upload',
  ADD COLUMN generator_definition jsonb;

ALTER TABLE user_seals
  ADD CONSTRAINT user_seals_source_kind_valid
    CHECK (source_kind IN ('upload', 'generated', 'legacy_generated')),
  ADD CONSTRAINT user_seals_generator_definition_object
    CHECK (generator_definition IS NULL OR jsonb_typeof(generator_definition) = 'object'),
  ADD CONSTRAINT user_seals_generated_definition_required
    CHECK (source_kind <> 'generated' OR generator_definition IS NOT NULL);

UPDATE user_seals
SET source_kind = 'legacy_generated'
WHERE generator_definition IS NULL
  AND size_bytes >= 32768
  AND (
    (width_px = 720 AND height_px = 720)
    OR (width_px = 840 AND height_px = 560)
    OR (width_px = 640 AND height_px = 640)
  );

UPDATE user_seal_generator_preferences
SET settings = settings || jsonb_build_object(
  'schema_version', 2,
  'organization_width', 92,
  'text_stroke_width', 2,
  'purpose_font_size', 42,
  'purpose_spacing', 4,
  'serial_width', 82
);
