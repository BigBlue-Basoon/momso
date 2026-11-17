CREATE TABLE IF NOT EXISTS chat_redaction_preserved (
    chat_id TEXT NOT NULL,
    field_kind TEXT NOT NULL,
    field_id TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    safe_text TEXT NOT NULL,
    redaction_count INTEGER NOT NULL,
    PRIMARY KEY (chat_id, field_kind, field_id),
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);
