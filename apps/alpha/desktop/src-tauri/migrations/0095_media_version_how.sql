ALTER TABLE media_generation ADD COLUMN version_how TEXT CHECK (version_how IS NULL OR version_how IN ('regenerated','edited'));
