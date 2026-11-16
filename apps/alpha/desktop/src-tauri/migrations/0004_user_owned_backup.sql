CREATE TABLE momso_transcription_terms (
  id TEXT PRIMARY KEY NOT NULL,
  heard_as TEXT NOT NULL,
  corrected_to TEXT NOT NULL,
  note TEXT,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (length(trim(heard_as)) > 0),
  CHECK (length(trim(corrected_to)) > 0),
  UNIQUE (heard_as, corrected_to)
);

CREATE INDEX idx_momso_transcription_terms_enabled_heard
  ON momso_transcription_terms(enabled, heard_as COLLATE NOCASE);

CREATE TABLE momso_backup_profiles (
  workspace_id TEXT PRIMARY KEY NOT NULL,
  github_repository TEXT,
  r2_bucket TEXT,
  neon_organization_id TEXT,
  neon_project_id TEXT,
  neon_branch_id TEXT,
  neon_database_name TEXT,
  neon_role_name TEXT,
  last_completed_snapshot_id TEXT,
  last_completed_content_sha256 TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (github_repository IS NULL OR length(trim(github_repository)) > 2),
  CHECK (r2_bucket IS NULL OR length(trim(r2_bucket)) >= 3)
);

CREATE TABLE momso_backup_snapshots (
  id TEXT PRIMARY KEY NOT NULL,
  workspace_id TEXT NOT NULL,
  parent_snapshot_id TEXT,
  state TEXT NOT NULL CHECK (state IN ('preparing', 'uploading', 'complete', 'failed', 'restored')),
  content_sha256 TEXT NOT NULL,
  encrypted_archive_sha256 TEXT,
  manifest_sha256 TEXT,
  github_commit_sha TEXT,
  r2_object_key TEXT,
  record_count INTEGER NOT NULL DEFAULT 0 CHECK (record_count >= 0),
  object_count INTEGER NOT NULL DEFAULT 0 CHECK (object_count >= 0),
  failure_code TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  FOREIGN KEY (workspace_id) REFERENCES momso_backup_profiles(workspace_id) ON DELETE CASCADE,
  FOREIGN KEY (parent_snapshot_id) REFERENCES momso_backup_snapshots(id),
  CHECK (
    (state = 'complete'
      AND encrypted_archive_sha256 IS NOT NULL
      AND manifest_sha256 IS NOT NULL
      AND github_commit_sha IS NOT NULL
      AND r2_object_key IS NOT NULL
      AND completed_at IS NOT NULL)
    OR state <> 'complete'
  )
);

CREATE INDEX idx_momso_backup_snapshots_workspace_created
  ON momso_backup_snapshots(workspace_id, created_at DESC);
