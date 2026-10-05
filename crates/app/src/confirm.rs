//! Add confirmations: what the app says when something has been added to a list.
//!
//! Every add (to the queue, the Crate, a playlist, the library, a like) calls [`confirm`] once,
//! after the write has gone through. Two things follow, both switched off by Settings > Playlists
//! > Confirm when adding:
//!
//! * the shell shows the one-line tip pill ("Added to Road trip") for [`CONFIRM_FOR`]; a newer add
//!   replaces it, so a run of adds never stacks;
//! * where the destination is on screen (the Queue and Crate buttons, the sidebar's Crate, the
//!   target playlist pill, the docked Crate header, an added row in an open list) it lights up
//!   with a single [`PULSE`]: bg.overlay fading in and out. No accent, no scaling.
//!
//! The pill ends with " · Ctrl+Z to undo" when the add can be undone and shortcut hints are on.
//! Nothing here is drawn for a failure; the existing error toast says that.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, Animation, AnimationExt, AnyElement, App, Context, Entity, EventEmitter,
    Global, Hsla, SharedString,
};

use crate::settings::{SettingsModel, CONFIRM_ADDS};
use crate::style::color;

/// How long the confirming pill stays (fade-out included).
pub const CONFIRM_FOR: Duration = Duration::from_millis(2500);
/// One fade in and out of the destination's highlight.
pub const PULSE: Duration = Duration::from_millis(400);
/// A playlist's name is cut to this many characters in the pill.
const TITLE_MAX: usize = 32;

/// Where an add lands, when that place can be on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Queue,
    Crate,
    /// The target playlist (its pill in the player bar).
    Target,
}

/// What the pill says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub text: SharedString,
    /// Ctrl+Z takes this add back.
    pub undoable: bool,
}

/// The shell listens for this and shows the pill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmEvent {
    Show(Confirmation),
}

#[derive(Clone, Debug)]
struct Pulse {
    seq: u64,
    place: Place,
    /// Songs added, for the rows that show them (empty: only the place itself lights up).
    rows: Vec<String>,
    /// An open list scrolls to the last of `rows` (not for a drop, which lands where the pointer
    /// already is).
    scroll: bool,
}

pub struct ConfirmModel {
    settings: Entity<SettingsModel>,
    seq: u64,
    pulse: Option<Pulse>,
}

impl EventEmitter<ConfirmEvent> for ConfirmModel {}

/// The shell's confirmations, reachable from any model.
#[derive(Clone)]
pub struct ConfirmHandle(pub Entity<ConfirmModel>);

impl Global for ConfirmHandle {}

impl ConfirmModel {
    pub fn new(settings: Entity<SettingsModel>) -> Self {
        Self {
            settings,
            seq: 0,
            pulse: None,
        }
    }

    /// Settings > Playlists > Confirm when adding.
    pub fn enabled(&self, cx: &App) -> bool {
        self.settings.read(cx).get(&CONFIRM_ADDS)
    }

