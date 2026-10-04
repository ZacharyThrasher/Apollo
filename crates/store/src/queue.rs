use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, QueueEntryId, Track, VideoId};
use apollo_domain::queue::{
    AutoplayStart, Origin, Queue, QueueEntry, QueueSnapshot, QueueSource, Repeat, SourceKind,
};
use rusqlite::{params, Transaction};

use crate::{Store, StoreError};

fn to_i64(v: u64, what: &'static str) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::OutOfRange(what))
}

fn origin_str(o: Origin) -> &'static str {
    match o {
        Origin::Context => "context",
        Origin::Manual => "manual",
        Origin::Radio => "radio",
    }
}

fn origin_from(s: &str) -> Result<Origin, StoreError> {
    match s {
        "context" => Ok(Origin::Context),
        "manual" => Ok(Origin::Manual),
        "radio" => Ok(Origin::Radio),
        _ => Err(StoreError::Corrupt("unknown origin")),
    }
}

fn repeat_str(r: Repeat) -> &'static str {
    match r {
        Repeat::Off => "off",
        Repeat::One => "one",
        Repeat::All => "all",
    }
}

fn repeat_from(s: &str) -> Result<Repeat, StoreError> {
    match s {
        "off" => Ok(Repeat::Off),
        "one" => Ok(Repeat::One),
        "all" => Ok(Repeat::All),
        _ => Err(StoreError::Corrupt("unknown repeat mode")),
    }
}

fn kind_str(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Playlist => "playlist",
        SourceKind::Album => "album",
        SourceKind::Artist => "artist",
        SourceKind::Radio => "radio",
        SourceKind::LikedSongs => "liked",
        SourceKind::Search => "search",
        SourceKind::Other => "other",
    }
}

/// The saved source, if it is one this build knows: a source is only a heading, so an
/// unknown one is dropped rather than failing the whole queue.
fn source_from(
    (kind, name, target, thumbnail): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
) -> Result<Option<QueueSource>, StoreError> {
    let (Some(kind), Some(name)) = (kind, name) else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "playlist" => SourceKind::Playlist,
        "album" => SourceKind::Album,
        "artist" => SourceKind::Artist,
        "radio" => SourceKind::Radio,
        "liked" => SourceKind::LikedSongs,
        "search" => SourceKind::Search,
        "other" => SourceKind::Other,
        _ => return Ok(None),
    };
    Ok(Some(QueueSource {
        kind,
        name,
        target,
        thumbnail,
    }))
}

fn insert_entry(
    tx: &Transaction<'_>,
    segment: &str,
    pos: usize,
    e: &QueueEntry,
) -> Result<(), StoreError> {
    let t = &e.track;
    tx.execute(
        "INSERT INTO queue_entries
            (entry_id, segment, pos, origin, video_id, title, album_title, album_id, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            to_i64(e.id.get(), "entry id")?,
            segment,
            to_i64(pos as u64, "position")?,
            origin_str(e.origin),
            t.video_id.as_str(),
            t.title,
            t.album.as_ref().map(|a| a.title.as_str()),
            t.album
                .as_ref()
                .and_then(|a| a.id.as_ref())
                .map(|b| b.as_str()),
            t.duration
                .map(|d| d.as_millis().min(i64::MAX as u128) as i64),
        ],
    )?;
    for (i, a) in t.artists.iter().enumerate() {
        tx.execute(
            "INSERT INTO entry_artists (entry_id, pos, name, browse_id) VALUES (?1, ?2, ?3, ?4)",
            params![
                to_i64(e.id.get(), "entry id")?,
                i as i64,
                a.name,
                a.id.as_ref().map(|b| b.as_str())
            ],
        )?;
    }
    Ok(())
}

