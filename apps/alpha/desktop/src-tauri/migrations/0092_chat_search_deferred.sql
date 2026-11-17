CREATE TABLE IF NOT EXISTS chat_search_index_state(id INTEGER PRIMARY KEY CHECK(id=1), last_rowid INTEGER NOT NULL, ready INTEGER NOT NULL);
INSERT OR IGNORE INTO chat_search_index_state(id,last_rowid,ready)
SELECT 1,0,CASE WHEN (SELECT count(*) FROM chat_messages)=(SELECT count(*) FROM chat_message_search_map) THEN 1 ELSE 0 END;
DROP TRIGGER IF EXISTS chat_message_search_insert;
DROP TRIGGER IF EXISTS chat_message_search_delete;
DROP TRIGGER IF EXISTS chat_message_search_move;
DROP TRIGGER IF EXISTS chat_message_search_update;
CREATE TRIGGER chat_message_search_insert AFTER INSERT ON chat_messages BEGIN
  INSERT INTO chat_message_search(rowid,text) VALUES(new.rowid,new.text);
  INSERT INTO chat_message_search_map(message_rowid,chat_id) VALUES(new.rowid,new.chat_id);
END;
CREATE TRIGGER chat_message_search_delete AFTER DELETE ON chat_messages WHEN EXISTS(SELECT 1 FROM chat_message_search_map WHERE message_rowid=old.rowid) BEGIN
  INSERT INTO chat_message_search(chat_message_search,rowid,text) VALUES('delete',old.rowid,old.text);
  DELETE FROM chat_message_search_map WHERE message_rowid=old.rowid;
END;
CREATE TRIGGER chat_message_search_move AFTER UPDATE OF chat_id ON chat_messages BEGIN
  UPDATE chat_message_search_map SET chat_id=new.chat_id WHERE message_rowid=new.rowid;
END;
CREATE TRIGGER chat_message_search_update AFTER UPDATE OF text ON chat_messages WHEN EXISTS(SELECT 1 FROM chat_message_search_map WHERE message_rowid=old.rowid) BEGIN
  INSERT INTO chat_message_search(chat_message_search,rowid,text) VALUES('delete',old.rowid,old.text);
  INSERT INTO chat_message_search(rowid,text) VALUES(new.rowid,new.text);
END;
