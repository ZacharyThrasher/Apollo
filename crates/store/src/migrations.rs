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
}, Migration {
    version: 4,
    sql: "
        -- The last answer for a page (Home, the library lists, an album, artist or playlist), so
        -- reopening one draws at once and refreshes behind it. `body` is the page serialized by
        -- the app; the store never reads it. Rows belong to one account and are purged when the
        -- account changes or signs out.
        CREATE TABLE page_cache (
            account_key TEXT    NOT NULL,
            page_key    TEXT    NOT NULL,
            body        TEXT    NOT NULL,
            fetched_at  INTEGER NOT NULL,
            PRIMARY KEY (account_key, page_key)
        );
    ",
}, Migration {
    version: 5,
    sql: "
        -- Where the saved queue came from (shown as \"Playing from ...\"), and where autoplay
        -- took over from it. All NULL when the queue was put together by hand.
        ALTER TABLE queue_state ADD COLUMN source_kind   TEXT;
        ALTER TABLE queue_state ADD COLUMN source_name   TEXT;
        ALTER TABLE queue_state ADD COLUMN source_target TEXT;
        ALTER TABLE queue_state ADD COLUMN source_thumb  TEXT;
        ALTER TABLE queue_state ADD COLUMN autoplay_first INTEGER;
        ALTER TABLE queue_state ADD COLUMN autoplay_seed  TEXT;
    ",
}, Migration {
    version: 6,
    sql: "
        -- The Crate: songs set aside to remember, per account. Track fields only.
        CREATE TABLE crate_items (
            account_key TEXT    NOT NULL,
            video_id    TEXT    NOT NULL,
            title       TEXT    NOT NULL,
            album_title TEXT,
            album_id    TEXT,
            duration_ms INTEGER,
            thumbnail   TEXT,
            added_ms    INTEGER NOT NULL,
            PRIMARY KEY (account_key, video_id)
        );
        CREATE TABLE crate_artists (
            account_key TEXT    NOT NULL,
            video_id    TEXT    NOT NULL,
            pos         INTEGER NOT NULL,
            name        TEXT    NOT NULL,
            browse_id   TEXT,
            PRIMARY KEY (account_key, video_id, pos),
            FOREIGN KEY (account_key, video_id)
                REFERENCES crate_items (account_key, video_id) ON DELETE CASCADE
        );
    ",
}, Migration {
    version: 7,
    sql: "
        -- One row per play that counted (the app decides when: about half a track or 30 s),
        -- for the local stats and Home's personal shelves. Nothing here leaves the machine.
        -- `artists` is the credited artists as JSON the app reads and writes; the store only
        -- groups by `video_id`. `art` is the cover address the app showed for the track.
        CREATE TABLE plays (
            id          INTEGER PRIMARY KEY,
            account_key TEXT    NOT NULL,
            played_unix INTEGER NOT NULL,
            video_id    TEXT    NOT NULL,
            title       TEXT    NOT NULL,
            artists     TEXT    NOT NULL,
            album_title TEXT,
            album_id    TEXT,
            duration_ms INTEGER,
            art         TEXT,
            listened_ms INTEGER NOT NULL
        );
        CREATE INDEX plays_by_account_time ON plays (account_key, played_unix);
        CREATE INDEX plays_by_account_video ON plays (account_key, video_id);
    ",
}, Migration {
    version: 8,
    sql: "
        -- Artists the owner told Apollo not to play from radio, autoplay and Home's personal
        -- shelves, per account. Local only: nothing here is sent anywhere. `entry_key` is the
        -- artist's channel id, or the lowercased name for an artist known only by name.
        CREATE TABLE blocked_artists (
            account_key TEXT    NOT NULL,
            entry_key   TEXT    NOT NULL,
            browse_id   TEXT,
            name        TEXT    NOT NULL,
            blocked_ms  INTEGER NOT NULL,
            PRIMARY KEY (account_key, entry_key)
        );
    ",
}, Migration {
    version: 9,
    sql: "
        -- The Crate is arranged by hand: `pos` is a song's place in it, 0 at the top. A new song
        -- goes below the last one. Songs already there keep the order the page showed so far
        -- (newest on top).
        ALTER TABLE crate_items ADD COLUMN pos INTEGER NOT NULL DEFAULT 0;
        UPDATE crate_items SET pos = (
            SELECT COUNT(*) FROM crate_items AS newer
            WHERE newer.account_key = crate_items.account_key
              AND (newer.added_ms > crate_items.added_ms
                   OR (newer.added_ms = crate_items.added_ms AND newer.rowid > crate_items.rowid))
        );
    ",
}, Migration {
    version: 10,
    sql: "
        -- Whether the service marks a Crate song explicit (the E badge), as far as it is known.
        -- 0 is 'not known to be': a song added from a place that does not say stays 0 until a
        -- swap to explicit finds out.
        ALTER TABLE crate_items ADD COLUMN explicit INTEGER NOT NULL DEFAULT 0;
    ",
}];
