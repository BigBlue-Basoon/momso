ALTER TABLE chat_messages ADD COLUMN fast INTEGER CHECK (fast IS NULL OR fast IN (0,1));
ALTER TABLE chat_messages ADD COLUMN not_handed_over INTEGER CHECK (not_handed_over IS NULL OR not_handed_over >= 0);
ALTER TABLE chat_messages ADD COLUMN retryable_fast INTEGER NOT NULL DEFAULT 0 CHECK (retryable_fast IN (0,1));
