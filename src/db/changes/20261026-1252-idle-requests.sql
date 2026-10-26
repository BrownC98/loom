CREATE TABLE idle_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node INTEGER NOT NULL UNIQUE REFERENCES nodes(id) ON DELETE CASCADE,
    old INTEGER NOT NULL DEFAULT 0
);
INSERT INTO idle_requests (node, old)
    SELECT s.node, 1 FROM agent_states s JOIN nodes n ON n.id = s.node
    WHERE n.server IS NULL AND instr(s.state, '"kind":"listening"') > 0;
