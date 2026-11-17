CREATE VIRTUAL TABLE chat_message_search USING fts5(text, content='chat_messages', content_rowid='rowid', detail=none, tokenize='trigram case_sensitive 1');
CREATE VIRTUAL TABLE chat_message_search_vocab USING fts5vocab(chat_message_search, 'row');
CREATE TABLE chat_message_search_map(message_rowid INTEGER PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL);
