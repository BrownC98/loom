CREATE TABLE closed_waits (
    seq INTEGER PRIMARY KEY,
    node INTEGER NOT NULL,
    name TEXT NOT NULL,
    wait TEXT NOT NULL,
    closed_at INTEGER NOT NULL,
    how TEXT NOT NULL
);
CREATE INDEX closed_waits_at ON closed_waits(closed_at);
