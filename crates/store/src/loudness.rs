//! Cache of YouTube's loudness measurement per video (volume normalization).
//!
//! Only a video id, a number and a timestamp: no URLs, no titles, nothing about the account.

use crate::{Store, StoreError};

/// Rows kept. The oldest are dropped past this, so the table cannot grow without bound.
const MAX_ROWS: i64 = 5000;

/// One cached answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessRow {
    /// `None` = the service had no measurement for the video.
    pub loudness_db: Option<f32>,
    pub fetched_unix: u64,
}

impl Store {
    pub fn get_loudness(&self, video_id: &str) -> Result<Option<LoudnessRow>, StoreError> {
        match self.conn.query_row(
            "SELECT loudness_db, fetched_unix FROM loudness WHERE video_id = ?1",
            [video_id],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, i64>(1)?)),
        ) {
            Ok((db, at)) => Ok(Some(LoudnessRow {
                loudness_db: db.filter(|d| d.is_finite()).map(|d| d as f32),
                fetched_unix: u64::try_from(at).unwrap_or(0),
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn put_loudness(&self, video_id: &str, row: LoudnessRow) -> Result<(), StoreError> {
        let fetched = i64::try_from(row.fetched_unix)
            .map_err(|_| StoreError::OutOfRange("loudness timestamp"))?;
        self.conn.execute(
            "INSERT INTO loudness(video_id, loudness_db, fetched_unix) VALUES (?1, ?2, ?3)
             ON CONFLICT(video_id) DO UPDATE SET
                 loudness_db = excluded.loudness_db, fetched_unix = excluded.fetched_unix",
            rusqlite::params![
                video_id,
                row.loudness_db.filter(|d| d.is_finite()).map(f64::from),
                fetched
            ],
        )?;
        self.conn.execute(
            "DELETE FROM loudness WHERE video_id IN
                 (SELECT video_id FROM loudness ORDER BY fetched_unix DESC, video_id LIMIT -1 OFFSET ?1)",
            [MAX_ROWS],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measurement_and_an_absence_both_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_loudness("a").unwrap(), None);
        store
            .put_loudness(
                "a",
                LoudnessRow {
                    loudness_db: Some(-3.25),
                    fetched_unix: 100,
                },
            )
            .unwrap();
        store
            .put_loudness(
                "b",
                LoudnessRow {
                    loudness_db: None,
                    fetched_unix: 200,
                },
            )
            .unwrap();
        assert_eq!(
            store.get_loudness("a").unwrap(),
            Some(LoudnessRow {
                loudness_db: Some(-3.25),
                fetched_unix: 100
            })
        );
        assert_eq!(
            store.get_loudness("b").unwrap(),
            Some(LoudnessRow {
                loudness_db: None,
                fetched_unix: 200
            })
        );
    }

    #[test]
    fn a_newer_answer_replaces_the_old_one() {
        let store = Store::open_in_memory().unwrap();
        store
            .put_loudness(
                "a",
                LoudnessRow {
                    loudness_db: None,
                    fetched_unix: 1,
                },
            )
            .unwrap();
        store
            .put_loudness(
                "a",
                LoudnessRow {
                    loudness_db: Some(2.0),
                    fetched_unix: 9,
                },
            )
            .unwrap();
        assert_eq!(
            store.get_loudness("a").unwrap(),
            Some(LoudnessRow {
                loudness_db: Some(2.0),
                fetched_unix: 9
            })
        );
    }

    #[test]
    fn the_table_is_bounded_and_keeps_the_newest() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..(MAX_ROWS as u64 + 20) {
            store
                .put_loudness(
                    &format!("v{i}"),
                    LoudnessRow {
                        loudness_db: Some(1.0),
                        fetched_unix: i,
                    },
                )
                .unwrap();
        }
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM loudness", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, MAX_ROWS);
        assert_eq!(store.get_loudness("v0").unwrap(), None);
        assert!(store
            .get_loudness(&format!("v{}", MAX_ROWS as u64 + 19))
            .unwrap()
            .is_some());
    }
}
