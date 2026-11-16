CREATE TABLE chats (
  id TEXT PRIMARY KEY NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE chat_messages (
  id TEXT PRIMARY KEY NOT NULL,
  chat_id TEXT NOT NULL REFERENCES chats(id),
  role TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
  text TEXT NOT NULL,
  created_at TEXT NOT NULL,
  provider TEXT,
  model TEXT
);
CREATE INDEX chat_messages_chat_id ON chat_messages(chat_id);
