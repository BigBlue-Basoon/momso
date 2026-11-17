CREATE TABLE source_asset_deletions (
  asset_id TEXT PRIMARY KEY NOT NULL REFERENCES momso_source_assets(id),
  operation_id TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL CHECK(state IN ('pending', 'trashed', 'purge_pending', 'purged')),
  requested_at TEXT NOT NULL,
  retain_until TEXT NOT NULL,
  remote_plan_json TEXT,
  remote_remaining_json TEXT,
  local_remaining_json TEXT
);
