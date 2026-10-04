//! App-wide keyboard shortcuts (the list is in `docs/keyboard.md`).
//!
//! * **In-app keys** are bound on the shell, except while a text field or a popup (the track
//!   menu, the playlist picker) has focus, so typing "m" or "l" in Search types the letter.
//!   A deeper context wins where it binds the same key: a focused slider steps with Left/Right,
//!   the queue list and Search results take Up/Down and L for their own cursor.
//! * **Global hotkeys** work while Apollo is not focused (`hotkeys_win.rs` registers them with
//!   Windows, only while Settings > Keyboard > Global hotkeys is on). What each one does is
//!   [`GlobalCommand::apply`]; which keys they are is [`GLOBAL_BINDINGS`]. Media keys are the
//!   SMTC bridge's and are not touched here.

use gpui::{actions, App, Context, KeyBinding};

use crate::os_media::OsCommand;
use crate::player::PlayerModel;
use crate::ui::shell::{NextTrack, PreviousTrack, KEY_CONTEXT};

actions!(
    apollo_keys,
    [
        ToggleMute,
        VolumeUp,
        VolumeDown,
        SeekBack,
        SeekForward,
        ToggleLyrics,
        LikeCurrent
    ]
);

/// How far Left/Right seek, in milliseconds.
pub const SEEK_STEP_MS: i64 = 5_000;
/// How far a volume key moves the level, in percent.
pub const VOLUME_STEP: i8 = 5;

/// Where the plain (unmodified) shortcuts apply: anywhere in the shell unless a text field or a
/// popup owns the keys.
const PLAIN: &str = "Shell && !TextField && !TrackMenu && !PlaylistPicker";

/// The in-app shortcuts. Space, Ctrl+K/F and Esc are bound with the shell's own list.
pub fn bind_keys(cx: &mut App) {
    let plain = Some(PLAIN);
    cx.bind_keys([
        KeyBinding::new("m", ToggleMute, plain),
        KeyBinding::new("up", VolumeUp, plain),
        KeyBinding::new("down", VolumeDown, plain),
        KeyBinding::new("left", SeekBack, plain),
        KeyBinding::new("right", SeekForward, plain),
        KeyBinding::new("shift-left", PreviousTrack, plain),
        KeyBinding::new("shift-right", NextTrack, plain),
        KeyBinding::new("ctrl-l", ToggleLyrics, plain),
        KeyBinding::new("l", LikeCurrent, plain),
    ]);
    // The shell context is what the plain keys hang off; keep the constant honest.
    debug_assert!(PLAIN.starts_with(KEY_CONTEXT));
}

/// What a global hotkey does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalCommand {
    PlayPause,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
}

/// A key a global hotkey can sit on (always with Ctrl+Alt held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalKey {
    Space,
    Left,
    Right,
    Up,
    Down,
}

/// The global hotkeys: every one is Ctrl+Alt plus the key.
pub const GLOBAL_BINDINGS: [(GlobalCommand, GlobalKey); 5] = [
    (GlobalCommand::PlayPause, GlobalKey::Space),
    (GlobalCommand::Next, GlobalKey::Right),
    (GlobalCommand::Previous, GlobalKey::Left),
    (GlobalCommand::VolumeUp, GlobalKey::Up),
    (GlobalCommand::VolumeDown, GlobalKey::Down),
];

impl GlobalCommand {
    pub fn apply(self, player: &mut PlayerModel, cx: &mut Context<PlayerModel>) {
        match self {
            GlobalCommand::PlayPause => player.os_command(OsCommand::Toggle, cx),
            GlobalCommand::Next => player.os_command(OsCommand::Next, cx),
            GlobalCommand::Previous => player.os_command(OsCommand::Previous, cx),
            GlobalCommand::VolumeUp => player.nudge_volume(VOLUME_STEP, cx),
            GlobalCommand::VolumeDown => player.nudge_volume(-VOLUME_STEP, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_hotkeys_do_not_share_a_key() {
        for (i, (_, a)) in GLOBAL_BINDINGS.iter().enumerate() {
            for (_, b) in &GLOBAL_BINDINGS[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn every_global_command_is_bound_once() {
        for command in [
            GlobalCommand::PlayPause,
            GlobalCommand::Next,
            GlobalCommand::Previous,
            GlobalCommand::VolumeUp,
            GlobalCommand::VolumeDown,
        ] {
            let n = GLOBAL_BINDINGS
                .iter()
                .filter(|(c, _)| *c == command)
                .count();
            assert_eq!(n, 1, "{command:?}");
        }
    }
}
