//! "Recently played": the songs heard in the last week, newest first, for the Crate page's
//! collapsible section and the dock's Recent tab, so a song loved an hour ago and forgotten can
//! still be set aside.
//!
//! It reads only the local play log (`stats::PlayLog`): no YouTube calls. What the log holds is
//! what counted as a play (see `stats::tally`), so a song left in a few seconds is not here and
//! skimmed songs never are. Consecutive plays of one song show once, at the newest, and the
//! artists the owner blocked stay out as they do on Home's personal shelves.
//!
//! * [`build`] and [`ago_text`] are pure.
//! * [`RecentModel`] reads the log off the UI thread for the account that is chosen, and again
//!   whenever the log has been written to since (a play counted), but only when a view asks
//!   ([`RecentModel::ensure_fresh`], called while one is on screen), so a closed tray costs nothing.
//!   It also remembers whether the Crate page's section is open.

use std::sync::Arc;

use apollo_domain::blocklist::Blocklist;
use apollo_domain::model::Track;
use apollo_store::RecentPlay;
use gpui::{App, Context, Entity, Global};

use crate::page_cache::PageCache;
use crate::settings::SettingsBackend;
use crate::stats::{track_of, PlayLog};

/// How far back the list goes.
pub const RECENT_DAYS: i64 = 7;
/// The most rows it shows.
pub const MAX_ROWS: usize = 200;
/// Plays read from the log before repeats are collapsed; a week of listening is far below it.
const RAW_LIMIT: usize = 2_000;
/// Where the section's open state is kept (the window settings store).
const KEY_OPEN: &str = "crate_recent_open";

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentRow {
    pub track: Track,
    /// The cover as the log kept it; handed to the Crate when the song is set aside.
    pub art: Option<String>,
    /// Unix seconds when the play counted.
    pub played_unix: i64,
}

/// The rows for plays given newest first: consecutive plays of the same song collapse into the
/// newest, blocked artists' songs are left out, and no more than [`MAX_ROWS`] stay.
pub fn build(plays: Vec<RecentPlay>, blocked: &Blocklist) -> Vec<RecentRow> {
    let mut rows: Vec<RecentRow> = Vec::new();
    let mut last: Option<String> = None;
    for play in plays {
        // Repeats are judged on the log itself: a blocked song between two plays of another
        // still separates them.
        if last.as_deref() == Some(play.video_id.as_str()) {
            continue;
        }
        last = Some(play.video_id.clone());
        let total = apollo_store::PlayTotal {
            video_id: play.video_id,
            title: play.title,
            artists: play.artists,
            album_title: play.album_title,
            album_id: play.album_id,
            duration_ms: play.duration_ms,
            art: play.art.clone(),
            plays: 1,
            listened_ms: 0,
            last_played: play.played_unix,
        };
        let track = track_of(&total);
        if blocked.blocks(&track) {
            continue;
        }
        rows.push(RecentRow {
            track,
            art: play.art,
            played_unix: play.played_unix,
        });
        if rows.len() == MAX_ROWS {
            break;
        }
    }
    rows
}

