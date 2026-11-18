CREATE TABLE generated_output_turns (
    turn_id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    parent_asset_id TEXT,
    model TEXT,
    model_ambiguous INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0
);
