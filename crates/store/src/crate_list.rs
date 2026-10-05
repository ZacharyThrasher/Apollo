//! The Crate: a scratch list of songs to remember, kept per account (see the `crate_items`
//! table). Track fields only, like the saved queue: no URLs, leases or cookies. Like the queue
//! it is private to one account: reading it for another account purges what was there, and
//! sign-out purges it all.

use std::time::Duration;

use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track, VideoId};
use rusqlite::{params, OptionalExtension};

use crate::{Store, StoreError};

/// One song in the crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateRow {
    pub track: Track,
    /// The cover URL the row that added it knew, so the song has art when it is listed.
    pub thumbnail: Option<String>,
    /// When it was added, in milliseconds since the Unix epoch.
    pub added_ms: i64,
}

impl Store {
    /// The crate of `account_key`, top to bottom as arranged. Rows of any other account are
    /// deleted: private state never crosses identities.
    pub fn crate_load(&mut self, account_key: &str) -> Result<Vec<CrateRow>, StoreError> {
        self.conn.execute(
            "DELETE FROM crate_items WHERE account_key <> ?1",
            [account_key],
        )?;
        let mut items = self.conn.prepare(
            "SELECT video_id, title, album_title, album_id, duration_ms, thumbnail, added_ms
             FROM crate_items WHERE account_key = ?1 ORDER BY pos, rowid",
        )?;
        let mut artists = self.conn.prepare(
            "SELECT name, browse_id FROM crate_artists
             WHERE account_key = ?1 AND video_id = ?2 ORDER BY pos",
        )?;
        let mut rows = items.query([account_key])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let video_id: String = r.get(0)?;
            let album_title: Option<String> = r.get(2)?;
            let album_id: Option<String> = r.get(3)?;
            let duration_ms: Option<i64> = r.get(4)?;
            let found = artists.query_map(params![account_key, video_id], |a| {
                Ok(ArtistRef {
                    name: a.get(0)?,
                    id: a.get::<_, Option<String>>(1)?.map(BrowseId::new),
                })
            })?;
            out.push(CrateRow {
                track: Track {
                    video_id: VideoId::new(video_id),
                    title: r.get(1)?,
                    artists: found.collect::<Result<_, _>>()?,
                    album: album_title.map(|title| AlbumRef {
                        title,
                        id: album_id.map(BrowseId::new),
                    }),
                    duration: duration_ms
                        .map(|ms| {
                            u64::try_from(ms)
                                .map_err(|_| StoreError::Corrupt("negative duration"))
                        })
                        .transpose()?
                        .map(Duration::from_millis),
                },
                thumbnail: r.get(5)?,
                added_ms: r.get(6)?,
            });
        }
        Ok(out)
    }

    /// Add a song at the bottom. One already in the crate stays where it is.
    pub fn crate_add(&mut self, account_key: &str, row: &CrateRow) -> Result<(), StoreError> {
        let t = &row.track;
        let tx = self.conn.transaction()?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM crate_items WHERE account_key = ?1 AND video_id = ?2",
                params![account_key, t.video_id.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            tx.execute(
                "INSERT INTO crate_items
                    (account_key, video_id, title, album_title, album_id, duration_ms, thumbnail,
                     added_ms, pos)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                         (SELECT COALESCE(MAX(pos) + 1, 0) FROM crate_items WHERE account_key = ?1))",
                params![
                    account_key,
                    t.video_id.as_str(),
                    t.title,
                    t.album.as_ref().map(|a| a.title.as_str()),
                    t.album
                        .as_ref()
                        .and_then(|a| a.id.as_ref())
                        .map(|b| b.as_str()),
                    t.duration
                        .map(|d| d.as_millis().min(i64::MAX as u128) as i64),
                    row.thumbnail,
                    row.added_ms,
                ],
            )?;
            for (i, a) in t.artists.iter().enumerate() {
                tx.execute(
                    "INSERT INTO crate_artists (account_key, video_id, pos, name, browse_id)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        account_key,
                        t.video_id.as_str(),
                        i as i64,
                        a.name,
                        a.id.as_ref().map(|b| b.as_str())
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Put the crate in this order: the songs of `video_ids` take places 0, 1, 2… top to bottom.
    /// A song not named keeps its old place, so a stale list can never lose one.
    pub fn crate_reorder(
        &mut self,
        account_key: &str,
        video_ids: &[String],
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        {
            let mut set = tx.prepare(
                "UPDATE crate_items SET pos = ?3 WHERE account_key = ?1 AND video_id = ?2",
            )?;
            for (at, id) in video_ids.iter().enumerate() {
                set.execute(params![account_key, id, at as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn crate_remove(&mut self, account_key: &str, video_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM crate_items WHERE account_key = ?1 AND video_id = ?2",
            params![account_key, video_id],
        )?;
        Ok(())
    }

    pub fn crate_clear(&mut self, account_key: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM crate_items WHERE account_key = ?1", [account_key])?;
        Ok(())
    }

    /// Forget every account's crate (sign-out).
    pub fn crate_purge(&mut self) -> Result<(), StoreError> {
        self.conn.execute_batch("DELETE FROM crate_items;")?;
        Ok(())
    }
}
