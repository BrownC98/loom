CREATE TABLE guarded_commands (
    node INTEGER NOT NULL REFERENCES node_hosts(node) ON DELETE CASCADE,
    pid INTEGER NOT NULL,
    birth INTEGER NOT NULL,
    PRIMARY KEY (node, pid, birth)
);
