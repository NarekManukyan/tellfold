-- One row per UTC day and app version: how many update checks arrived. Nothing else is stored.
CREATE TABLE IF NOT EXISTS checks (
    day     TEXT    NOT NULL,
    version TEXT    NOT NULL,
    n       INTEGER NOT NULL,
    PRIMARY KEY (day, version)
);
