CREATE TABLE chat_attachments (
  receipt_id TEXT PRIMARY KEY NOT NULL,
  chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  asset_id TEXT NOT NULL REFERENCES momso_source_assets(id),
  message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE
);
CREATE INDEX chat_attachments_pending ON chat_attachments(chat_id, message_id);
