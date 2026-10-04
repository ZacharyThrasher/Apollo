//! Page cache rows (see the `page_cache` table). The store keeps text under a page key and an
//! account; what the text means is the app's business.

use rusqlite::{params, OptionalExtension};

use crate::{Store, StoreError};

/// Pages kept per account. Past this the oldest answers go.
pub const MAX_PAGES_PER_ACCOUNT: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedPage {
    pub key: String,
    pub body: String,
    /// Unix seconds when the answer was fetched.
    pub fetched_at: i64,
}

impl Store {
    /// One page, if the account has it.
    pub fn get_page(&self, account_key: &str, page_key: &str) -> Result<Option<CachedPage>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT page_key, body, fetched_at FROM page_cache
                 WHERE account_key = ?1 AND page_key = ?2",
                params![account_key, page_key],
                row_to_page,
            )
            .optional()?)
    }

    /// Every page the account has, newest first.
    pub fn pages_for(&self, account_key: &str) -> Result<Vec<CachedPage>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT page_key, body, fetched_at FROM page_cache
             WHERE account_key = ?1 ORDER BY fetched_at DESC",
        )?;
        let rows = stmt.query_map([account_key], row_to_page)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Store a page, replacing the account's earlier answer, and drop the oldest pages beyond
    /// [`MAX_PAGES_PER_ACCOUNT`].
    pub fn put_page(
        &self,
        account_key: &str,
        page_key: &str,
        body: &str,
        fetched_at: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO page_cache(account_key, page_key, body, fetched_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_key, page_key) DO UPDATE SET
                 body = excluded.body, fetched_at = excluded.fetched_at",
            params![account_key, page_key, body, fetched_at],
        )?;
        self.conn.execute(
            "DELETE FROM page_cache WHERE account_key = ?1 AND page_key NOT IN (
                 SELECT page_key FROM page_cache WHERE account_key = ?1
                 ORDER BY fetched_at DESC, page_key LIMIT ?2)",
            params![account_key, MAX_PAGES_PER_ACCOUNT as i64],
        )?;
        Ok(())
    }

    /// Drop one page.
    pub fn delete_page(&self, account_key: &str, page_key: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM page_cache WHERE account_key = ?1 AND page_key = ?2",
            params![account_key, page_key],
        )?;
        Ok(())
    }

    /// Drop every page of every account (sign-out).
    pub fn clear_pages(&self) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM page_cache", [])?;
        Ok(())
    }

    /// Drop every page that is not `account_key`'s: a different account is in charge now.
    pub fn clear_pages_except(&self, account_key: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM page_cache WHERE account_key <> ?1",
            [account_key],
        )?;
        Ok(())
    }
}

fn row_to_page(r: &rusqlite::Row<'_>) -> rusqlite::Result<CachedPage> {
    Ok(CachedPage {
        key: r.get(0)?,
        body: r.get(1)?,
        fetched_at: r.get(2)?,
    })
}
