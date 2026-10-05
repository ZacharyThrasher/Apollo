//! Dig from here: one seed song, keep what fits. D on a song asks for its radio and Skims it
//! right away (K keeps into the Crate, Enter plays, Esc goes back to what was playing).
//!
//! * **One radio fetch per dig**, in the foreground lane: while YouTube's cooldown runs
//!   ([`Throttle`]) nothing is sent and the pill says so. Nothing here fires on hover, at launch
//!   or by itself; the only other traffic is Skim's own one-ahead prefetch.
//! * **Songs you already have are left out** (Settings > Playlists > Leave out songs I already
//!   have): the Crate, the target playlist and the library's Songs as far as the page cache holds
//!   them, the songs the radio itself marks liked, and the local play log. Nothing is asked of
//!   YouTube for that; a list that was never opened here simply is not known. Blocked artists'
//!   songs are always left out, as in any radio. At most [`CAP`] songs are skimmed.
//! * A dig is a [`Skim`](super::skim): same window, same keys, same "before" for Esc, and the
//!   player bar's pill reads "Digging · <seed title>".

use std::collections::HashSet;

use apollo_domain::blocklist::{retain_allowed, Blocklist};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_services_common::{Lane, Throttle};
use gpui::Context;

use super::PlayerModel;
use crate::library::source::TrackRow;
use crate::page_cache::{browse_id, keys, PageCache};
use crate::queue_source;
use crate::stats::PlayLog;

/// Most songs one dig skims.
pub const CAP: usize = 50;
/// What the pill says when the radio held nothing that was not already had.
pub const NOTHING_NEW: &str = "Nothing new in this radio";
/// ... while YouTube's cooldown runs (or answered "too many requests").
pub const BUSY: &str = "YouTube is busy, try again shortly";
/// ... when the radio could not be fetched for another reason.
pub const FAILED: &str = "Couldn't dig from this song";

/// The seed's title is cut to this many characters in the pill, which sits beside the cover in
/// the player bar and must not crowd the title out.
const PILL_TITLE_MAX: usize = 18;

/// The pill of a dig: "Digging · <seed title>", the title cut when long.
pub fn pill_text(seed_title: &str) -> String {
    let title = seed_title.trim();
    let mut cut: String = title.chars().take(PILL_TITLE_MAX).collect();
    if title.chars().count() > PILL_TITLE_MAX {
        cut.pop();
        cut.push('\u{2026}');
    }
    format!("Digging \u{b7} {cut}")
}

/// A radio's rows as a dig skims them: blocked artists, the seed itself and repeats out, then (when
/// `leave_out`) every song in `have` or marked liked, and no more than [`CAP`].
pub fn pick(
    rows: Vec<TrackRow>,
    seed: &str,
    have: &HashSet<String>,
    blocked: &Blocklist,
    leave_out: bool,
) -> Vec<TrackRow> {
    let rows = retain_allowed(rows, blocked, |r| &r.track);
    let mut seen: HashSet<String> = HashSet::new();
    rows.into_iter()
        .filter(|r| {
            let id = r.track.video_id.as_str();
            id != seed
                && seen.insert(id.to_owned())
                && !(leave_out && (have.contains(id) || r.liked == Some(true)))
        })
        .take(CAP)
        .collect()
}

/// What the library already holds that this PC knows without asking YouTube. Reads the page cache
/// and the play log, so it runs off the UI thread.
fn known_songs(
    cache: Option<PageCache>,
    target: Option<String>,
    log: Option<PlayLog>,
) -> HashSet<String> {
    let mut have = HashSet::new();
    let account = cache.as_ref().and_then(|c| c.account());
    if let Some(cache) = &cache {
        let mut take = |rows: Option<Vec<TrackRow>>| {
            for row in rows.unwrap_or_default() {
                have.insert(row.track.video_id.as_str().to_owned());
            }
        };
        take(cache.get::<Vec<TrackRow>>(keys::SONGS).map(|c| c.value));
        if let Some(id) = target {
            let key = keys::collection(&BrowseId::new(browse_id(&id)));
            take(cache.get::<Vec<TrackRow>>(&key).map(|c| c.value));
        }
    }
    if let (Some(log), Some(account)) = (log, account) {
        for play in log.totals(&account, None) {
            have.insert(play.video_id);
        }
    }
    have
}

impl PlayerModel {
    /// Whether D (or the menu's item) can start a dig now: Skim is on, an engine is there, and
    /// the last dig's radio is not still coming.
    pub fn dig_ready(&self) -> bool {
        self.skim_enabled && self.driver.is_some() && !self.dig_inflight
    }

