-- AUTOINCREMENT prevents reuse of committed IDs, even after deleting every todo.
CREATE TABLE todos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL CHECK (length(trim(text)) > 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1))
);
