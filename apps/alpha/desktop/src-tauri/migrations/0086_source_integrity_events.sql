CREATE TABLE source_integrity_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  created_at TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('changed', 'unavailable')),
  changes_json TEXT NOT NULL
);
CREATE INDEX source_integrity_events_chat_id_id ON source_integrity_events(chat_id, id);
