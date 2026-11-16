-- These tables are a rebuildable index. Archive files remain authoritative.
CREATE TABLE archive_zets (
  id TEXT PRIMARY KEY, title TEXT NOT NULL, summary TEXT NOT NULL,
  kind TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, file_path TEXT NOT NULL UNIQUE
);
CREATE TABLE archive_zets_assets (
  zet_id TEXT NOT NULL, asset_id TEXT NOT NULL,
  PRIMARY KEY (zet_id, asset_id)
);
CREATE TABLE archive_zets_edges (
  zet_id TEXT NOT NULL, edge_type TEXT NOT NULL, target_id TEXT NOT NULL,
  PRIMARY KEY (zet_id, edge_type, target_id)
);
CREATE TABLE archive_receipts (
  id TEXT PRIMARY KEY, created_at TEXT NOT NULL,
  tool TEXT NOT NULL, model TEXT NOT NULL
);
CREATE TABLE archive_receipts_read (
  receipt_id TEXT NOT NULL, reference_id TEXT NOT NULL,
  PRIMARY KEY (receipt_id, reference_id)
);
CREATE TABLE archive_receipts_wrote (
  receipt_id TEXT NOT NULL, reference_id TEXT NOT NULL,
  PRIMARY KEY (receipt_id, reference_id)
);
