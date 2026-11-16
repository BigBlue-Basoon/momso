CREATE TABLE chat_provider_handoff (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    delivered_rowid INTEGER NOT NULL,
    PRIMARY KEY (chat_id, provider)
);
