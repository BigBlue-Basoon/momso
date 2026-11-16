CREATE TABLE archive_location_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    path TEXT NOT NULL,
    initialized INTEGER NOT NULL DEFAULT 0 CHECK (initialized IN (0, 1))
);
