# Keyboard shortcuts

The list is here, and in the app: press **?** (anywhere except a text field) for the cheat sheet,
Settings > Keyboard for the switches. See "Learning the keys" below.

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
| ? (Shift+/) | Cheat sheet: every key, grouped by Play / Collect / Navigate / Window. Esc, ?, Enter or Space close it; the other keys stand down while it is open |
| Ctrl+L | Show / hide lyrics (opens Now Playing) |
| L | Like / unlike the playing song |
| C | Set the playing song aside in the Crate (in the queue, Library and Search lists: the song under the cursor) |
| P | Add to playlist: open the picker for the playing song (in the queue, Library, Search and Crate lists: the song under the cursor). Settings > Playback > "P opens the playlist picker" switches it off |
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

## The playlist picker

P (or the add-to-playlist button) opens it with the caret in a filter box. Recently used
playlists are on top, a divider, then the rest in the order YouTube Music lists them.

| Key | Does |
|---|---|
| Type | Narrow the list (every word must be in the name) |
| Up / Down | Move the cursor |
| Space | Mark / unmark the playlist under the cursor. Only while the filter is empty: with text in it a space is a space |
| Ctrl+Space | Mark / unmark, any time |
| Enter | Add to every marked playlist, or to the one under the cursor if none is marked; on the "Create" row, make that playlist with the song in it. Closes (unless Settings > Playback > "Keep picker open after adding" is on) |
| Ctrl+Enter | Make a playlist of the typed name, whatever row the cursor is on |
| Esc | Close (even with text in the filter) |

A playlist that already holds the song shows its check and cannot be marked. Marks are add-only;
taking a song out is done on the playlist's own page. A click on a row adds to it at once.

## Learning the keys

The help reads the live keymap (never a copy of it), so a key bound anywhere shows up in all of
these without further work:

- **Tooltips end with their key**: "Add to Crate  C", "Mute  M", "Next  Shift+→". A button in the
  player bar or Now Playing shows only keys that work wherever focus is; a row's actions also
  show their list's key (L, C, P in Search, Library, Queue and the Crate).
- **Context-menu rows show the key** at their right edge (11px caption, text.secondary): Like,
  Add to Crate, Add to playlist, Remove.
- **`?` cheat sheet** (`ui/cheat_sheet.rs`): bg.overlay panel, key caps as bg.raised pills. When
  Global hotkeys are on, the Ctrl+Alt keys are listed under Window.
- **Hold Ctrl for about 0.6 s** and the buttons that have a single-key shortcut (Play, Mute, Like,
  Add to playlist) wear a small badge with the key, like Office key tips; letting go, switching
  windows or pressing any other key puts them away.
- **A nudge, once per command**: the third time a command that has a key is done with the mouse
  (a button, a row action or a menu row; keyboard use never counts), a one-line "Tip: press C"
  shows in the pill above the player bar for 4 s. Each command's tip shows once ever (the counts
  are kept in the settings table under `shortcut_hint_state`).

Settings > Keyboard: **Show shortcut hints** (on) covers the tooltips, the menu keys and the
nudge; **Hold Ctrl for key badges** (on) covers the badges. The cheat sheet is always there.
The mini player and the HUD show none of this (their keys are a subset).

### Adding a key

Bind it as usual. A plain letter whose action no entry names appears in the cheat sheet's
"More" group, named after its action. For a proper label, group, tooltip and menu key, add one
line to `ENTRIES` in `crates/app/src/shortcuts.rs`: an id, a label, a group, the short action
names (one per page that binds the key) and the tooltip labels that mean the command. A menu
row gets its key from `shortcut_of` in `ui/track_menu.rs`.

### Why the letters are what they are

One letter, one meaning, the same in every list: **L**ike, **C**rate, **P**laylist, **M**ute; the
planned **K**eep (Skim) and **A**dd (target playlist) follow the same rule. Chords for what
reaches further: Ctrl+K / Ctrl+F search, Ctrl+L lyrics, Ctrl+Z / Ctrl+Y queue undo and redo.
Space, the arrows, Esc and F11 are the ones every player has.

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
- `crates/app/src/shortcuts.rs`: reads the keymap for the tooltips, menu keys, cheat sheet rows,
  badges and the third-use nudge (`HintLedger`).
- `crates/app/src/ui/shell.rs`: the action handlers. `tests/keyboard.rs` drives them.
