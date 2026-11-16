CREATE TABLE chat_context_state (
  chat_id TEXT PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  model TEXT,
  used_tokens INTEGER,
  limit_tokens INTEGER,
  updated_at TEXT NOT NULL
);
