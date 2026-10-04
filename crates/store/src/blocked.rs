//! Blocked artists: the artists whose songs radio, autoplay and Home's personal shelves skip
//! (see the `blocked_artists` table). Kept per account, like the Crate: reading it for another
//! account purges what was there, and sign-out purges it all.

use apollo_domain::blocklist::BlockedArtist;
use rusqlite::params;

use crate::{Store, StoreError};

/// What identifies a blocked artist inside one account: its channel id, or its name in lowercase.
pub fn entry_key(artist: &BlockedArtist) -> String {
    match &artist.id {
        Some(id) => id.clone(),
        None => format!("name:{}", artist.name.trim().to_lowercase()),
    }
}

impl Store {
    /// The blocked artists of `account_key`, newest first. Rows of any other account are deleted:
    /// private state never crosses identities.
    pub fn blocked_load(&mut self, account_key: &str) -> Result<Vec<BlockedArtist>, StoreError> {
        self.conn.execute(
            "DELETE FROM blocked_artists WHERE account_key <> ?1",
            [account_key],
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT browse_id, name FROM blocked_artists
             WHERE account_key = ?1 ORDER BY blocked_ms DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([account_key], |r| {
            Ok(BlockedArtist {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Block an artist. One already blocked stays as it is.
    pub fn blocked_add(
        &mut self,
        account_key: &str,
        artist: &BlockedArtist,
        blocked_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO blocked_artists
                (account_key, entry_key, browse_id, name, blocked_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account_key,
                entry_key(artist),
                artist.id,
                artist.name,
                blocked_ms
            ],
        )?;
        Ok(())
    }

    pub fn blocked_remove(
        &mut self,
        account_key: &str,
        artist: &BlockedArtist,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM blocked_artists WHERE account_key = ?1 AND entry_key = ?2",
            params![account_key, entry_key(artist)],
        )?;
        Ok(())
    }

    /// Forget every account's blocked artists (sign-out).
    pub fn blocked_purge(&mut self) -> Result<(), StoreError> {
        self.conn.execute_batch("DELETE FROM blocked_artists;")?;
        Ok(())
    }
}
