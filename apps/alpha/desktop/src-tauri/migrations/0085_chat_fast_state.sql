ALTER TABLE chat_messages ADD COLUMN fast_state TEXT CHECK (fast_state IS NULL OR fast_state IN ('on', 'off', 'cooldown'));
