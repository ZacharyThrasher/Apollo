//! C on a shelf's song card (Home, Explore, Related): the card the keyboard ring is on, else the
//! card under the pointer, else (the shell's own C) the playing song.
//!
//! The card under the pointer is remembered here: the cards say so as the pointer enters and
//! leaves, and the shell reads it when C reaches it with no card holding the keyboard. It never
//! starts any work on hover, only notes which song it is.

use apollo_domain::model::Track;
use gpui::{App, Global};

use crate::ui::track_menu::Subject;

/// The song card the pointer is over.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hovered {
    /// The card's own id, so a card the pointer left does not clear the one it moved on to.
    card: String,
    track: Track,
    thumbnail: Option<String>,
}

#[derive(Default)]
struct HoverState(Option<Hovered>);

impl Global for HoverState {}

/// The song a card stands for (`None` for a menu that is not about one song).
fn song_of(menu: &Subject) -> Option<(Track, Option<String>)> {
    match menu {
        Subject::Track(t) => Some((t.track.clone(), t.thumbnail.clone())),
        _ => None,
    }
}

/// The pointer entered (`on`) or left card `card`, whose menu is `menu`.
pub fn hover(card: &str, menu: &Subject, on: bool, cx: &mut App) {
    if on {
        let Some((track, thumbnail)) = song_of(menu) else {
            return;
        };
        cx.set_global(HoverState(Some(Hovered {
            card: card.to_owned(),
            track,
            thumbnail,
        })));
    } else if cx
        .try_global::<HoverState>()
        .and_then(|s| s.0.as_ref())
        .is_some_and(|h| h.card == card)
    {
        cx.set_global(HoverState(None));
    }
}

/// Forget the card under the pointer: the page it was on went away without telling.
pub fn clear(cx: &mut App) {
    if cx.try_global::<HoverState>().is_some_and(|s| s.0.is_some()) {
        cx.set_global(HoverState(None));
    }
}

/// The song under the pointer, if a card has said so.
pub fn hovered(cx: &App) -> Option<(Track, Option<String>)> {
    cx.try_global::<HoverState>()
        .and_then(|s| s.0.as_ref())
        .map(|h| (h.track.clone(), h.thumbnail.clone()))
}

/// C on a card that holds the keyboard. `menu` is the card's own song; `ring` is whether its
/// keyboard ring was showing when it was last drawn. With the ring showing the card is the
/// target; without it (focus left over from a click) a card under the pointer is, else the card.
pub fn crate_card(menu: &Subject, ring: bool, cx: &mut App) {
    let target = if ring {
        song_of(menu)
    } else {
        hovered(cx).or_else(|| song_of(menu))
    };
    if let Some((track, thumbnail)) = target {
        crate::crate_list::add_card_song(track, thumbnail, cx);
    }
}

/// C reached the shell with no card holding the keyboard: the card under the pointer, if any.
/// Returns whether one took it.
pub fn crate_hovered(cx: &mut App) -> bool {
    match hovered(cx) {
        Some((track, thumbnail)) => {
            crate::crate_list::add_card_song(track, thumbnail, cx);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(id: &str) -> Subject {
        Subject::track(Track::new(id, format!("Song {id}")), Some(format!("https://art/{id}")))
    }

    #[gpui::test]
    fn the_card_under_the_pointer_is_remembered_until_it_leaves(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(hovered(cx).is_none());
            hover("a", &menu("aaaaaaaaaaa"), true, cx);
            assert_eq!(hovered(cx).map(|(t, _)| t.video_id.as_str().to_owned()), Some("aaaaaaaaaaa".into()));
            // The pointer moved straight to another card: the first card's "left" comes after.
            hover("b", &menu("bbbbbbbbbbb"), true, cx);
            hover("a", &menu("aaaaaaaaaaa"), false, cx);
            assert_eq!(hovered(cx).map(|(t, _)| t.video_id.as_str().to_owned()), Some("bbbbbbbbbbb".into()));
            hover("b", &menu("bbbbbbbbbbb"), false, cx);
            assert!(hovered(cx).is_none());
            hover("c", &menu("ccccccccccc"), true, cx);
            clear(cx);
            assert!(hovered(cx).is_none());
        });
    }

    #[gpui::test]
    fn a_menu_that_is_not_one_song_is_not_remembered(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            hover("q", &Subject::Queue, true, cx);
            assert!(hovered(cx).is_none());
        });
    }
}
