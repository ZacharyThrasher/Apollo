//! Songs dragged out of a list: the payload any song row carries, and the ghost that follows the
//! pointer. The docked Crate panel takes them (`ui::crate_page` in its dock layout), so a song
//! can be set aside at an exact place by dropping it there instead of pressing C.
//!
//! The payload is the row's own menu subject, so a picked row drags every picked song and the
//! drop adds them in list order. A row that already drags for its own list (an up-next queue row,
//! a row of a playlist of your own, a Crate row) keeps its own payload and carries a
//! [`SongDrag`] inside it.

use apollo_theme::tokens::{radius, spacing, typography};
use gpui::{
    div, prelude::*, px, AppContext, Context, IntoElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use crate::style::{self, color};
use crate::ui::track_menu::{Subject, TrackSubject};

/// The ghost's width and height (a 56px row less its ring inset).
const GHOST_WIDTH: f32 = 280.0;
const GHOST_HEIGHT: f32 = 48.0;

/// The songs being dragged, in list order.
#[derive(Clone)]
pub struct SongDrag {
    pub songs: Vec<TrackSubject>,
    title: SharedString,
    artist: SharedString,
}

impl SongDrag {
    pub fn new(songs: Vec<TrackSubject>) -> Option<Self> {
        let first = songs.first()?;
        let (title, artist): (SharedString, SharedString) = if songs.len() == 1 {
            (
                first.track.title.clone().into(),
                first
                    .track
                    .artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
            )
        } else {
            (format!("{} songs", songs.len()).into(), "".into())
        };
        Some(Self {
            songs,
            title,
            artist,
        })
    }

    /// The songs a row's menu subject stands for; none for an album card, the queue or an artist.
    pub fn of(subject: &Subject) -> Option<Self> {
        match subject {
            Subject::Track(song) => Self::new(vec![song.clone()]),
            Subject::Tracks(songs) => Self::new(songs.clone()),
            _ => None,
        }
    }
}

/// The ghost that follows the pointer.
pub struct SongGhost(SongDrag);

impl Render for SongGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // On bg.overlay, so the lowest text tier is text.secondary.
        div()
            .w(px(GHOST_WIDTH))
            .h(px(GHOST_HEIGHT))
            .px(px(spacing::S3))
            .flex()
            .flex_col()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_strong())
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .truncate()
                    .text_color(color::text_primary())
                    .child(self.0.title.clone()),
            )
            .when(!self.0.artist.is_empty(), |d| {
                d.child(
                    style::text_style(div(), typography::BODY, cx)
                        .truncate()
                        .text_color(color::text_secondary())
                        .child(self.0.artist.clone()),
                )
            })
    }
}

/// Make `el` (a song row) draggable with the songs of `subject`; a subject that is no song (a
/// card, the queue) leaves the row as it was.
pub fn draggable<E: StatefulInteractiveElement>(el: E, subject: &Subject) -> E {
    match SongDrag::of(subject) {
        Some(drag) => el.on_drag(drag, ghost),
        None => el,
    }
}

/// What a drag starts with: the ghost for `drag`.
pub fn ghost(
    drag: &SongDrag,
    _: gpui::Point<gpui::Pixels>,
    _: &mut Window,
    cx: &mut gpui::App,
) -> gpui::Entity<SongGhost> {
    cx.new(|_| SongGhost(drag.clone()))
}
