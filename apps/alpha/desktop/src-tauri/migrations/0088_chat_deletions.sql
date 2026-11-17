CREATE TABLE chat_deletions (
  chat_id TEXT PRIMARY KEY NOT NULL,
  deleted_at TEXT NOT NULL
);

INSERT INTO chat_deletions(chat_id, deleted_at)
SELECT substr(key, 21), strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM app_runtime_meta
WHERE key GLOB 'chat_archive_delete:*'
  AND NOT EXISTS (SELECT 1 FROM chats WHERE id = substr(key, 21));

CREATE TRIGGER prevent_deleted_chat_recreation
BEFORE INSERT ON chats
WHEN EXISTS (SELECT 1 FROM chat_deletions WHERE chat_id = NEW.id)
BEGIN
  SELECT RAISE(ABORT, 'chat was deleted');
END;