/// How long ago, as a row shows it: "now", "5m ago", "3h ago", "2d ago".
pub fn ago_text(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        0..=59 => "now".into(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// The recent plays as the views see them.
pub struct RecentModel {
    rows: Vec<RecentRow>,
    /// The account `rows` were read for.
    loaded_for: Option<String>,
    /// What the log's write count was when the last read began.
    seen: Option<u64>,
    /// Newest read wanted; an older one that finishes late is dropped.
    rev: u64,
    open: bool,
    store: Option<Arc<dyn SettingsBackend>>,
}

impl RecentModel {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            loaded_for: None,
            seen: None,
            rev: 0,
            open: false,
            store: None,
        }
    }

    pub fn rows(&self) -> &[RecentRow] {
        &self.rows
    }

    /// Whether the Crate page's section is expanded (it starts collapsed).
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        if let Some(store) = self.store.clone() {
            cx.background_executor()
                .spawn(async move { store.set(KEY_OPEN, if open { "on" } else { "off" }) })
                .detach();
        }
        cx.notify();
    }

    /// Where the section's open state is kept, and the saved state applied.
    pub fn install(&mut self, store: Option<Arc<dyn SettingsBackend>>, cx: &mut Context<Self>) {
        self.store = store.clone();
        let Some(store) = store else { return };
        let before = self.open;
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { store.get(KEY_OPEN) })
                .await;
            let _ = this.update(cx, |m, cx| {
                // A choice made while this was loading wins over the saved one.
                if saved.as_deref() == Some("on") && m.open == before && !m.open {
                    m.open = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads the log again if it was written to since the last read, or another account is
    /// chosen. Cheap when nothing changed, so a view that is on screen calls it on every render.
    /// It never notifies: the view that asked is drawing already, and the finished read notifies.
    pub fn ensure_fresh(&mut self, cx: &mut Context<Self>) {
        let account = PageCache::try_global(cx).and_then(|c| c.account());
        let log = PlayLog::try_global(cx);
        let (Some(account), Some(log)) = (account, log) else {
            // Nobody is chosen (or nothing is recorded): there is nothing to list.
            self.rows.clear();
            self.loaded_for = None;
            self.seen = None;
            return;
        };
        let written = log.written();
        if self.loaded_for.as_deref() == Some(account.as_str()) && self.seen == Some(written) {
            return;
        }
        if self.loaded_for.as_deref() != Some(account.as_str()) {
            // Another account's songs must not show while its own are read.
            self.rows.clear();
        }
        self.read(account, log, written, cx);
    }

    fn read(&mut self, account: String, log: PlayLog, written: u64, cx: &mut Context<Self>) {
        self.rev += 1;
        let rev = self.rev;
        self.seen = Some(written);
        self.loaded_for = Some(account.clone());
        let blocked = crate::blocked_artists::snapshot(cx);
        cx.spawn(async move |this, cx| {
            let asked = account.clone();
            let rows = cx
                .background_executor()
                .spawn(async move {
                    let now = apollo_services_common::unix_now() as i64;
                    let since = now - RECENT_DAYS * 24 * 60 * 60;
                    build(log.recent(&asked, since, RAW_LIMIT), &blocked)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let current = PageCache::try_global(cx).and_then(|c| c.account());
                if m.rev != rev || current.as_deref() != Some(account.as_str()) {
                    return;
                }
                if m.rows != rows {
                    m.rows = rows;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Default for RecentModel {
    fn default() -> Self {
        Self::new()
    }
}

/// The shell's list, reachable from the views that draw it.
#[derive(Clone)]
pub struct RecentHandle(pub Entity<RecentModel>);

impl Global for RecentHandle {}

/// The shell's list, where there is a shell.
pub fn handle(cx: &App) -> Option<Entity<RecentModel>> {
    cx.try_global::<RecentHandle>().map(|h| h.0.clone())
}

#[cfg(test)]
mod tests {
    use apollo_domain::blocklist::BlockedArtist;
    use apollo_domain::model::{ArtistRef, BrowseId};

    use super::*;

    fn play(video: &str, at: i64, artist: Option<(&str, &str)>) -> RecentPlay {
        let artists: Vec<ArtistRef> = artist
            .map(|(name, id)| {
                vec![ArtistRef {
                    name: name.into(),
                    id: Some(BrowseId::new(id)),
                }]
            })
            .unwrap_or_default();
        RecentPlay {
            video_id: video.into(),
            title: format!("Title {video}"),
            artists: serde_json::to_string(&artists).unwrap(),
            album_title: None,
            album_id: None,
            duration_ms: Some(180_000),
            art: Some(format!("https://example.com/{video}.jpg")),
            played_unix: at,
        }
    }

    fn ids(rows: &[RecentRow]) -> Vec<&str> {
        rows.iter().map(|r| r.track.video_id.as_str()).collect()
    }

    #[test]
    fn rows_keep_the_log_order_newest_first() {
        let plays = vec![play("c", 300, None), play("b", 200, None), play("a", 100, None)];
        let rows = build(plays, &Blocklist::default());
        assert_eq!(ids(&rows), ["c", "b", "a"]);
        assert_eq!(rows[0].played_unix, 300);
        assert_eq!(rows[0].art.as_deref(), Some("https://example.com/c.jpg"));
    }

    #[test]
    fn consecutive_repeats_show_once_at_the_newest_play() {
        let plays = vec![
            play("a", 500, None),
            play("a", 400, None),
            play("b", 300, None),
            play("a", 200, None),
            play("a", 100, None),
        ];
        let rows = build(plays, &Blocklist::default());
        assert_eq!(ids(&rows), ["a", "b", "a"], "a heard again after b is a new row");
        assert_eq!((rows[0].played_unix, rows[2].played_unix), (500, 200));
    }

    #[test]
    fn a_blocked_song_still_separates_two_plays_of_another() {
        let ban = Blocklist::new(vec![BlockedArtist {
            id: Some("UCban".into()),
            name: "Banned".into(),
        }]);
        let plays = vec![
            play("a", 300, None),
            play("x", 200, Some(("Banned", "UCban"))),
            play("a", 100, None),
        ];
        assert_eq!(ids(&build(plays, &ban)), ["a", "a"]);
    }

    #[test]
    fn blocked_artists_stay_out() {
        let ban = Blocklist::new(vec![BlockedArtist {
            id: Some("UCban".into()),
            name: "Banned".into(),
        }]);
        let plays = vec![
            play("x", 300, Some(("Banned", "UCban"))),
            play("y", 200, Some(("Fine", "UCfine"))),
        ];
        assert_eq!(ids(&build(plays, &ban)), ["y"]);
    }

    #[test]
    fn no_more_than_two_hundred_rows() {
        let plays: Vec<RecentPlay> = (0..450)
            .map(|i| play(&format!("v{i}"), 10_000 - i, None))
            .collect();
        let rows = build(plays, &Blocklist::default());
        assert_eq!(rows.len(), MAX_ROWS);
        assert_eq!(rows[0].track.video_id.as_str(), "v0");
        assert_eq!(rows[MAX_ROWS - 1].track.video_id.as_str(), "v199");
    }

    #[test]
    fn the_details_come_back_as_a_track() {
        let rows = build(vec![play("a", 100, Some(("Amy", "UCamy")))], &Blocklist::default());
        let track = &rows[0].track;
        assert_eq!(track.title, "Title a");
        assert_eq!(track.artists[0].name, "Amy");
        assert_eq!(track.duration.map(|d| d.as_secs()), Some(180));
    }

    #[test]
    fn how_long_ago_reads_in_the_biggest_whole_unit() {
        let now = 1_000_000;
        assert_eq!(ago_text(now, now), "now");
        assert_eq!(ago_text(now, now - 59), "now");
        assert_eq!(ago_text(now, now - 60), "1m ago");
        assert_eq!(ago_text(now, now - 59 * 60), "59m ago");
        assert_eq!(ago_text(now, now - 3_600), "1h ago");
        assert_eq!(ago_text(now, now - 23 * 3_600 - 59), "23h ago");
        assert_eq!(ago_text(now, now - 86_400), "1d ago");
        assert_eq!(ago_text(now, now - 6 * 86_400), "6d ago");
        assert_eq!(ago_text(now, now + 30), "now", "a clock that stepped back is not negative");
    }
}
