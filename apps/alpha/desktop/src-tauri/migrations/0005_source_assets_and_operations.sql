CREATE TABLE momso_source_assets (
  id TEXT PRIMARY KEY NOT NULL,
  asset_kind TEXT NOT NULL CHECK (
    asset_kind IN (
      'audio_recording',
      'raw_transcript',
      'reference_document',
      'generated_report',
      'other'
    )
  ),
  content_sha256 TEXT NOT NULL UNIQUE,
  byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
  storage_relative_path TEXT NOT NULL UNIQUE,
  original_file_name TEXT NOT NULL,
  media_type TEXT,
  captured_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  CHECK (length(content_sha256) = 64),
  CHECK (storage_relative_path LIKE 'sha256/%'),
  CHECK (length(trim(original_file_name)) > 0),
  CHECK (length(trim(captured_by)) > 0)
);

CREATE TABLE momso_record_source_assets (
  record_id TEXT NOT NULL,
  record_version INTEGER NOT NULL CHECK (record_version >= 1),
  asset_id TEXT NOT NULL,
  relation_role TEXT NOT NULL CHECK (
    relation_role IN ('source', 'supporting', 'derived')
  ),
  linked_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (record_id, record_version, asset_id, relation_role),
  FOREIGN KEY (record_id, record_version)
    REFERENCES momso_record_versions(record_id, version)
    ON DELETE CASCADE,
  FOREIGN KEY (asset_id)
    REFERENCES momso_source_assets(id)
    ON DELETE RESTRICT,
  CHECK (length(trim(linked_by)) > 0)
);

CREATE INDEX idx_momso_record_source_assets_record
  ON momso_record_source_assets(record_id, record_version, created_at);

CREATE TABLE momso_operation_runs (
  id TEXT PRIMARY KEY NOT NULL,
  operation_kind TEXT NOT NULL,
  state TEXT NOT NULL CHECK (
    state IN ('running', 'succeeded', 'failed', 'cancelled', 'interrupted')
  ),
  stage TEXT NOT NULL,
  completed_units INTEGER NOT NULL DEFAULT 0 CHECK (completed_units >= 0),
  total_units INTEGER NOT NULL DEFAULT 0 CHECK (total_units >= 0),
  result_json TEXT,
  result_sha256 TEXT,
  error_code TEXT,
  started_at TEXT NOT NULL,
  heartbeat_at TEXT NOT NULL,
  completed_at TEXT,
  CHECK (length(trim(operation_kind)) > 0),
  CHECK (length(trim(stage)) > 0),
  CHECK (completed_units <= total_units OR total_units = 0),
  CHECK (
    (state = 'running'
      AND result_json IS NULL
      AND result_sha256 IS NULL
      AND completed_at IS NULL)
    OR state <> 'running'
  ),
  CHECK (
    (state = 'succeeded'
      AND result_json IS NOT NULL
      AND result_sha256 IS NOT NULL
      AND completed_at IS NOT NULL)
    OR state <> 'succeeded'
  ),
  CHECK (result_sha256 IS NULL OR length(result_sha256) = 64)
);

CREATE INDEX idx_momso_operation_runs_started
  ON momso_operation_runs(started_at DESC);
