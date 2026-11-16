CREATE TABLE chat_imports (
  chat_id TEXT PRIMARY KEY NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  source_provider TEXT NOT NULL,
  source_id TEXT NOT NULL,
  UNIQUE(source_provider, source_id)
);
