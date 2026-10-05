//! Auto-advance: after C, A or K acts on a list's cursor row, the cursor steps to the next row,
//! so collecting is one finger on one key (Lightroom's "Auto Advance" after flagging). Settings >
//! Playlists > "Move to the next song after C, A or K" switches it (on by default).
//!
//! A list asks [`next_row`] after the key did its work. Picked songs win: with two or more picked
//! the key acts on them and the cursor stays where it is, so a bulk C does not walk away.

use gpui::{App, Entity, Global};

use crate::settings::{SettingsModel, AUTO_ADVANCE};

/// The shell's settings, reachable from any list's key handler.
#[derive(Clone)]
pub struct AutoAdvanceHandle(pub Entity<SettingsModel>);

impl Global for AutoAdvanceHandle {}

/// Settings > Playlists > Move to the next song after C, A or K. On where there is no shell to
/// say otherwise.
pub fn enabled(cx: &App) -> bool {
    cx.try_global::<AutoAdvanceHandle>()
        .is_none_or(|h| h.0.read(cx).get(&AUTO_ADVANCE))
}

/// The row the cursor steps to after the key acted on row `at` of `len` rows: the next one, or
/// none at the last row (the cursor stays) or when the switch is off.
pub fn next_row(at: usize, len: usize, cx: &App) -> Option<usize> {
    step(at, len).filter(|_| enabled(cx))
}

fn step(at: usize, len: usize) -> Option<usize> {
    (at + 1 < len).then_some(at + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_steps_down_and_stays_at_the_last_row() {
        assert_eq!(step(0, 3), Some(1));
        assert_eq!(step(1, 3), Some(2));
        assert_eq!(step(2, 3), None);
        assert_eq!(step(0, 1), None);
        assert_eq!(step(0, 0), None);
    }
}
