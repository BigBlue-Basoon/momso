CREATE VIRTUAL TABLE chat_message_search USING fts5(
  text,
  content='chat_messages',
  content_rowid='rowid',
  detail=none,
  tokenize='trigram case_sensitive 1'
);
CREATE VIRTUAL TABLE chat_message_search_vocab USING fts5vocab(chat_message_search, 'row');

CREATE TABLE chat_message_search_map (
  message_rowid INTEGER PRIMARY KEY NOT NULL,
  chat_id TEXT NOT NULL
);
INSERT INTO chat_message_search_map(message_rowid,chat_id)
SELECT rowid,chat_id FROM chat_messages;

INSERT INTO chat_message_search(chat_message_search) VALUES('rebuild');

CREATE TRIGGER chat_message_search_insert AFTER INSERT ON chat_messages BEGIN
  INSERT INTO chat_message_search(rowid, text) VALUES (new.rowid, new.text);
  INSERT INTO chat_message_search_map(message_rowid, chat_id) VALUES (new.rowid, new.chat_id);
END;
CREATE TRIGGER chat_message_search_delete AFTER DELETE ON chat_messages BEGIN
  INSERT INTO chat_message_search(chat_message_search, rowid, text) VALUES ('delete', old.rowid, old.text);
  DELETE FROM chat_message_search_map WHERE message_rowid=old.rowid;
END;
CREATE TRIGGER chat_message_search_move AFTER UPDATE OF chat_id ON chat_messages BEGIN
  UPDATE chat_message_search_map SET chat_id=new.chat_id WHERE message_rowid=new.rowid;
END;
CREATE TRIGGER chat_message_search_update AFTER UPDATE OF text ON chat_messages BEGIN
  INSERT INTO chat_message_search(chat_message_search, rowid, text) VALUES ('delete', old.rowid, old.text);
  INSERT INTO chat_message_search(rowid, text) VALUES (new.rowid, new.text);
END;