impl Store {
    /// Replace the persisted queue atomically. `account_key` is an opaque,
    /// non-secret identifier of the selected account/channel; it must not be
    /// derived from cookie material.
    pub fn save_queue(
        &mut self,
        account_key: &str,
        snap: &QueueSnapshot,
        position_ms: u64,
    ) -> Result<(), StoreError> {
        let next_id = to_i64(snap.next_id, "next id")?;
        let position = to_i64(position_ms, "position")?;
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM queue_canonical; DELETE FROM entry_artists;
             DELETE FROM queue_entries; DELETE FROM queue_state;",
        )?;
        tx.execute(
            "INSERT INTO queue_state
                (singleton, account_key, next_id, shuffle, seed, repeat, current_removed, position_ms,
                 source_kind, source_name, source_target, source_thumb,
                 autoplay_first, autoplay_seed)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                account_key,
                next_id,
                snap.shuffle as i64,
                snap.seed as i64, // bit-preserving
                repeat_str(snap.repeat),
                snap.current.as_ref().is_some_and(|(_, removed)| *removed) as i64,
                position,
                snap.source.as_ref().map(|s| kind_str(s.kind)),
                snap.source.as_ref().map(|s| s.name.as_str()),
                snap.source.as_ref().and_then(|s| s.target.as_deref()),
                snap.source.as_ref().and_then(|s| s.thumbnail.as_deref()),
                snap.autoplay
                    .as_ref()
                    .map(|a| to_i64(a.first.get(), "autoplay entry"))
                    .transpose()?,
                snap.autoplay.as_ref().map(|a| a.seed.as_str()),
            ],
        )?;
        for (i, e) in snap.history.iter().enumerate() {
            insert_entry(&tx, "history", i, e)?;
        }
        if let Some((e, _)) = &snap.current {
            insert_entry(&tx, "current", 0, e)?;
        }
        for (i, e) in snap.manual.iter().enumerate() {
            insert_entry(&tx, "manual", i, e)?;
        }
        for (i, e) in snap.context.iter().enumerate() {
            insert_entry(&tx, "context", i, e)?;
        }
        for (i, e) in snap.radio.iter().enumerate() {
            insert_entry(&tx, "radio", i, e)?;
        }
        for (i, id) in snap.canonical.iter().enumerate() {
            tx.execute(
                "INSERT INTO queue_canonical (pos, entry_id) VALUES (?1, ?2)",
                params![i as i64, to_i64(id.get(), "entry id")?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Load the persisted queue for `account_key`. A queue saved for a different
    /// account is purged and `None` returned: private state never crosses identities.
    pub fn load_queue(&mut self, account_key: &str) -> Result<Option<(Queue, u64)>, StoreError> {
        let row = self.conn.query_row(
            "SELECT account_key, next_id, shuffle, seed, repeat, current_removed, position_ms,
                    source_kind, source_name, source_target, source_thumb,
                    autoplay_first, autoplay_seed
             FROM queue_state WHERE singleton = 1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    (
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<String>>(10)?,
                    ),
                    (r.get::<_, Option<i64>>(11)?, r.get::<_, Option<String>>(12)?),
                ))
            },
        );
        let (stored_key, next_id, shuffle, seed, repeat, removed, position, source, autoplay) = match row {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if stored_key != account_key {
            self.purge_private()?;
            return Ok(None);
        }

        let mut history = Vec::new();
        let mut current = None;
        let mut manual = Vec::new();
        let mut context = Vec::new();
        let mut radio = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT entry_id, segment, origin, video_id, title, album_title, album_id, duration_ms
                 FROM queue_entries ORDER BY segment, pos",
            )?;
            let mut artists = self.conn.prepare(
                "SELECT name, browse_id FROM entry_artists WHERE entry_id = ?1 ORDER BY pos",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let id: i64 = r.get(0)?;
                let segment: String = r.get(1)?;
                let origin = origin_from(&r.get::<_, String>(2)?)?;
                let album_title: Option<String> = r.get(5)?;
                let album_id: Option<String> = r.get(6)?;
                let duration_ms: Option<i64> = r.get(7)?;
                let artist_rows = artists.query_map([id], |a| {
                    Ok(ArtistRef {
                        name: a.get(0)?,
                        id: a.get::<_, Option<String>>(1)?.map(BrowseId::new),
                    })
                })?;
                let track = Track {
                    video_id: VideoId::new(r.get::<_, String>(3)?),
                    title: r.get(4)?,
                    artists: artist_rows.collect::<Result<_, _>>()?,
                    album: album_title.map(|title| AlbumRef {
                        title,
                        id: album_id.map(BrowseId::new),
                    }),
                    duration: duration_ms
                        .map(|ms| {
                            u64::try_from(ms).map_err(|_| StoreError::Corrupt("negative duration"))
                        })
                        .transpose()?
                        .map(std::time::Duration::from_millis),
                };
                let entry = QueueEntry {
                    id: QueueEntryId::from_raw(
                        u64::try_from(id).map_err(|_| StoreError::Corrupt("negative id"))?,
                    ),
                    track,
                    origin,
                };
                match segment.as_str() {
                    "history" => history.push(entry),
                    "current" => current = Some((entry, removed != 0)),
                    "manual" => manual.push(entry),
                    "context" => context.push(entry),
                    "radio" => radio.push(entry),
                    _ => return Err(StoreError::Corrupt("unknown segment")),
                }
            }
        }
        let canonical = {
            let mut stmt = self
                .conn
                .prepare("SELECT entry_id FROM queue_canonical ORDER BY pos")?;
            let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            let mut out = Vec::new();
            for id in ids {
                out.push(QueueEntryId::from_raw(
                    u64::try_from(id?).map_err(|_| StoreError::Corrupt("negative id"))?,
                ));
            }
            out
        };
        let snap = QueueSnapshot {
            next_id: u64::try_from(next_id).map_err(|_| StoreError::Corrupt("negative next id"))?,
            history,
            current,
            manual,
            context,
            radio,
            canonical,
            shuffle: shuffle != 0,
            seed: seed as u64,
            repeat: repeat_from(&repeat)?,
            source: source_from(source)?,
            autoplay: match autoplay {
                (Some(first), Some(seed)) => Some(AutoplayStart {
                    first: QueueEntryId::from_raw(
                        u64::try_from(first).map_err(|_| StoreError::Corrupt("negative id"))?,
                    ),
                    seed,
                }),
                _ => None,
            },
        };
        let queue = Queue::restore(snap).map_err(|_| StoreError::Corrupt("queue invariants"))?;
        Ok(Some((queue, u64::try_from(position).unwrap_or(0))))
    }

    /// Cheap periodic checkpoint (about every five seconds, and on pause/exit).
    pub fn checkpoint_position(&self, position_ms: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE queue_state SET position_ms = ?1 WHERE singleton = 1",
            [to_i64(position_ms, "position")?],
        )?;
        Ok(())
    }

    /// Forget all account-private persisted state (logout, identity change, corruption).
    pub fn purge_private(&mut self) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM queue_canonical; DELETE FROM entry_artists;
             DELETE FROM queue_entries; DELETE FROM queue_state;",
        )?;
        tx.commit()?;
        Ok(())
    }
}
