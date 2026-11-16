CREATE TABLE momso_records (
  id TEXT PRIMARY KEY NOT NULL,
  record_kind TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft', 'needs_review', 'reviewed')),
  current_version INTEGER NOT NULL CHECK (current_version >= 1),
  summary TEXT,
  summary_sha256 TEXT,
  summary_basis_body_sha256 TEXT,
  body_sha256 TEXT NOT NULL,
  body_bytes INTEGER NOT NULL CHECK (body_bytes >= 0),
  created_by TEXT NOT NULL,
  updated_by TEXT NOT NULL,
  reviewed_by TEXT,
  reviewed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (
    (summary IS NULL AND summary_sha256 IS NULL AND summary_basis_body_sha256 IS NULL)
    OR (summary IS NOT NULL AND summary_sha256 IS NOT NULL)
  ),
  CHECK (
    (status = 'reviewed'
      AND summary IS NOT NULL
      AND summary_basis_body_sha256 IS NOT NULL
      AND summary_basis_body_sha256 = body_sha256
      AND reviewed_by IS NOT NULL
      AND reviewed_at IS NOT NULL)
    OR (status <> 'reviewed' AND reviewed_by IS NULL AND reviewed_at IS NULL)
  )
);

CREATE TABLE momso_record_versions (
  record_id TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version >= 1),
  record_kind TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft', 'needs_review', 'reviewed')),
  summary TEXT,
  summary_sha256 TEXT,
  summary_basis_body_sha256 TEXT,
  body_sha256 TEXT NOT NULL,
  body_bytes INTEGER NOT NULL CHECK (body_bytes >= 0),
  change_kind TEXT NOT NULL CHECK (change_kind IN ('create', 'update', 'restore')),
  source_version INTEGER,
  actor TEXT NOT NULL,
  confirmed_by TEXT,
  reviewed_by TEXT,
  reviewed_at TEXT,
  plan_digest TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (record_id, version),
  FOREIGN KEY (record_id) REFERENCES momso_records(id) ON DELETE CASCADE,
  CHECK (
    (summary IS NULL AND summary_sha256 IS NULL AND summary_basis_body_sha256 IS NULL)
    OR (summary IS NOT NULL AND summary_sha256 IS NOT NULL)
  ),
  CHECK (
    (change_kind = 'restore' AND source_version IS NOT NULL)
    OR (change_kind <> 'restore' AND source_version IS NULL)
  ),
  CHECK (
    (status = 'reviewed'
      AND summary IS NOT NULL
      AND summary_basis_body_sha256 IS NOT NULL
      AND summary_basis_body_sha256 = body_sha256
      AND reviewed_by IS NOT NULL
      AND reviewed_at IS NOT NULL)
    OR (status <> 'reviewed' AND reviewed_by IS NULL AND reviewed_at IS NULL)
  )
);

CREATE TABLE momso_record_bodies (
  record_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  body TEXT NOT NULL,
  body_sha256 TEXT NOT NULL,
  body_bytes INTEGER NOT NULL CHECK (body_bytes >= 0),
  PRIMARY KEY (record_id, version),
  FOREIGN KEY (record_id, version)
    REFERENCES momso_record_versions(record_id, version)
    ON DELETE CASCADE
);

CREATE INDEX idx_momso_records_status_updated
  ON momso_records(status, updated_at DESC);

CREATE INDEX idx_momso_record_versions_record_created
  ON momso_record_versions(record_id, created_at DESC);
