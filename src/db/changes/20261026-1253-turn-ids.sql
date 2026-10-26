ALTER TABLE nodes ADD COLUMN turn TEXT;
CREATE TABLE ended_turns (
    node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    turn TEXT NOT NULL,
    PRIMARY KEY (node, turn)
);
