CREATE TABLE feedback_outbox (
  feedback_id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
  rating TEXT CHECK (rating IN ('helpful', 'unhelpful')),
  reason_codes_json TEXT NOT NULL DEFAULT '[]',
  workflow TEXT NOT NULL CHECK (workflow IN ('ask', 'draft', 'review')),
  response_kind TEXT NOT NULL CHECK (response_kind IN ('prototype', 'generated')),
  completion TEXT NOT NULL CHECK (completion IN ('completed', 'stopped', 'failed', 'unknown')),
  length_bucket TEXT NOT NULL CHECK (length_bucket IN (
    'under_500',
    'from_500_to_1499',
    'from_1500_to_2999',
    'from_3000_to_5999',
    'over_6000'
  )),
  app_version TEXT NOT NULL,
  platform TEXT NOT NULL CHECK (platform = 'windows'),
  schema_version INTEGER NOT NULL CHECK (schema_version = 1),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  next_attempt_at INTEGER NOT NULL DEFAULT (unixepoch()),
  last_error_code TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  CHECK (
    (operation = 'upsert' AND rating IS NOT NULL)
    OR (operation = 'delete' AND rating IS NULL)
  )
);

CREATE INDEX feedback_outbox_due_idx
  ON feedback_outbox (next_attempt_at, created_at);
