CREATE TABLE media_generation (
  asset_id TEXT PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('image', 'video')),
  prompt TEXT NOT NULL,
  provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude', 'gemini')),
  model TEXT,
  created_at TEXT,
  chat_id TEXT NOT NULL,
  message_id TEXT,
  version_group_id TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  parent_asset_id TEXT,
  UNIQUE(version_group_id, version)
);
CREATE INDEX media_generation_prompt ON media_generation(chat_id, kind, prompt);
