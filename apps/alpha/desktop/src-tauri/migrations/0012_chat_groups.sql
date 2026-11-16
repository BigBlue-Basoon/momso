CREATE TABLE chat_groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  position INTEGER NOT NULL,
  created_at TEXT NOT NULL
);
ALTER TABLE chats ADD COLUMN group_id TEXT REFERENCES chat_groups(id) ON DELETE SET NULL;
CREATE INDEX chat_groups_position ON chat_groups(position);
CREATE INDEX chats_group_id ON chats(group_id);
