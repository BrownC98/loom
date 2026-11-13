CREATE TABLE doc_authors (
    doc TEXT PRIMARY KEY,
    node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE
);
CREATE TABLE doc_talk (
    id INTEGER PRIMARY KEY,
    doc TEXT NOT NULL,
    said TEXT NOT NULL,
    name TEXT NOT NULL,
    text TEXT NOT NULL,
    message INTEGER,
    wait INTEGER,
    at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX doc_talk_doc ON doc_talk(doc);
CREATE INDEX doc_talk_message ON doc_talk(message) WHERE message IS NOT NULL;
