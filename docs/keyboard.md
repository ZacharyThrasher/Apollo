# Keyboard shortcuts

The list is here, not in the app (Settings shows only the Global hotkeys switch).

## In the window

These work wherever focus is, **except** while a text field (Search) or a popup (the track menu,
the playlist picker) has it, so typing "m" in Search types an "m". Where a more specific part of
the screen binds the same key it wins: a focused slider steps with Left/Right, the queue list and
Search results use Up/Down and L on their own cursor row. The kind chips under the field are one Tab stop (Left/Right walk them, Enter picks); the Top result card, its Play button and each "Show all" are Tab stops too (Enter presses them).

| Key | Does |
|---|---|
| Space | Play / pause |
| M | Mute / unmute (the slider keeps its level) |
| Up / Down | Volume +5 / -5 (Up unmutes; Down while muted does nothing) |
| Left / Right | Seek -5 s / +5 s |
| Shift+Left / Shift+Right | Previous / next track |
| Ctrl+Left / Ctrl+Right | Previous / next track (older binding, kept) |
| Ctrl+F or Ctrl+K | Open Search with the caret in the field (these work in text fields too) |
| Ctrl+L | Show / hide lyrics (opens Now Playing) |
| L | Like / unlike the playing song |
| C | Set the playing song aside in the Crate (in the queue, Library and Search lists: the song under the cursor) |
| Ctrl+Z | Undo the last queue change |
| Ctrl+Shift+Z or Ctrl+Y | Redo it |
| Esc | Close Now Playing |
| F11 | Fullscreen while Now Playing is open (leaves fullscreen when Now Playing closes) |

### While skimming

Skim (the **Skim** button beside Play and Shuffle on playlist, album and Songs headers, and on
Explore's Top songs) plays each song from about a third in for a few seconds. These keys exist
only then, and a text field or popup still keeps its own keys:

| Key | Does |
|---|---|
| K | Set the song aside in the Crate; the skim carries on |
| Enter | Stop skimming and play this song from its start; the rest of the list plays after it (Ctrl+Z goes back to what was queued before) |
| Esc | Stop skimming and put back what was playing before (nothing, if nothing was) |
| Shift+Right / Shift+Left | Next / previous song's window |

Ctrl+Z does not touch a skim. The list is played in order with shuffle and repeat off for as long
as it lasts, and it is not saved: a restart brings back what was queued before. Skimmed songs are
never added to YouTube Music history or the local stats. Settings > Playback > Skim (on) and
Seconds per song (10, 20 or 30; default 20).

Undo covers what changes the queue: playing a new list (replacing it), Clear, Remove, reorder,
Play next / Add to queue, Start radio (with the radio that follows it, as one step), and the
Shuffle switch. Fifty steps are kept for the session. It never interrupts the song that is
playing; only when the undone action had changed that song (a new list replaced the queue) does
the old song come back, paused where it was. A small "Queue restored" pill shows for 2 seconds.

Media keys are the OS's (SMTC) and are untouched.

## Global hotkeys

Settings > Keyboard > **Global hotkeys** (off by default). While on, these work when Apollo is
hidden in the tray or something else has focus:

| Key | Does |
|---|---|
| Ctrl+Alt+Space | Play / pause |
| Ctrl+Alt+Right / Left | Next / previous |
| Ctrl+Alt+Up / Down | Volume +5 / -5 |
| Ctrl+Alt+H | Show / hide the mini player HUD (opens the mini player if it is closed) |

A combination another program already owns is skipped quietly (one line on stderr, e.g.
`hotkeys: could not register VolumeUp: ...`); the others still work. Windows does not repeat a
registered hotkey while the keys are held, so each press is one step.

## Where it lives

- `crates/app/src/player/skim.rs`: Skim itself (the window, the ticker, Enter and Esc).
- `crates/app/src/keymap.rs`: the in-app bindings (one key context predicate decides where they
  apply) and what each global hotkey does.
- `crates/app/src/hotkeys_win.rs`: registers the global hotkeys with Windows (`global-hotkey` on
  GPUI's own message loop, like the tray) while the switch is on.
- `crates/app/src/ui/shell.rs`: the action handlers. `tests/keyboard.rs` drives them.
