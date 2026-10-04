//! The local play log (see the `plays` table): one row per play that counted, kept per account
//! for the stats screen and Home's personal shelves. The app decides what counts as a play and
//! what `artists` holds; the store records, groups by track and never reads that text.

use rusqlite::params;

use crate::{Store, StoreError};

/// Rows kept per account. Past this the oldest plays go, so the table cannot grow without bound
/// (this many plays is years of listening).
pub const MAX_PLAYS_PER_ACCOUNT: i64 = 200_000;

/// A play to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPlay {
    /// Unix seconds when the play counted.
    pub played_unix: i64,
    pub video_id: String,
    pub title: String,
    /// The credited artists, as the app serialized them.
    pub artists: String,
    pub album_title: Option<String>,
    pub album_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub art: Option<String>,
    /// How much of the track had been listened to when the play counted.
    pub listened_ms: u64,
}

/// What the log says about one track over a period: how often it was played and when last, with
/// the details of its latest play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayTotal {
    pub video_id: String,
    pub title: String,
    pub artists: String,
    pub album_title: Option<String>,
    pub album_id: Option<String>,
    pub duration_ms: Option<u64>,
    pub art: Option<String>,
    pub plays: u64,
    pub listened_ms: u64,
    /// Unix seconds of the latest play in the period.
    pub last_played: i64,
}

fn to_i64(v: u64, what: &'static str) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::OutOfRange(what))
}

impl Store {
    /// Records a play for `account_key`; returns the row's id, to update its listening time.
    pub fn record_play(&self, account_key: &str, play: &NewPlay) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO plays(account_key, played_unix, video_id, title, artists,
                               album_title, album_id, duration_ms, art, listened_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                account_key,
                play.played_unix,
                play.video_id,
                play.title,
                play.artists,
                play.album_title,
                play.album_id,
                play.duration_ms
                    .map(|d| to_i64(d, "play duration"))
                    .transpose()?,
                play.art,
                to_i64(play.listened_ms, "listened time")?,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM plays WHERE account_key = ?1 AND id NOT IN
                 (SELECT id FROM plays WHERE account_key = ?1
                  ORDER BY played_unix DESC, id DESC LIMIT ?2)",
            params![account_key, MAX_PLAYS_PER_ACCOUNT],
        )?;
        Ok(id)
    }

    /// The play's final listening time, once the track was left.
    pub fn set_listened(&self, id: i64, listened_ms: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE plays SET listened_ms = ?2 WHERE id = ?1",
            params![id, to_i64(listened_ms, "listened time")?],
        )?;
        Ok(())
    }

    /// Per track, the plays of `account_key` at or after `since_unix` (all of them for `None`).
    /// The details come from each track's latest play in the period. Most played first, then
    /// latest.
    pub fn play_totals(
        &self,
        account_key: &str,
        since_unix: Option<i64>,
    ) -> Result<Vec<PlayTotal>, StoreError> {
        // The bare columns next to MAX(played_unix) come from the row that holds the maximum.
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, artists, album_title, album_id, duration_ms, art,
                    COUNT(*), SUM(listened_ms), MAX(played_unix)
             FROM plays
             WHERE account_key = ?1 AND played_unix >= ?2
             GROUP BY video_id
             ORDER BY COUNT(*) DESC, MAX(played_unix) DESC, video_id",
        )?;
        let rows = stmt.query_map(params![account_key, since_unix.unwrap_or(i64::MIN)], |r| {
            Ok(PlayTotal {
                video_id: r.get(0)?,
                title: r.get(1)?,
                artists: r.get(2)?,
                album_title: r.get(3)?,
                album_id: r.get(4)?,
                duration_ms: r
                    .get::<_, Option<i64>>(5)?
                    .and_then(|d| u64::try_from(d).ok()),
                art: r.get(6)?,
                plays: u64::try_from(r.get::<_, i64>(7)?).unwrap_or(0),
                listened_ms: u64::try_from(r.get::<_, i64>(8)?).unwrap_or(0),
                last_played: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(video: &str, at: i64, listened_ms: u64) -> NewPlay {
        NewPlay {
            played_unix: at,
            video_id: video.into(),
            title: format!("Title {video} @{at}"),
            artists: "[]".into(),
            album_title: Some("Album".into()),
            album_id: Some("MPREb_1".into()),
            duration_ms: Some(200_000),
            art: Some("https://example.com/a.jpg".into()),
            listened_ms,
        }
    }

    #[test]
    fn totals_group_by_track_and_take_the_latest_details() {
        let store = Store::open_in_memory().unwrap();
        store.record_play("a", &play("x", 100, 30_000)).unwrap();
        store.record_play("a", &play("y", 150, 30_000)).unwrap();
        store.record_play("a", &play("x", 200, 40_000)).unwrap();
        let totals = store.play_totals("a", None).unwrap();
        assert_eq!(totals.len(), 2);
        let x = &totals[0];
        assert_eq!((x.video_id.as_str(), x.plays, x.listened_ms), ("x", 2, 70_000));
        assert_eq!(x.last_played, 200);
        assert_eq!(x.title, "Title x @200", "details come from the latest play");
        assert_eq!(totals[1].video_id, "y");
    }

    #[test]
    fn a_period_leaves_out_older_plays_and_other_accounts() {
        let store = Store::open_in_memory().unwrap();
        store.record_play("a", &play("x", 100, 1)).unwrap();
        store.record_play("a", &play("x", 300, 1)).unwrap();
        store.record_play("b", &play("x", 300, 1)).unwrap();
        let totals = store.play_totals("a", Some(200)).unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].plays, 1);
        assert!(store.play_totals("c", None).unwrap().is_empty());
    }

    #[test]
    fn the_listening_time_can_be_settled_afterwards() {
        let store = Store::open_in_memory().unwrap();
        let id = store.record_play("a", &play("x", 100, 30_000)).unwrap();
        store.set_listened(id, 190_000).unwrap();
        assert_eq!(store.play_totals("a", None).unwrap()[0].listened_ms, 190_000);
    }

    #[test]
    fn ties_go_to_the_more_recent_track() {
        let store = Store::open_in_memory().unwrap();
        store.record_play("a", &play("old", 100, 1)).unwrap();
        store.record_play("a", &play("new", 900, 1)).unwrap();
        let totals = store.play_totals("a", None).unwrap();
        assert_eq!(totals[0].video_id, "new");
    }
}
