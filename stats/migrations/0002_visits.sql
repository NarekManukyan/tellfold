-- One row per UTC day, page or link, and source: how many visits or clicks. Nothing else is stored.
CREATE TABLE IF NOT EXISTS visits (
    day    TEXT    NOT NULL,
    name   TEXT    NOT NULL,
    source TEXT    NOT NULL,
    n      INTEGER NOT NULL,
    PRIMARY KEY (day, name, source)
);
