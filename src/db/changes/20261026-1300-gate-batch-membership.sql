UPDATE gates SET state = '"failed"', batch = NULL, queued = NULL, seq = NULL
    WHERE state = '"merging"' AND (batch IS NULL OR batch NOT IN (SELECT id FROM batches));
UPDATE gates SET state = '"passed"' WHERE state = '"merging"';
UPDATE gates SET batch = NULL
    WHERE batch IS NOT NULL AND (state != '"passed"' OR batch NOT IN (SELECT id FROM batches));
ALTER TABLE batches DROP COLUMN started;
ALTER TABLE batches ADD COLUMN moved INTEGER NOT NULL DEFAULT 0;
CREATE TABLE gates_new (
    repo INTEGER NOT NULL,
    branch TEXT NOT NULL,
    owner INTEGER NOT NULL,
    gate INTEGER NOT NULL,
    target TEXT NOT NULL,
    worktree TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state != '"merging"'),
    head TEXT NOT NULL,
    opened INTEGER NOT NULL,
    queued INTEGER,
    seq INTEGER,
    batch INTEGER REFERENCES batches(id),
    PRIMARY KEY (repo, branch)
);
INSERT INTO gates_new (repo, branch, owner, gate, target, worktree, state, head, opened, queued, seq, batch)
    SELECT repo, branch, owner, gate, target, worktree, state, head, opened, queued, seq, batch FROM gates;
DROP TABLE gates;
ALTER TABLE gates_new RENAME TO gates;
