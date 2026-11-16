CREATE TABLE IF NOT EXISTS chat_imports (
  chat_id TEXT PRIMARY KEY NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  source_provider TEXT NOT NULL,
  source_id TEXT NOT NULL,
  UNIQUE(source_provider, source_id)
);
CREATE TABLE IF NOT EXISTS provider_context_limits (
  chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  provider TEXT NOT NULL CHECK (provider IN ('claude', 'gemini')),
  model TEXT NOT NULL,
  limit_tokens INTEGER NOT NULL CHECK (limit_tokens > 0),
  used_tokens INTEGER NOT NULL CHECK (used_tokens >= 0),
  updated_at TEXT NOT NULL,
  PRIMARY KEY (chat_id, provider, model)
);
