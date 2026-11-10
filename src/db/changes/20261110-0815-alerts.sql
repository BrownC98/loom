CREATE TABLE alerts (
    id INTEGER PRIMARY KEY,
    node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    wait TEXT NOT NULL
);
CREATE INDEX alerts_node ON alerts(node);
