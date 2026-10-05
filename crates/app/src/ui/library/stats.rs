//! The Library's Stats tab: what Apollo itself played, from the local play log (nothing is asked
//! of the service). A row of three cards (listening time, tracks played, top artist), then the
//! Top tracks, Top artists and Top albums of the chosen period, ten rows each. The period is
//! chosen in the popover at the tab row's right end (the sort control's twin).
//!
//! The lists are one keyboard selection across all three, numbered in the order they are
//! drawn; clicking a track plays the Top tracks list from it, an artist or an album opens its
//! page (when the log kept its id).

use std::sync::Arc;

use super::*;
use crate::stats::report::{artist_line, listening_time, StatsReport};

/// Width of the plays count at a row's right end ("128 plays").
const PLAYS_WIDTH: f32 = 72.0;
/// Placeholder rows while the first answer is on its way (it is a local read: a blink).
const STATS_SKELETON_ROWS: usize = 6;

const HEADINGS: [&str; 3] = ["Top tracks", "Top artists", "Top albums"];

/// Which list a selected row is in, and its place there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatRow {
    Track(usize),
    Artist(usize),
    Album(usize),
}

/// The row with selection index `ix` when the three lists are `lens` long.
pub(super) fn stat_row(lens: [usize; 3], ix: usize) -> Option<StatRow> {
    let [tracks, artists, albums] = lens;
    if ix < tracks {
        Some(StatRow::Track(ix))
    } else if ix < tracks + artists {
        Some(StatRow::Artist(ix - tracks))
    } else if ix < tracks + artists + albums {
        Some(StatRow::Album(ix - tracks - artists))
    } else {
        None
    }
}

/// The child of the scrolling column that holds the row with selection index `ix`: child 0 is
/// the cards, then each list that has rows is its heading and its rows.
pub(super) fn stats_child(lens: [usize; 3], ix: usize) -> usize {
    let mut child = 1;
    let mut first = 0;
    for len in lens {
        if len == 0 {
            continue;
        }
        if ix < first + len {
            return child + 1 + (ix - first);
        }
        child += 1 + len;
        first += len;
    }
    child.saturating_sub(1)
}

fn plays_text(plays: u64) -> String {
    if plays == 1 {
        "1 play".into()
    } else {
        format!("{plays} plays")
    }
}

impl LibraryView {
    pub(super) fn render_stats_tab(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let report: Option<Arc<StatsReport>> = match self.model.read(cx).stats() {
            Load::Ready(report) => Some(report.clone()),
            Load::Idle | Load::Loading | Load::Failed(_) => None,
        };
        let Some(report) = report else {
            return Self::skeleton_rows(
                STATS_SKELETON_ROWS,
                TRACK_ROW,
                TRACK_ART,
                !cx.reduce_motion(),
            );
        };
        if report.is_empty() {
            return div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(layout::MIN_HIT_TARGET))
                .child(
                    style::text_style(div(), typography::BODY, cx)
                        .debug_selector(|| "library-stats-empty".into())
                        .text_color(color::text_secondary())
                        .child("Play some music and your stats show up here"),
                )
                .into_any_element();
        }

        let animations = !cx.reduce_motion();
        let list_focused = keyboard_focused(&self.focus.list, window);
        self.hover.prune();
        let mut moving = false;
        let playing = self.player.read(cx).current_video();

