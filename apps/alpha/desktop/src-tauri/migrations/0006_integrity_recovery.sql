ALTER TABLE momso_source_assets
  ADD COLUMN integrity_state TEXT NOT NULL DEFAULT 'active'
  CHECK (integrity_state IN ('active', 'quarantined'));

ALTER TABLE momso_source_assets
  ADD COLUMN integrity_issue TEXT;

ALTER TABLE momso_source_assets
  ADD COLUMN quarantined_at TEXT;

ALTER TABLE momso_source_assets
  ADD COLUMN quarantined_by TEXT;

ALTER TABLE momso_backup_profiles
  ADD COLUMN recovery_key_confirmed_at TEXT;

CREATE INDEX idx_momso_source_assets_integrity_state
  ON momso_source_assets(integrity_state, created_at);

CREATE TRIGGER momso_source_assets_integrity_insert
BEFORE INSERT ON momso_source_assets
WHEN NOT (
  (
    NEW.integrity_state = 'active'
    AND NEW.integrity_issue IS NULL
    AND NEW.quarantined_at IS NULL
    AND NEW.quarantined_by IS NULL
  )
  OR
  (
    NEW.integrity_state = 'quarantined'
    AND length(trim(COALESCE(NEW.integrity_issue, ''))) > 0
    AND length(trim(COALESCE(NEW.quarantined_at, ''))) > 0
    AND length(trim(COALESCE(NEW.quarantined_by, ''))) > 0
  )
)
BEGIN
  SELECT RAISE(ABORT, 'source asset integrity metadata is inconsistent');
END;

CREATE TRIGGER momso_source_assets_integrity_update
BEFORE UPDATE OF integrity_state, integrity_issue, quarantined_at, quarantined_by
ON momso_source_assets
WHEN NOT (
  (
    NEW.integrity_state = 'active'
    AND NEW.integrity_issue IS NULL
    AND NEW.quarantined_at IS NULL
    AND NEW.quarantined_by IS NULL
  )
  OR
  (
    NEW.integrity_state = 'quarantined'
    AND length(trim(COALESCE(NEW.integrity_issue, ''))) > 0
    AND length(trim(COALESCE(NEW.quarantined_at, ''))) > 0
    AND length(trim(COALESCE(NEW.quarantined_by, ''))) > 0
  )
)
BEGIN
  SELECT RAISE(ABORT, 'source asset integrity metadata is inconsistent');
END;
