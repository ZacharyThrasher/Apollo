//! Local listening stats: what Apollo itself played, kept on this machine and never sent
//! anywhere, for the Library's Stats tab and Home's "On repeat" and "Forgotten favourites".
//!
//! * [`tally`] decides when a play counts (pure).
//! * [`PlayLog`] (this file) writes counted plays to the app's SQLite file from one worker
//!   thread, in order, and reads the per-track totals back. It is a GPUI global like the page
//!   cache; without one (tests, the headless harness) nothing is recorded.
//! * [`report`] turns totals into what the screens show (pure).
//!
//! Plays belong to the account that was chosen when they counted (the page cache's account) and
//! are kept when it signs out; another account never sees them.

pub mod report;
pub mod tally;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use apollo_domain::model::{AlbumRef, ArtistRef, Track};
use apollo_store::{NewPlay, PlayTotal};
use gpui::{App, Global};

use crate::page_cache::open_store;

/// The play log's own words for one counted play, before it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayRecord {
    pub track: Track,
    pub art: Option<String>,
    pub played_unix: i64,
    pub listened_ms: u64,
}

impl PlayRecord {
    fn into_row(self) -> NewPlay {
        NewPlay {
            played_unix: self.played_unix,
            video_id: self.track.video_id.as_str().to_owned(),
            title: self.track.title,
            artists: serde_json::to_string(&self.track.artists).unwrap_or_else(|_| "[]".into()),
            album_title: self.track.album.as_ref().map(|a| a.title.clone()),
            album_id: self
                .track
                .album
                .as_ref()
                .and_then(|a| a.id.as_ref())
                .map(|id| id.as_str().to_owned()),
            duration_ms: self.track.duration.map(|d| d.as_millis() as u64),
            art: self.art,
            listened_ms: self.listened_ms,
        }
    }
}

/// The track a stored play describes, as far as the log kept it.
pub fn track_of(total: &PlayTotal) -> Track {
    let artists: Vec<ArtistRef> = serde_json::from_str(&total.artists).unwrap_or_default();
    let mut track = Track::new(total.video_id.clone(), total.title.clone());
    track.artists = artists;
    track.album = total.album_title.clone().map(|title| AlbumRef {
        title,
        id: total
            .album_id
            .clone()
            .map(apollo_domain::model::BrowseId::new),
    });
    track.duration = total.duration_ms.map(std::time::Duration::from_millis);
    track
}

enum Job {
    Record {
        key: u64,
        account: String,
        play: Box<NewPlay>,
    },
    Settle {
        key: u64,
        listened_ms: u64,
    },
    /// Everything before this is written; then the sender is told.
    Barrier(Sender<()>),
}

/// Plays kept in memory to settle their listening time after they were written.
const REMEMBERED_ROWS: usize = 4;

struct Inner {
    path: Option<PathBuf>,
    writer: Option<Mutex<Sender<Job>>>,
}

/// See the module docs. Cheap to clone; clones share one writer.
#[derive(Clone)]
pub struct PlayLog {
    inner: Arc<Inner>,
}

impl Global for PlayLog {}

impl PlayLog {
    /// A log kept in the SQLite file at `path`; with `None` nothing is recorded or read.
    pub fn new(path: Option<PathBuf>) -> Self {
        let (writer, rx) = match &path {
            Some(_) => {
                let (tx, rx) = channel::<Job>();
                (Some(Mutex::new(tx)), Some(rx))
            }
            None => (None, None),
        };
        if let (Some(path), Some(rx)) = (path.clone(), rx) {
            let _ = std::thread::Builder::new()
                .name("apollo-play-log".into())
                .spawn(move || write_loop(path, rx));
        }
        Self {
            inner: Arc::new(Inner { path, writer }),
        }
    }

    /// Makes this the app's play log.
    pub fn install(self, cx: &mut App) {
        cx.set_global(self);
    }

    /// The app's play log, when one is installed.
    pub fn try_global(cx: &App) -> Option<PlayLog> {
        cx.try_global::<PlayLog>().cloned()
    }

    fn send(&self, job: Job) {
        if let Some(writer) = &self.inner.writer {
            let _ = writer.lock().unwrap_or_else(|e| e.into_inner()).send(job);
        }
    }

    /// A play counted for `account`. `key` names it for [`settle`](Self::settle).
    pub fn record(&self, account: String, key: u64, play: PlayRecord) {
        self.send(Job::Record {
            key,
            account,
            play: Box::new(play.into_row()),
        });
    }

    /// The play `key` was listened to for `listened_ms` in all.
    pub fn settle(&self, key: u64, listened_ms: u64) {
        self.send(Job::Settle { key, listened_ms });
    }

    /// Waits until everything handed over so far is written (tests).
    pub fn flush(&self) {
        let (done, wait) = channel();
        self.send(Job::Barrier(done));
        let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
    }

    /// Per track, what `account` played at or after `since_unix` (everything for `None`). Blocking:
    /// call off the UI thread. A log that cannot be read says nothing.
    pub fn totals(&self, account: &str, since_unix: Option<i64>) -> Vec<PlayTotal> {
        let Some(store) = self.inner.path.as_deref().and_then(open_store) else {
            return Vec::new();
        };
        match store.play_totals(account, since_unix) {
            Ok(totals) => totals,
            Err(err) => {
                eprintln!("play log read failed: {err}");
                Vec::new()
            }
        }
    }
}

fn write_loop(path: PathBuf, rx: std::sync::mpsc::Receiver<Job>) {
    // The rows written this run, newest last, so a play's listening time can be settled.
    let mut rows: Vec<(u64, i64)> = Vec::new();
    let mut ids: HashMap<u64, i64> = HashMap::new();
    while let Ok(job) = rx.recv() {
        match job {
            Job::Barrier(done) => {
                let _ = done.send(());
            }
            Job::Record { key, account, play } => {
                let Some(store) = open_store(&path) else {
                    continue;
                };
                match store.record_play(&account, &play) {
                    Ok(id) => {
                        rows.push((key, id));
                        ids.insert(key, id);
                        while rows.len() > REMEMBERED_ROWS {
                            let (old, _) = rows.remove(0);
                            ids.remove(&old);
                        }
                    }
                    Err(err) => eprintln!("play log write failed: {err}"),
                }
            }
            Job::Settle { key, listened_ms } => {
                let (Some(id), Some(store)) = (ids.get(&key).copied(), open_store(&path)) else {
                    continue;
                };
                if let Err(err) = store.set_listened(id, listened_ms) {
                    eprintln!("play log write failed: {err}");
                }
            }
        }
    }
}
