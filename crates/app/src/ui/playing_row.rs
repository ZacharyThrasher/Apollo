//! How the row that is playing looks, shared by the queue panel and every track list so the
//! two cannot drift apart (DIRECTION: queue decision 2, library decision 4): the nav's
//! selected-row marker (a 3px accent bar), the selected tint, and a body-strong title.

use apollo_theme::tokens::{spacing, typography::TextStyle};
use gpui::{div, prelude::*, px, Div, Hsla};

use crate::style::color;

/// Width of the accent bar.
pub const MARKER_WIDTH: f32 = 3.0;

/// The accent bar at a row's left edge. Place it inside a `relative` row.
pub fn marker(accent: Hsla) -> Div {
    div()
        .absolute()
        .left_0()
        .top(px(spacing::S2))
        .bottom(px(spacing::S2))
        .w(px(MARKER_WIDTH))
        .rounded_full()
        .bg(accent)
}

/// The row's background when it is playing or the keyboard cursor is on it.
pub fn tint() -> Hsla {
    color::state_selected()
}

/// The title style for a row: the playing row's title is strong, at the row's own size.
pub fn title_style(base: TextStyle, playing: bool) -> TextStyle {
    if playing {
        TextStyle::new(base.size_px, 600)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_theme::tokens::typography::{BODY, BODY_STRONG, LIST_TITLE};

    #[test]
    fn the_playing_title_is_strong_at_its_own_size() {
        assert_eq!(title_style(BODY, true), BODY_STRONG);
        assert_eq!(title_style(BODY, false), BODY);
        assert_eq!(title_style(LIST_TITLE, true), TextStyle::new(15.0, 600));
    }
}
