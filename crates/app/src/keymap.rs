//! App-wide keyboard shortcuts (the list is in `docs/keyboard.md`).
//!
//! * **In-app keys** are bound on the shell, except while a text field or a popup (the track
//!   menu, the playlist picker) has focus, so typing "m" or "l" in Search types the letter.
//!   A deeper context wins where it binds the same key: a focused slider steps with Left/Right,
//!   the queue list and Search results take Up/Down and L for their own cursor.
//! * **The plain arrows only navigate.** They move the cursor or the focus (see
//!   `Shell::navigate`); playback and volume sit on chords: Shift+Left/Right change song,
//!   Ctrl+Left/Right seek, Ctrl+Up/Down change the volume.
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
        LikeCurrent,
        CrateCurrent,
        ToggleCrateDock,
        AddCurrentToPlaylist,
        AddCurrentToTarget,
        UndoQueue,
        RedoQueue,
        ToggleFullscreen,
        ShowShortcuts,
        GoBack,
        GoForward,
        SkimCrate,
        SkimKeep,
        SkimCancel,
        NavigateUp,
        NavigateDown,
        NavigateLeft,
        NavigateRight
    ]
);

/// How far Ctrl+Left/Right seek, in milliseconds.
pub const SEEK_STEP_MS: i64 = 5_000;
/// How far Ctrl+Up/Down move the level, in percent.
pub const VOLUME_STEP: i8 = 5;

/// Where the plain (unmodified) shortcuts apply: anywhere in the shell unless a text field or a
/// popup owns the keys.
const PLAIN: &str = "Shell && !TextField && !TrackMenu && !PlaylistPicker && !CheatSheet";

/// The extra key context the shell carries while Skim runs: K, Enter and Esc mean something
/// else only then.
pub const SKIMMING: &str = "Skimming";

/// Where the Skim keys apply: the plain keys' places, while skimming.
const SKIM_PLAIN: &str = "Shell && Skimming && !TextField && !TrackMenu && !PlaylistPicker && !CheatSheet";

/// The in-app shortcuts. Space, Ctrl+K/F and Esc are bound with the shell's own list.
pub fn bind_keys(cx: &mut App) {
    let plain = Some(PLAIN);
    cx.bind_keys([
        KeyBinding::new("m", ToggleMute, plain),
        // Plain arrows navigate; a list, grid or slider that has focus binds them itself and
        // wins. These four are what is left when it does not.
        KeyBinding::new("up", NavigateUp, plain),
        KeyBinding::new("down", NavigateDown, plain),
        KeyBinding::new("left", NavigateLeft, plain),
        KeyBinding::new("right", NavigateRight, plain),
        KeyBinding::new("ctrl-up", VolumeUp, plain),
        KeyBinding::new("ctrl-down", VolumeDown, plain),
        KeyBinding::new("ctrl-left", SeekBack, plain),
        KeyBinding::new("ctrl-right", SeekForward, plain),
        KeyBinding::new("shift-left", PreviousTrack, plain),
        KeyBinding::new("shift-right", NextTrack, plain),
        KeyBinding::new("ctrl-l", ToggleLyrics, plain),
        KeyBinding::new("l", LikeCurrent, plain),
        KeyBinding::new("c", CrateCurrent, plain),
        KeyBinding::new("ctrl-shift-c", ToggleCrateDock, plain),
        KeyBinding::new("p", AddCurrentToPlaylist, plain),
        KeyBinding::new("a", AddCurrentToTarget, plain),
        KeyBinding::new("ctrl-z", UndoQueue, plain),
        KeyBinding::new("ctrl-shift-z", RedoQueue, plain),
        KeyBinding::new("ctrl-y", RedoQueue, plain),
        KeyBinding::new("f11", ToggleFullscreen, plain),
        // "?" is Shift+/ on most layouts; a layout that types it on its own key sends "?".
        KeyBinding::new("shift-/", ShowShortcuts, plain),
        KeyBinding::new("?", ShowShortcuts, plain),
        // Backspace goes back like a browser's, but never in a text field (it deletes there).
        KeyBinding::new("backspace", GoBack, plain),
    ]);
    // Alt+Left / Alt+Right work wherever focus is, text fields included (no field uses them).
    cx.bind_keys([
        KeyBinding::new("alt-left", GoBack, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-right", GoForward, Some(KEY_CONTEXT)),
    ]);
    // Bound after the shell's own Esc (Close Now Playing), so while skimming Esc ends the skim.
    let skim = Some(SKIM_PLAIN);
    cx.bind_keys([
        KeyBinding::new("k", SkimCrate, skim),
        KeyBinding::new("enter", SkimKeep, skim),
        KeyBinding::new("escape", SkimCancel, skim),
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
    /// Shows or hides the mini player (the HUD).
    ToggleHud,
}

/// A key a global hotkey can sit on (always with Ctrl+Alt held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalKey {
    Space,
    Left,
    Right,
    Up,
    Down,
    H,
}

/// The global hotkeys: every one is Ctrl+Alt plus the key.
pub const GLOBAL_BINDINGS: [(GlobalCommand, GlobalKey); 6] = [
    (GlobalCommand::PlayPause, GlobalKey::Space),
    (GlobalCommand::Next, GlobalKey::Right),
    (GlobalCommand::Previous, GlobalKey::Left),
    (GlobalCommand::VolumeUp, GlobalKey::Up),
    (GlobalCommand::VolumeDown, GlobalKey::Down),
    (GlobalCommand::ToggleHud, GlobalKey::H),
];

impl GlobalCommand {
    pub fn apply(self, player: &mut PlayerModel, cx: &mut Context<PlayerModel>) {
        match self {
            GlobalCommand::PlayPause => player.os_command(OsCommand::Toggle, cx),
            GlobalCommand::Next => player.os_command(OsCommand::Next, cx),
            GlobalCommand::Previous => player.os_command(OsCommand::Previous, cx),
            GlobalCommand::VolumeUp => player.nudge_volume(VOLUME_STEP, cx),
            GlobalCommand::VolumeDown => player.nudge_volume(-VOLUME_STEP, cx),
            // The window's, not the player's: the hotkey listener handles it.
            GlobalCommand::ToggleHud => {}
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
            GlobalCommand::ToggleHud,
        ] {
            let n = GLOBAL_BINDINGS
                .iter()
                .filter(|(c, _)| *c == command)
                .count();
            assert_eq!(n, 1, "{command:?}");
        }
    }
}
