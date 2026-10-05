//! "More like this": the last rows of a playlist page you own. A title row (the shelf title with a
//! Refresh button), then ten songs from the playlist's radio. They look like the playlist's own
//! rows, but the title and artist are text.secondary until hovered, and a + takes the place of
//! the More button: it adds the song to the playlist, and the row moves up into the list.
//!
//! A child of the library view: the rows are extra rows of the page's virtualized track list
//! (after the footer row), so they scroll with it. What they show comes from
//! [`crate::more_like::MoreLikeModel`].

use super::*;
use crate::more_like::{MoreLikeModel, Phase};
use crate::queue_source;
use crate::ui::row_actions::row_action;

/// Skeleton rows under the title while the first set is on its way.
const SKELETON: usize = 3;

impl LibraryView {
    /// Settings > Playlists > Show More like this.
    pub fn set_more_like_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.more.update(cx, |m, cx| m.set_enabled(enabled, cx));
    }

    /// What the section shows (tests).
    pub fn more_like(&self) -> &Entity<MoreLikeModel> {
        &self.more
    }

    /// How many rows the section adds after the page's footer row: none unless the page is a
    /// playlist of your own, every row is in view (no filter) and there is something to show.
    pub(super) fn rail_rows(&self, cx: &App) -> usize {
        if !self.filter_text(cx).is_empty() {
            return 0;
        }
        let editable = self.model.read(cx).detail().is_some_and(|d| d.can_edit());
        if !editable {
            return 0;
        }
        let more = self.more.read(cx);
        match more.phase() {
            Phase::Hidden => 0,
            Phase::Loading => 1 + SKELETON,
            Phase::Ready => 1 + more.songs().len(),
            Phase::Failed(_) => 2,
        }
    }

    /// Which row of the section list row `ix` is (0 is the title row), if it is one.
    pub(super) fn rail_slot(&self, ix: usize, cx: &App) -> Option<usize> {
        let loaded = self.model.read(cx).detail()?.listing.items().len();
        // The footer row sits at `loaded`; the section starts after it.
        let slot = ix.checked_sub(loaded + 1)?;
        (slot < self.rail_rows(cx)).then_some(slot)
    }

    pub(super) fn rail_row(
        &mut self,
        slot: usize,
        frame: RowFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let phase = self.more.read(cx).phase();
        if slot == 0 {
            return self.rail_title(phase, frame.accent, cx);
        }
        match phase {
            Phase::Ready => self.suggestion_row(slot - 1, frame, window, cx),
            Phase::Loading => {
                Self::skeleton_rows(1, TRACK_ROW, TRACK_ART, !cx.reduce_motion())
            }
            _ => div()
                .flex()
                .items_center()
                .h(px(TRACK_ROW))
                .px(px(spacing::S2))
                .child(Self::line(
                    ("more-like-failed", frame.ix),
                    "Couldn't load suggestions".into(),
                    typography::BODY,
                    color::text_secondary(),
                    false,
                    cx,
                ))
                .into_any_element(),
        }
    }

    /// The shelf title, and Refresh (Retry after a failure) at the right.
    fn rail_title(&mut self, phase: Phase, accent: Accent, cx: &mut Context<Self>) -> AnyElement {
        let refreshing = self.more.read(cx).is_refreshing();
        let more = self.more.clone();
        let button = match phase {
            Phase::Ready | Phase::Failed(_) => {
                let label = if matches!(phase, Phase::Failed(_)) {
                    "Retry"
                } else {
                    "Refresh"
                };
                Some(
                    TextButton::new("more-like-refresh", &self.focus.more_refresh, label, accent)
                        .busy(refreshing)
                        .on_click(move |_, cx| {
                            more.update(cx, |m, cx| match phase {
                                Phase::Failed(_) => m.retry(cx),
                                _ => m.refresh(cx),
                            })
                        }),
                )
            }
            _ => None,
        };
        div()
            .id("more-like-title")
            .debug_selector(|| "more-like-title".into())
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(TRACK_ROW))
            .px(px(spacing::S2))
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child("More like this"),
            )
            .children(
                button.map(|b| div().debug_selector(|| "more-like-refresh".into()).child(b)),
            )
            .into_any_element()
    }

    /// One suggested song: a playlist row with a + for its More button.
    fn suggestion_row(
        &mut self,
        pos: usize,
        frame: RowFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.more.read(cx).songs().get(pos).cloned() else {
            return div().into_any_element();
        };
        let ix = frame.ix;
        let hovered = frame.level > 0.0;
        let playing =
            self.player.read(cx).current_video().as_deref() == Some(row.track.video_id.as_str());
        let video: String = row.track.video_id.as_str().to_owned();
        let art = self.art_tile(
            row.thumbnail.as_deref(),
            ArtSize::List,
            TRACK_ART,
            Icon::MusicNote,
            color::text_tertiary(),
            cx,
        );
        let artists: SharedString = row
            .track
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
            .into();
        let duration: SharedString = row
            .track
            .duration
            .map(|d| format_time(d.as_millis() as u64))
            .unwrap_or_default()
            .into();
        let show_album = f32::from(window.viewport_size().width) >= ALBUM_COLUMN_MIN_WINDOW;
        let album: Option<SharedString> = row.track.album.as_ref().map(|a| a.title.clone().into());
        // Quiet until the pointer is on the row.
        let text = if hovered || frame.selected || playing {
            color::text_primary()
        } else {
            color::text_secondary()
        };
        let liked = self.writes.read(cx).is_liked(&video);
        let accent = frame.accent;
        let subject = Subject::track(row.track.clone(), row.thumbnail.clone());
        let heart = div()
            .relative()
            .size(px(ACTION_SIZE))
            .flex_none()
            .when(hovered || frame.selected || liked, |d| {
                let (writes, video) = (self.writes.clone(), video.clone());
                d.child(heart_action(("more-like-like", ix), liked, move |_, cx| {
                    writes.update(cx, |w, cx| w.toggle_like(&video, cx))
                }))
            });
        let plus = div()
            .relative()
            .size(px(ACTION_SIZE))
            .flex_none()
            .debug_selector(move || format!("more-like-add-{pos}"))
            .child(row_action(
                ("more-like-add", ix),
                Icon::Add,
                "Add to playlist",
                color::text_secondary(),
                {
                    let (more, video) = (self.more.clone(), video.clone());
                    move |_, cx| more.update(cx, |m, cx| m.add(&video, cx))
                },
            ));

        self.row_shell(
            &frame,
            TRACK_ROW,
            playing,
            Some(row.track.clone()),
            subject,
            {
                let video = video.clone();
                move |this, _, cx| {
                    // The suggestions are the queue, from this song on.
                    let songs = this.more.read(cx).songs().to_vec();
                    let Some(start) = songs
                        .iter()
                        .position(|r| r.track.video_id.as_str() == video)
                    else {
                        return;
                    };
                    let tracks = songs.iter().map(|r| r.track.clone()).collect();
                    this.player.update(cx, |p, cx| {
                        for song in &songs {
                            p.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
                        }
                        p.play_tracks_from(
                            tracks,
                            Some(start),
                            None,
                            Some(queue_source::named("More like this")),
                            cx,
                        );
                    });
                }
            },
            cx,
        )
        .aria_label(row.track.title.clone())
        .when(playing, |d| {
            d.child(playing_row::marker(color::accent(accent)))
        })
        // The playlist's rows have a number here; these have none, and keep the art in line.
        .child(div().w(px(INDEX_WIDTH)).flex_none())
        .child(art)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(Self::line(
                    ("more-like-title", ix),
                    row.track.title.clone().into(),
                    playing_row::title_style(typography::LIST_TITLE, playing),
                    text,
                    false,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(spacing::S1))
                        .min_w_0()
                        .when(row.explicit, |d| {
                            d.child(
                                style::text_style(div(), typography::CAPTION, cx)
                                    .id(("more-like-explicit", ix))
                                    .flex_none()
                                    .text_color(color::text_tertiary())
                                    .aria_label("Explicit")
                                    .child("E"),
                            )
                        })
                        .child(Self::line(
                            ("more-like-artist", ix),
                            artists,
                            typography::BODY,
                            text,
                            false,
                            cx,
                        )),
                ),
        )
        .when(show_album, |d| {
            d.child(div().flex_1().min_w_0().children(album.map(|a| {
                Self::line(
                    ("more-like-album", ix),
                    a,
                    typography::BODY,
                    text,
                    false,
                    cx,
                )
            })))
        })
        .child(heart)
        // Where the playlist's rows have their own add-to-playlist button.
        .child(div().size(px(ACTION_SIZE)).flex_none())
        .child(plus)
        .child(
            div()
                .w(px(DURATION_WIDTH))
                .flex_none()
                .flex()
                .justify_end()
                .child(Self::line(
                    ("more-like-duration", ix),
                    duration,
                    typography::CAPTION,
                    color::text_tertiary(),
                    true,
                    cx,
                )),
        )
        .into_any_element()
    }
}