        let top_artist = report
            .top_artist()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "\u{2014}".into());
        let mut children: Vec<AnyElement> = vec![div()
            .id("stats-cards")
            .flex()
            .flex_row()
            .flex_none()
            .gap(px(spacing::S3))
            .child(self.stat_card(
                "stats-card-time",
                "Listening time",
                listening_time(report.listened_ms),
                cx,
            ))
            .child(self.stat_card(
                "stats-card-plays",
                "Tracks played",
                report.plays.to_string(),
                cx,
            ))
            .child(self.stat_card("stats-card-artist", "Top artist", top_artist, cx))
            .into_any_element()];

        let lens = [report.tracks.len(), report.artists.len(), report.albums.len()];
        let mut ix = 0;
        for (list, len) in lens.into_iter().enumerate() {
            if len == 0 {
                continue;
            }
            children.push(
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id(("stats-heading", list))
                    .flex_none()
                    .px(px(spacing::S2))
                    .mt(px(spacing::S4))
                    .mb(px(spacing::S1))
                    .text_color(color::text_secondary())
                    .child(HEADINGS[list])
                    .into_any_element(),
            );
            for _ in 0..len {
                let frame = RowFrame {
                    ix,
                    real: ix,
                    level: self.hover.level(ix, &mut moving),
                    selected: list_focused && ix == self.selected,
                    picked: false,
                    accent,
                    animations,
                };
                let row = match stat_row(lens, ix) {
                    Some(StatRow::Track(i)) => {
                        self.stat_track_row(frame, &report, i, playing.as_deref(), cx)
                    }
                    Some(StatRow::Artist(i)) => self.stat_artist_row(frame, &report, i, cx),
                    Some(StatRow::Album(i)) => self.stat_album_row(frame, &report, i, cx),
                    None => div().into_any_element(),
                };
                children.push(row);
                ix += 1;
            }
        }
        if moving {
            window.request_animation_frame();
        }
        self.rendered = 0..ix;

        let list = div()
            .id("library-stats")
            .key_context(LIST_CONTEXT)
            .track_focus(&self.focus.list)
            .role(gpui::Role::List)
            .aria_label("Stats")
            .size_full()
            .min_h_0();
        Self::list_actions(list, cx)
            .child(
                div()
                    .id("stats-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.stats_scroll)
                    .pb(px(spacing::S6))
                    .children(children),
            )
            .into_any_element()
    }

    /// One of the three cards: a small label over a big value, on bg.raised.
    fn stat_card(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(spacing::S1))
            .p(px(spacing::S4))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_raised())
            .child(
                style::text_style(div(), typography::CAPTION, cx)
                    .text_color(color::text_secondary())
                    .child(label),
            )
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .truncate()
                    .text_color(color::text_primary())
                    .child(SharedString::from(value)),
            )
            .into_any_element()
    }

    /// The rank at a row's left, in text.secondary.
    fn stat_rank(rank: usize, cx: &App) -> AnyElement {
        div()
            .w(px(INDEX_WIDTH))
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .child(
                style::tabular(style::text_style(div(), typography::BODY, cx))
                    .text_color(color::text_secondary())
                    .child(format!("{rank}")),
            )
            .into_any_element()
    }

    /// The plays count at a row's right end, like a track's duration.
    fn stat_plays(ix: usize, plays: u64, cx: &App) -> AnyElement {
        div()
            .w(px(PLAYS_WIDTH))
            .flex_none()
            .flex()
            .justify_end()
            .child(Self::line(
                ("stat-plays", ix),
                plays_text(plays).into(),
                typography::CAPTION,
                color::text_tertiary(),
                true,
                cx,
            ))
            .into_any_element()
    }

    fn stat_track_row(
        &mut self,
        frame: RowFrame,
        report: &StatsReport,
        i: usize,
        playing_video: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(top) = report.tracks.get(i) else {
            return div().into_any_element();
        };
        let ix = frame.ix;
        let playing = playing_video == Some(top.track.video_id.as_str());
        let accent = frame.accent;
        let art = self.art_tile(
            top.art.as_deref(),
            ArtSize::List,
            TRACK_ART,
            Icon::MusicNote,
            color::text_tertiary(),
            cx,
        );
        let artists: SharedString = artist_line(&top.track.artists).into();
        let subject = Subject::track(top.track.clone(), top.art.clone());
        self.row_shell_with(
            &frame,
            TRACK_ROW,
            playing,
            Some(top.track.clone()),
            Some(subject),
            move |this, window, cx| this.activate_stat(ix, window, cx),
            cx,
        )
        .aria_label(top.track.title.clone())
        .when(playing, |d| {
            d.child(playing_row::marker(color::accent(accent)))
        })
        .child(Self::stat_rank(i + 1, cx))
        .child(art)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(Self::line(
                    ("stat-title", ix),
                    top.track.title.clone().into(),
                    playing_row::title_style(typography::LIST_TITLE, playing),
                    color::text_primary(),
                    false,
                    cx,
                ))
                .child(Self::line(
                    ("stat-subtitle", ix),
                    artists,
                    typography::BODY,
                    color::text_secondary(),
                    false,
                    cx,
                )),
        )
        .child(Self::stat_plays(ix, top.plays, cx))
        .into_any_element()
    }

    fn stat_artist_row(
        &mut self,
        frame: RowFrame,
        report: &StatsReport,
        i: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(top) = report.artists.get(i) else {
            return div().into_any_element();
        };
        let ix = frame.ix;
        // The artist's cover is that of their most played track, in the artist's own shape.
        let art = self.art_tile_shaped(
            top.art.as_deref(),
            ArtSize::List,
            TRACK_ART,
            Icon::MusicNote,
            color::text_tertiary(),
            true,
            cx,
        );
        self.row_shell_with(
            &frame,
            TRACK_ROW,
            false,
            None,
            None,
            move |this, window, cx| this.activate_stat(ix, window, cx),
            cx,
        )
        .aria_label(top.name.clone())
        .child(Self::stat_rank(i + 1, cx))
        .child(art)
        .child(
            div().flex_1().min_w_0().child(Self::line(
                ("stat-title", ix),
                top.name.clone().into(),
                typography::LIST_TITLE,
                color::text_primary(),
                false,
                cx,
            )),
        )
        .child(Self::stat_plays(ix, top.plays, cx))
        .into_any_element()
    }

    fn stat_album_row(
        &mut self,
        frame: RowFrame,
        report: &StatsReport,
        i: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(top) = report.albums.get(i) else {
            return div().into_any_element();
        };
        let ix = frame.ix;
        let art = self.art_tile(
            top.art.as_deref(),
            ArtSize::List,
            TRACK_ART,
            Icon::MusicNote,
            color::text_tertiary(),
            cx,
        );
        // An album the log has no id for has no page to open and no menu.
        let menu = Self::album_collection(top).map(Subject::Collection);
        self.row_shell_with(
            &frame,
            TRACK_ROW,
            false,
            None,
            menu,
            move |this, window, cx| this.activate_stat(ix, window, cx),
            cx,
        )
        .aria_label(top.title.clone())
        .child(Self::stat_rank(i + 1, cx))
        .child(art)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(Self::line(
                    ("stat-title", ix),
                    top.title.clone().into(),
                    typography::LIST_TITLE,
                    color::text_primary(),
                    false,
                    cx,
                ))
                .children(top.artist.clone().map(|artist| {
                    Self::line(
                        ("stat-subtitle", ix),
                        artist.into(),
                        typography::BODY,
                        color::text_secondary(),
                        false,
                        cx,
                    )
                })),
        )
        .child(Self::stat_plays(ix, top.plays, cx))
        .into_any_element()
    }

    /// The album as a collection to open, when the log kept its id.
    fn album_collection(album: &crate::stats::report::TopAlbum) -> Option<Collection> {
        Some(Collection {
            kind: CollectionKind::Album,
            id: album.id.clone()?,
            title: album.title.clone(),
            subtitle: album.artist.clone(),
            thumbnail: album.art.clone(),
        })
    }

    /// Enter or a click on a stats row: a track plays the Top tracks list from it, an artist or
    /// an album opens its page.
    pub(super) fn activate_stat(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let report = match self.model.read(cx).stats() {
            Load::Ready(report) => report.clone(),
            _ => return,
        };
        let lens = [report.tracks.len(), report.artists.len(), report.albums.len()];
        match stat_row(lens, ix) {
            Some(StatRow::Track(i)) => {
                let tracks: Vec<Track> = report.tracks.iter().map(|t| t.track.clone()).collect();
                self.player.update(cx, |p, cx| {
                    for top in &report.tracks {
                        p.hint_art(top.track.video_id.as_str(), top.art.as_deref());
                    }
                    p.play_tracks(tracks, Some(i), Some(false), cx)
                });
            }
            Some(StatRow::Artist(i)) => {
                if let Some(top) = report.artists.get(i) {
                    if let Some(id) = top.id.clone() {
                        self.open_artist(
                            ArtistTarget {
                                id,
                                name: top.name.clone(),
                                thumbnail: top.art.clone(),
                            },
                            window,
                            cx,
                        );
                    }
                }
            }
            Some(StatRow::Album(i)) => {
                if let Some(collection) = report.albums.get(i).and_then(Self::album_collection) {
                    self.open(collection, window, cx);
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_index_runs_through_the_three_lists() {
        let lens = [3, 2, 4];
        assert_eq!(stat_row(lens, 0), Some(StatRow::Track(0)));
        assert_eq!(stat_row(lens, 2), Some(StatRow::Track(2)));
        assert_eq!(stat_row(lens, 3), Some(StatRow::Artist(0)));
        assert_eq!(stat_row(lens, 5), Some(StatRow::Album(0)));
        assert_eq!(stat_row(lens, 8), Some(StatRow::Album(3)));
        assert_eq!(stat_row(lens, 9), None);
    }

    #[test]
    fn the_scroll_child_skips_the_cards_and_the_headings() {
        let lens = [3, 2, 4];
        // 0 cards, 1 heading, 2-4 tracks, 5 heading, 6-7 artists, 8 heading, 9-12 albums.
        assert_eq!(stats_child(lens, 0), 2);
        assert_eq!(stats_child(lens, 3), 6);
        assert_eq!(stats_child(lens, 5), 9);
        // A list with no rows has no heading.
        assert_eq!(stats_child([3, 0, 4], 3), 6);
    }

    #[test]
    fn plays_read_plainly() {
        assert_eq!(plays_text(1), "1 play");
        assert_eq!(plays_text(12), "12 plays");
    }
}
