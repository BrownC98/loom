CREATE TABLE kept_images (
    wait INTEGER NOT NULL,
    part TEXT NOT NULL,
    idx INTEGER NOT NULL,
    name TEXT NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (wait, part, idx)
);
