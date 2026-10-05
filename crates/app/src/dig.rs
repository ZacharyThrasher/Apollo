//! Dig from here, the settings side: Settings > Playlists > Dig key (D on a song digs from it) and
//! Leave out songs I already have. The dig itself is `PlayerModel::dig_from` (`player/dig.rs`).
//! Both read the shell's settings through the handle auto-advance installed, and mean "on" where
//! there is no shell to say otherwise.

use gpui::App;

use crate::auto_advance::AutoAdvanceHandle;
use crate::settings::{DIG_KEY, DIG_LEAVE_OUT};

/// Settings > Playlists > Dig key.
pub fn key_enabled(cx: &App) -> bool {
    cx.try_global::<AutoAdvanceHandle>()
        .is_none_or(|h| h.0.read(cx).get(&DIG_KEY))
}

/// Settings > Playlists > Leave out songs I already have.
pub fn leave_out(cx: &App) -> bool {
    cx.try_global::<AutoAdvanceHandle>()
        .is_none_or(|h| h.0.read(cx).get(&DIG_LEAVE_OUT))
}
