CREATE TABLE IF NOT EXISTS chat_archive_changes (
  chat_id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1
);

INSERT INTO chat_archive_changes(chat_id,revision) SELECT id,1 FROM chats WHERE 1
ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
INSERT INTO app_runtime_meta(key,value,updated_at) VALUES('chat_archive_tracking_version','1',CURRENT_TIMESTAMP)
ON CONFLICT(key) DO UPDATE SET value='1',updated_at=CURRENT_TIMESTAMP;

CREATE TRIGGER IF NOT EXISTS chat_archive_room_insert AFTER INSERT ON chats BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(new.id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_room_update AFTER UPDATE ON chats BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(new.id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_room_delete AFTER DELETE ON chats BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_message_insert AFTER INSERT ON chat_messages BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(new.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_message_update AFTER UPDATE ON chat_messages BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
  INSERT INTO chat_archive_changes(chat_id,revision) SELECT new.chat_id,1 WHERE new.chat_id != old.chat_id
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_message_delete AFTER DELETE ON chat_messages BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_import_insert AFTER INSERT ON chat_imports BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(new.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_import_update AFTER UPDATE ON chat_imports BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
  INSERT INTO chat_archive_changes(chat_id,revision) SELECT new.chat_id,1 WHERE new.chat_id != old.chat_id
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_import_delete AFTER DELETE ON chat_imports BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_attachment_insert AFTER INSERT ON chat_attachments BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(new.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_attachment_update AFTER UPDATE ON chat_attachments BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
  INSERT INTO chat_archive_changes(chat_id,revision) SELECT new.chat_id,1 WHERE new.chat_id != old.chat_id
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_attachment_delete AFTER DELETE ON chat_attachments BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision) VALUES(old.chat_id,1)
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
CREATE TRIGGER IF NOT EXISTS chat_archive_group_update AFTER UPDATE ON chat_groups BEGIN
  INSERT INTO chat_archive_changes(chat_id,revision)
  SELECT id,1 FROM chats WHERE group_id=new.id
  ON CONFLICT(chat_id) DO UPDATE SET revision=revision+1;
END;