    /// Dig from `seed`: fetch its radio and Skim it. Does nothing unless [`Self::dig_ready`]; says
    /// "YouTube is busy" instead of asking while the cooldown runs.
    pub fn dig_from(&mut self, seed: Track, thumbnail: Option<String>, cx: &mut Context<Self>) {
        if !self.dig_ready() {
            return;
        }
        if Throttle::global().blocked(Lane::Foreground).is_some() {
            crate::confirm::said(BUSY, cx);
            return;
        }
        let Some(source) = self.library.clone() else {
            return;
        };
        self.dig_inflight = true;
        self.hint_art(seed.video_id.as_str(), thumbnail.as_deref());
        let leave_out = crate::dig::leave_out(cx);
        let blocked = crate::blocked_artists::snapshot(cx);
        let cache = PageCache::try_global(cx);
        let log = PlayLog::try_global(cx);
        let target = crate::target_playlist::current_id(cx);
        let video = seed.video_id.as_str().to_owned();
        cx.spawn(async move |this, cx| {
            let (found, have) = cx
                .background_executor()
                .spawn(async move {
                    let found = source.radio(&video);
                    let have = if leave_out && found.is_ok() {
                        known_songs(cache, target, log)
                    } else {
                        HashSet::new()
                    };
                    (found, have)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.dig_arrived(seed, found, have, leave_out, blocked, cx)
            });
        })
        .detach();
    }

    fn dig_arrived(
        &mut self,
        seed: Track,
        found: Result<Vec<TrackRow>, LoadFailure>,
        mut have: HashSet<String>,
        leave_out: bool,
        blocked: Blocklist,
        cx: &mut Context<Self>,
    ) {
        self.dig_inflight = false;
        let rows = match found {
            Ok(rows) if !rows.is_empty() => rows,
            Err(LoadFailure::Throttled) => return crate::confirm::said(BUSY, cx),
            _ => return crate::confirm::said(FAILED, cx),
        };
        if leave_out {
            // What was set aside while the radio was coming counts too.
            if let Some(crate_) = crate::crate_list::handle(cx) {
                have.extend(
                    crate_
                        .read(cx)
                        .items()
                        .iter()
                        .map(|r| r.track.video_id.as_str().to_owned()),
                );
            }
        }
        let rows = pick(rows, seed.video_id.as_str(), &have, &blocked, leave_out);
        if rows.is_empty() {
            return crate::confirm::said(NOTHING_NEW, cx);
        }
        for row in &rows {
            self.hint_art(row.track.video_id.as_str(), row.thumbnail.as_deref());
        }
        let title = seed.title.clone();
        let tracks: Vec<Track> = rows.into_iter().map(|r| r.track).collect();
        self.begin_skim(
            tracks,
            0,
            Some(queue_source::radio(title.clone())),
            Some(title.into()),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, liked: Option<bool>) -> TrackRow {
        TrackRow {
            track: Track::new(id, id),
            occurrence: None,
            thumbnail: None,
            explicit: false,
            liked,
        }
    }

    fn ids(rows: &[TrackRow]) -> Vec<&str> {
        rows.iter().map(|r| r.track.video_id.as_str()).collect()
    }

    fn have(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn songs_you_have_the_seed_and_repeats_are_left_out() {
        let rows = vec![
            row("seed", None),
            row("a", None),
            row("b", Some(true)),
            row("a", None),
            row("c", Some(false)),
            row("d", None),
        ];
        let picked = pick(rows, "seed", &have(&["d"]), &Blocklist::default(), true);
        assert_eq!(ids(&picked), ["a", "c"]);
    }

    #[test]
    fn with_the_switch_off_only_the_seed_repeats_and_blocked_artists_go() {
        let rows = vec![row("seed", None), row("a", None), row("b", Some(true)), row("a", None)];
        let picked = pick(rows, "seed", &have(&["a"]), &Blocklist::default(), false);
        assert_eq!(ids(&picked), ["a", "b"]);
    }

    #[test]
    fn a_dig_skims_at_most_fifty() {
        let rows: Vec<TrackRow> = (0..80).map(|n| row(&format!("t{n}"), None)).collect();
        let picked = pick(rows, "seed", &HashSet::new(), &Blocklist::default(), true);
        assert_eq!(picked.len(), CAP);
        assert_eq!(picked[0].track.video_id.as_str(), "t0", "in the radio's order");
    }

    #[test]
    fn everything_known_leaves_nothing() {
        let rows = vec![row("a", None), row("b", Some(true))];
        assert!(pick(rows, "seed", &have(&["a"]), &Blocklist::default(), true).is_empty());
    }

    #[test]
    fn the_pill_names_the_seed_and_cuts_a_long_title() {
        assert_eq!(pill_text("Midnight Drive"), "Digging \u{b7} Midnight Drive");
        let long = pill_text(&"x".repeat(60));
        assert_eq!(long.chars().count(), "Digging \u{b7} ".chars().count() + PILL_TITLE_MAX);
        assert!(long.ends_with('\u{2026}'));
    }
}