    fn show(
        &mut self,
        text: String,
        undoable: bool,
        pulse: Option<(Place, Vec<String>, bool)>,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled(cx) {
            return;
        }
        self.seq += 1;
        let seq = self.seq;
        self.pulse = pulse.map(|(place, rows, scroll)| Pulse { seq, place, rows, scroll });
        cx.emit(ConfirmEvent::Show(Confirmation {
            text: text.into(),
            undoable,
        }));
        cx.notify();
        if self.pulse.is_some() {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PULSE).await;
                let _ = this.update(cx, |m, cx| {
                    if m.pulse.as_ref().is_some_and(|p| p.seq == seq) {
                        m.pulse = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

impl ConfirmModel {
    /// Say something that is not an add (a sort of the Crate): the pill, without a pulse and
    /// whatever Confirm when adding says, since that setting is about adds.
    fn announce(&mut self, text: String, undoable: bool, cx: &mut Context<Self>) {
        cx.emit(ConfirmEvent::Show(Confirmation {
            text: text.into(),
            undoable,
        }));
    }
}

/// The shell's confirmations, where there is a shell.
pub fn handle(cx: &App) -> Option<Entity<ConfirmModel>> {
    cx.try_global::<ConfirmHandle>().map(|h| h.0.clone())
}

/// Say that something was added. `place` and `rows` say what lights up (see the module docs).
/// Does nothing where there is no shell, or the setting is off.
pub fn confirm(
    text: String,
    undoable: bool,
    pulse: Option<(Place, Vec<String>, bool)>,
    cx: &mut App,
) {
    if let Some(model) = handle(cx) {
        model.update(cx, |m, cx| m.show(text, undoable, pulse, cx));
    }
}

/// Redraw `view` when a pulse starts or ends (views that draw one call this once, when made).
pub fn watch<T: 'static>(cx: &mut Context<T>) {
    if let Some(model) = handle(cx) {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
    }
}

/// The pulse now on `place`, as the number that keys its animation. None when there is none, and
/// under reduced motion (the pill alone is the confirmation then).
pub fn pulse(place: Place, cx: &App) -> Option<u64> {
    if cx.reduce_motion() {
        return None;
    }
    let model = handle(cx)?;
    let model = model.read(cx);
    model.pulse.as_ref().filter(|p| p.place == place).map(|p| p.seq)
}

/// The pulse for the row of `video_id` in the list at `place`.
pub fn row_pulse(place: Place, video_id: &str, cx: &App) -> Option<u64> {
    if cx.reduce_motion() {
        return None;
    }
    let model = handle(cx)?;
    let model = model.read(cx);
    model
        .pulse
        .as_ref()
        .filter(|p| p.place == place && p.rows.iter().any(|r| r == video_id))
        .map(|p| p.seq)
}

/// The songs an open list at `place` should scroll to (the last of them), once per add: the
/// number says which add, so a list that draws again does not scroll again. Independent of
/// reduced motion (a jump is not an animation).
pub fn scroll_to(place: Place, cx: &App) -> Option<(u64, Vec<String>)> {
    let model = handle(cx)?;
    let model = model.read(cx);
    model
        .pulse
        .as_ref()
        .filter(|p| p.place == place && p.scroll && !p.rows.is_empty())
        .map(|p| (p.seq, p.rows.clone()))
}

/// How far into its fade a pulse is: 0 at the ends, 1 in the middle.
pub fn tent(t: f32) -> f32 {
    1.0 - (2.0 * t.clamp(0.0, 1.0) - 1.0).abs()
}

/// The highlight itself: an absolutely placed layer over its (relative) parent, `colour` fading in
/// and out once. It takes no mouse events.
pub fn overlay(seq: u64, colour: Hsla, radius: f32) -> AnyElement {
    div()
        .absolute()
        .size_full()
        .rounded(px(radius))
        .bg(colour.opacity(0.0))
        .with_animation(
            ("add-pulse", seq),
            Animation::new(PULSE),
            move |el, t| el.bg(colour.opacity(tent(t))),
        )
        .into_any_element()
}

/// [`overlay`] in bg.overlay, the pulse's own colour.
pub fn highlight(seq: u64, radius: f32) -> AnyElement {
    overlay(seq, color::bg_overlay(), radius)
}

/// A container that holds `child`ren and, while `seq` is a pulse, the highlight behind them.
pub fn pulsing(seq: Option<u64>, radius: f32) -> gpui::Div {
    div()
        .relative()
        .flex_none()
        .children(seq.map(|seq| highlight(seq, radius)))
}

// ---- words ------------------------------------------------------------------------------

/// "Added to Queue" / "Added 3 songs to Queue".
pub fn added_text(count: usize, destination: &str) -> String {
    if count <= 1 {
        format!("Added to {destination}")
    } else {
        format!("Added {count} songs to {destination}")
    }
}

/// A playlist's name as the pill shows it: cut if long.
pub fn short_title(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() <= TITLE_MAX {
        return title.to_owned();
    }
    let mut cut: String = title.chars().take(TITLE_MAX - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// The pill's text with the undo hint, when there is one to give.
pub fn with_undo_hint(text: &str, undoable: bool, hints_on: bool) -> String {
    if undoable && hints_on {
        format!("{text} \u{b7} Ctrl+Z to undo")
    } else {
        text.to_owned()
    }
}

// ---- the adds ---------------------------------------------------------------------------

/// The Crate was rearranged ("Sorted by artist"), or there was nothing to rearrange (`undoable`
/// false). Says so in the pill and, when Ctrl+Z takes it back and shortcut hints are on, says that
/// too. Not an add, so Confirm when adding does not switch it off.
pub fn arranged(text: String, undoable: bool, cx: &mut App) {
    if let Some(model) = handle(cx) {
        model.update(cx, |m, cx| m.announce(text, undoable, cx));
    }
}

/// Songs went into the queue (at the end, or right after the playing one).
pub fn queued(videos: Vec<String>, cx: &mut App) {
    if videos.is_empty() {
        return;
    }
    confirm(
        added_text(videos.len(), "Queue"),
        true,
        Some((Place::Queue, videos, true)),
        cx,
    );
}

/// Songs went into the Crate.
pub fn crated(videos: Vec<String>, scroll: bool, cx: &mut App) {
    if videos.is_empty() {
        return;
    }
    confirm(
        added_text(videos.len(), "Crate"),
        true,
        Some((Place::Crate, videos, scroll)),
        cx,
    );
}

/// What was asked for is in the Crate already.
pub fn already_crated(cx: &mut App) {
    confirm("Already in Crate".to_owned(), false, Some((Place::Crate, Vec::new(), false)), cx);
}

/// The pill for an add of many songs to the Crate: "Added 24 songs to Crate", "Added 18 · 6
/// already in Crate" when some were held, "Already in Crate" when all were. `partial` marks a list
/// that was still loading when it was read.
pub fn crated_many_text(added: usize, held: usize, partial: bool) -> String {
    let mut text = match (added, held) {
        (0, _) => "Already in Crate".to_owned(),
        (n, 0) => added_text(n, "Crate"),
        (n, held) => format!("Added {n} · {held} already in Crate"),
    };
    if partial {
        text.push_str(" · still loading");
    }
    text
}

/// Songs went into the Crate in one add (see [`crated_many_text`]); `held` is how many were in
/// it already.
pub fn crated_many(videos: Vec<String>, held: usize, partial: bool, cx: &mut App) {
    let text = crated_many_text(videos.len(), held, partial);
    // Ctrl+Z takes back whatever went in; an open list scrolls to the last of it.
    let undoable = !videos.is_empty();
    confirm(text, undoable, Some((Place::Crate, videos, true)), cx);
}

/// Songs went into a playlist; the service has said so.
pub fn added_to_playlist(playlist_id: &str, title: &str, count: usize, cx: &mut App) {
    let place = crate::target_playlist::is_target(playlist_id, cx).then_some((Place::Target, Vec::new(), false));
    confirm(added_text(count, &short_title(title)), true, place, cx);
}

/// What was asked for is in the playlist already.
pub fn already_in_playlist(title: &str, cx: &mut App) {
    confirm(format!("Already in {}", short_title(title)), false, None, cx);
}

/// A song or collection went into the library.
pub fn saved_to_library(cx: &mut App) {
    confirm("Saved to library".to_owned(), false, None, cx);
}

/// Songs were liked.
pub fn liked(count: usize, cx: &mut App) {
    let text = if count <= 1 {
        "Liked".to_owned()
    } else {
        format!("Liked {count} songs")
    };
    confirm(text, false, None, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(added_text(1, "Queue"), "Added to Queue");
        assert_eq!(added_text(3, "Road trip"), "Added 3 songs to Road trip");
        assert_eq!(short_title("  Road trip "), "Road trip");
        assert_eq!(short_title(&"x".repeat(40)).chars().count(), 32);
        assert!(short_title(&"x".repeat(40)).ends_with('\u{2026}'));
        assert_eq!(with_undo_hint("Added to Queue", true, true), "Added to Queue \u{b7} Ctrl+Z to undo");
        assert_eq!(with_undo_hint("Added to Queue", true, false), "Added to Queue");
        assert_eq!(with_undo_hint("Liked", false, true), "Liked");
        assert_eq!(crated_many_text(24, 0, false), "Added 24 songs to Crate");
        assert_eq!(crated_many_text(1, 0, false), "Added to Crate");
        assert_eq!(crated_many_text(18, 6, false), "Added 18 · 6 already in Crate");
        assert_eq!(crated_many_text(0, 6, false), "Already in Crate");
        assert_eq!(crated_many_text(5, 0, true), "Added 5 songs to Crate · still loading");
    }

    #[test]
    fn the_pulse_is_zero_at_both_ends_and_one_in_the_middle() {
        assert_eq!(tent(0.0), 0.0);
        assert_eq!(tent(0.5), 1.0);
        assert_eq!(tent(1.0), 0.0);
    }
}
