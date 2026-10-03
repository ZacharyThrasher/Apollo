//! Lyrics cache rows (see the `lyrics_cache` table). The store keeps text and a kind; what the
//! text means is the lyrics crate's business.

use rusqlite::{params, OptionalExtension};

use crate::{Store, StoreError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedLyrics {
    /// `synced`, `plain` or `none`.
    pub kind: String,
    pub body: String,
    /// Unix seconds.
    pub fetched_at: i64,
}

impl Store {
    pub fn get_lyrics(&self, video_id: &str) -> Result<Option<CachedLyrics>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT kind, body, fetched_at FROM lyrics_cache WHERE video_id = ?1",
                [video_id],
                |r| {
                    Ok(CachedLyrics {
                        kind: r.get(0)?,
                        body: r.get(1)?,
                        fetched_at: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn put_lyrics(
        &self,
        video_id: &str,
        kind: &str,
        body: &str,
        fetched_at: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO lyrics_cache(video_id, kind, body, fetched_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(video_id) DO UPDATE SET
                 kind = excluded.kind, body = excluded.body, fetched_at = excluded.fetched_at",
            params![video_id, kind, body, fetched_at],
        )?;
        Ok(())
    }
}
