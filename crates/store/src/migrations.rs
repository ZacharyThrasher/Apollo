/// One forward-only schema step. `version` values must increase by one.
pub struct Migration {
    pub version: i64,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: "
        CREATE TABLE settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Single row: the persisted queue's scalar state, bound to an account.
        CREATE TABLE queue_state (
            singleton       INTEGER PRIMARY KEY CHECK (singleton = 1),
            account_key     TEXT    NOT NULL,
            next_id         INTEGER NOT NULL,
            shuffle         INTEGER NOT NULL CHECK (shuffle IN (0, 1)),
            seed            INTEGER NOT NULL,
            repeat          TEXT    NOT NULL CHECK (repeat IN ('off', 'one', 'all')),
            current_removed INTEGER NOT NULL DEFAULT 0 CHECK (current_removed IN (0, 1)),
            position_ms     INTEGER NOT NULL DEFAULT 0
        );

        -- One row per queue occurrence. Track IDs only: no URLs, leases or cookies.
        CREATE TABLE queue_entries (
            entry_id    INTEGER PRIMARY KEY,
            segment     TEXT    NOT NULL CHECK (segment IN ('history','current','manual','context','radio')),
            pos         INTEGER NOT NULL,
            origin      TEXT    NOT NULL CHECK (origin IN ('context','manual','radio')),
            video_id    TEXT    NOT NULL,
            title       TEXT    NOT NULL,
            album_title TEXT,
            album_id    TEXT,
            duration_ms INTEGER
        );
        CREATE UNIQUE INDEX queue_entries_order ON queue_entries (segment, pos);

        CREATE TABLE entry_artists (
            entry_id  INTEGER NOT NULL REFERENCES queue_entries(entry_id) ON DELETE CASCADE,
            pos       INTEGER NOT NULL,
            name      TEXT    NOT NULL,
            browse_id TEXT,
            PRIMARY KEY (entry_id, pos)
        );

        -- Canonical (unshuffled) context order, so toggling shuffle off is deterministic.
        CREATE TABLE queue_canonical (
            pos      INTEGER PRIMARY KEY,
            entry_id INTEGER NOT NULL
        );
    ",
},
Migration {
    version: 2,
    sql: "
        -- YouTube's measured loudness (dB against its reference level) per video, so the player
        -- request is made once per track. NULL = the service had no measurement; those rows are
        -- believed for a day (see apollo-services-common).
        CREATE TABLE loudness (
            video_id     TEXT PRIMARY KEY,
            loudness_db  REAL,
            fetched_unix INTEGER NOT NULL
        );
        CREATE INDEX loudness_fetched ON loudness (fetched_unix);
    ",
}, Migration {
    version: 3,
    sql: "
        -- Lyrics found (or not found) per video. `kind` says how to read `body`: LRC text for
        -- 'synced', plain text for 'plain', empty for 'none' (looked up, nothing there).
        CREATE TABLE lyrics_cache (
            video_id   TEXT PRIMARY KEY,
            kind       TEXT    NOT NULL CHECK (kind IN ('synced', 'plain', 'none')),
            body       TEXT    NOT NULL,
            fetched_at INTEGER NOT NULL
        );
    ",
}];
