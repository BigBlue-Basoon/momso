CREATE TABLE chat_claude_sessions (
 chat_id TEXT PRIMARY KEY REFERENCES chats(id),
 session_id TEXT NOT NULL,
 usage_json TEXT
);
