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
| Ctrl+Up / Ctrl+Down | Volume +5 / -5 (Up unmutes; Down while muted does nothing) |
| Ctrl+Left / Ctrl+Right | Seek -5 s / +5 s |
| Shift+Left / Shift+Right | Previous / next track |
| Ctrl+F or Ctrl+K | Open Search with the caret in the field (these work in text fields too) |
| ? (Shift+/) | Cheat sheet: every key, grouped by Play / Collect / Navigate / Window. Esc, ?, Enter or Space close it; the other keys stand down while it is open |
| Ctrl+L | Show / hide lyrics (opens Now Playing) |
| L | Like / unlike the playing song |
| C | Set the playing song aside in the Crate (in the queue, Library and Search lists: the song under the cursor) |
| P | Add to playlist: open the picker for the playing song (in the queue, Library, Search and Crate lists: the song under the cursor). Settings > Playback > "P opens the playlist picker" switches it off |
| A | Add to the target playlist: the playing song (in the queue, Library, Search and Crate lists: the song under the cursor; with 2 or more picked, all of them). With no target set the picker opens to choose one. Settings > Playlists > "A adds to the target playlist" switches it off. See "The target playlist" |
| Ctrl+Shift+C | Show / hide the docked Crate on the right of the window (the Crate button in the player bar does the same). See "The docked Crate" |
| Ctrl+Z | Undo the last queue change, playlist edit or Crate change, whichever came last (see "Undo") |
| Ctrl+Shift+Z or Ctrl+Y | Redo a queue change (playlist edits have no redo) |
| Alt+Left / Alt+Right | Back / Forward through the places you have been (see "Back and Forward"). Work in text fields too |
| Backspace | Back, except while a text field (Search, a filter, a rename) has the keys, and in the queue and the Crate, where it removes the row |
| Esc | Close Now Playing |
| F11 | Fullscreen while Now Playing is open (leaves fullscreen when Now Playing closes) |

## Moving around

**The plain arrow keys only navigate. They never change song, seek or touch the volume**
(playback is on Shift, seek and volume on Ctrl: see the table above). Tab and Shift+Tab walk
every control in reading order: the nav rail, the page, the queue, the player bar.

| Where focus is | Up / Down | Left / Right |
|---|---|---|
| Nothing (a click on empty space, the window just opened, Esc out of Search) | The first press puts the cursor on the page's main list: the Library / Crate / queue list, the first Search result, the first Home or Explore shelf item; with no list on the page, the open queue or docked Crate. Any arrow does it | same |
| A list (Library, Search results, the Crate, the queue, Explore's Top songs) | Move the cursor (the keys below) | Left goes to the nav rail's button for the page (not in the docked Crate on the right, where it is the previous control). Right does nothing |
| A Home shelf of cards | Up / Down: the shelf above / below (its remembered card) | Previous / next card. Left on the first card goes to the nav rail |
| A Home grid of songs (columns of four) | Up / Down: the song above / below in its column; at the end of a column, the shelf above / below | Left / Right: the same song row in the previous / next column. Left on the first column goes to the nav rail |
| An Explore shelf or chip group | Next / previous control (Explore's Top songs list moves its cursor) | Previous / next item |
| The nav rail | Up / Down walk the destinations (Enter opens one) | Right goes into the page; Left stays |
| A slider (progress, volume) | its own steps (the volume slider Up / Down, the progress slider Left / Right) | its own steps |
| Any other control (a button, a switch, a chip) | Previous / next control, in Tab order (Up / Left back, Down / Right forward) | same |
| A text field, the track menu, the playlist picker, a dialog | their own (caret, rows, fields) | their own |

On a page with no list (Settings) the first press lands on its first control.

## The docked Crate

The Crate button in the player bar (or **Ctrl+Shift+C**) opens the Crate as a 320px panel on the
right of the main window, in the queue panel's place: the two share that slot, one at a time, and
the **Queue · Crate** tabs at the top of either swap them (opening the queue closes the Crate and
the other way round). It stays open while you move between pages, and open or closed is
remembered for the next run. It steps aside for Now Playing, which fills the window, and for the
Crate page itself (the same list); below 1100px of window width it overlays the page's right edge
instead of squeezing it.

The list is the Crate page's, in the same order and with the same keys (a click on a row or Tab
puts the cursor in it): Up/Down, Enter plays from there, L, P, A, Delete, Alt+Up/Down and
multi-select all work, and More opens the usual song menu. The header has **Save as playlist…**
and **Add all to…**.

**Dragging songs in:** pick up a song (or, from a picked row, every picked song) in Search, a
playlist, an album or the Songs tab and let go on the panel. A thin line shows where it lands;
below the last row, or on the empty panel, is the bottom / the start. A song the Crate already
holds moves to that place instead of being added twice. C still adds at the bottom. Rows of the
panel drag to rearrange it, as on the page.

## Back and Forward

Like a browser's. The window remembers the places it showed (Home, Explore, Search, a Library tab,
a playlist, album or artist page, the Crate, Settings, Now Playing), up to 50, in memory only.

- **Buttons:** the chevrons at the left of the title bar (grey when there is nowhere to go), the
  keys above, and the mouse's side buttons (Back / Forward).
- **Going back restores the page:** how far it was scrolled, what the Search field held and what a
  playlist's or the Songs tab's filter box held. The page cache makes the page itself instant.
- **A new place from the middle** drops the places that were ahead (the browser rule). Choosing
  the nav item of the page you are already on adds nothing.
- **Now Playing is a place:** Esc, the chevron and Back all close it by stepping back to the page
  it was opened over; Forward reopens it.
- **A mood page in Explore** closes first (Back from it shows the tiles), then Back leaves Explore.
- **A popup or dialog open:** the keys and side buttons leave the screen alone.
- A page's own Back button and Esc on a page do the same as Back.

## Picking several songs

On Search results, playlist and album pages, the Library Songs tab, the Crate and the queue. The
cursor row keeps its own ring; the picked rows are tinted with the accent.

| Key | Does |
|---|---|
| Ctrl+click | Pick or un-pick a song (it also moves the cursor there) |
| Shift+click | Pick everything from the last picked song to this one |
| Shift+Up / Shift+Down | Move the cursor and stretch the picks to it (going back shrinks them) |
| Ctrl+A | Pick every song in the list |
| Esc | Clear the picks (the next Esc does what it did before: leave the list, close the page) |
| Up / Down | Move the cursor and clear the picks |
| L / C / P / A / Ctrl+Enter | With 2 or more picked: like all (or un-like when all are liked) / Crate all / Add all to a playlist (the picker opens at the cursor row; in the Crate and the queue it opens in the middle) / Add all to the target playlist / Add all to the queue (Search). C is not bound in the Crate |
| Delete / Backspace | With 2 or more picked, in the Crate or the queue: take them all out (the playing row stays in the queue) |

With 2 or more picked, right-click on any picked row (or its More button) opens a menu for all
of them: Play next, Add to queue, Add to playlist…, Add to Crate (Remove from Crate when they are
all in it, as on the Crate page), Like, and on a playlist of your own Move to top, Move to bottom and Remove from playlist; in the
queue Remove from queue. A row that is not picked keeps its own menu. A plain click plays the song
as before and clears the picks. Likes and playlist removals go to the account one at a time;
Remove from queue is one step for Ctrl+Z, whatever the count. The Crate still drags one row at a
time (a drag does not take the picks with it). The picks belong to the list: another page, tab
or search starts with none.

## In lists

| Key | Does |
|---|---|
| Up / Down | Move the cursor |
| Enter | Play the row |
| L | Like / unlike the row |
| C | Set the row aside in the Crate (queue, Library, Search) |
| Delete / Backspace | Remove the row (queue, Crate) |
| Alt+Up / Alt+Down | Move the row one place up / down (up-next in the queue; the Crate; a playlist of your own). The row keeps the cursor. With songs picked on a playlist, all of them move one place together. |

The Crate and a playlist of your own also drag: pick a row up and let go where it should go (a thin line shows the place; below the last song sends it to the bottom). New songs land at the bottom.

### Arranging a playlist of your own

On a playlist you own, once every song has loaded (and not while the filter box has text in it,
because a filtered row's place on screen is not its place in the playlist):

| Do | Does |
|---|---|
| Alt+Up / Alt+Down | The cursor row, or every picked song, moves one place; the cursor follows. At the top (or bottom) nothing moves, and with several picked, one pick already there holds them all. |
| Right-click a song (or the picked songs) > **Move to top** / **Move to bottom** | Sends it (or all of them, keeping their order among themselves) to that end. |
| Playlist header More > **Remove duplicates…** | Opens a preview of every song listed again (matched by video id; the first copy of each stays) with the place each repeat has. **Remove N** takes exactly those rows out, **Cancel** (focused, Esc) keeps everything. |

There is no whole-playlist sort. Each move shows at once and goes to YouTube Music as its own
"move before" edit, one at a time through the page's edit queue; a refused one puts the playlist
back as YouTube Music holds it and leaves the usual notice.

### Skim from here

| Key | Does |
|---|---|
| K (in a playlist, album or Songs list, while not skimming) | Skim the list from the song under the pointer, or from the keyboard's row when the pointer is on none |

Settings > Playback > Skim off turns it off. Once a skim runs, clicking any
song of that same list moves the skim to that song and goes on skimming from there; Enter on a
row (or on the song being skimmed, below) leaves skim and plays normally.

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

### Undo covers playlist edits too

Ctrl+Z is one line in time across the queue and your playlists: it takes back whichever of the
two changed last, then the one before, and so on. Playlist edits it covers (twenty are kept for
the session, in memory only; a restart or a different account starts empty):

| Edit | Ctrl+Z does |
|---|---|
| Adding songs to a playlist (P picker, the Create row, A target add, Crate "Add all to…", More like this +, a multi-select add) | Takes out exactly the rows that add made, by the row tokens YouTube Music returned. Another copy of the same song is never touched. |
| Making a playlist with songs (the picker's Create row, Save as playlist from the queue or the Crate) | Deletes the playlist if it still holds only those songs; if you have added others since, only the songs it was made with come out. |
| Removing songs from a playlist (Remove from playlist, a multi-select remove, Remove duplicates) | Puts them back where they were (each in front of the row that followed it), even a repeat of a song the playlist still holds. If the row it sat in front of is gone, it goes at the end and a notice says "Put back at the end instead". |
| Moving songs in a playlist (drag, Alt+Up / Alt+Down, Move to top / bottom) | Puts the rows back in the order they had; rows added since stay after them. |

Several songs removed or moved at once are one step. After an undo the pill reads "Undid: added
3 songs to Gym Mix" for 4 seconds; a failed undo shows the usual error ("Couldn't undo that"). An
add whose reply from YouTube Music was lost cannot be undone exactly (which rows it made is not
known), so Ctrl+Z says so and leaves it. An add that has not been answered yet answers "Try again
in a moment".

### Undo covers the Crate too

The Crate's own changes are steps in the same line of time (thirty are kept for the session, in
memory only; a restart or a different account starts empty):

| Change | Ctrl+Z does |
|---|---|
| A song set aside (C, Add to Crate, a drop on the dock) | Takes it out again |
| Remove, or Delete with several picked | Puts the songs back where they were (several at once are one step) |
| Clear | Puts every song back. Clear no longer asks first: Ctrl+Z is the safety net |
| A move (drag, Alt+Up / Alt+Down, a drop in the dock) | Puts the order back |
| A song filed to a playlist, with "Take songs out of the Crate once filed" on | Puts it back in the Crate (the playlist add is its own step, taken back by the next Ctrl+Z) |

The pill reads "Undid: removed 3 songs from the Crate" for 4 seconds. Ctrl+Z takes the newest of
the queue's, the playlists' and the Crate's steps, so a Crate change made after a queue change is
undone first. There is no redo for the Crate.

### File and forget

Settings > Playlists > "Take songs out of the Crate once filed" (off by default, because a song
can belong in two playlists): once YouTube Music confirms a song is in a playlist, it leaves the
Crate. That covers A, the P picker (including its Create row and several playlists marked at
once: the songs leave when the picker closes), the Crate's "Add all to…" and a picked-songs add.
A failed or lost add takes nothing out. "Save as playlist…" keeps its own "Clear Crate" check.

An add that Ctrl+Z can take back says so in its confirmation: "Added to Road trip · Ctrl+Z to
undo" (queue adds, Crate adds and playlist adds, including a playlist made from the queue or the
Crate), for 2.5 seconds, while **Show shortcut hints** is on. Likes and library saves have no undo
and no hint. Settings > Playlists > **Confirm when adding** switches the confirmation off.

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

## The target playlist

One playlist of your own is the **target**; **A** drops a song into it with no dialog. Build a
playlist while you browse: A, A, A.

- **Choose it** from the picker (press **A** with no target set, or click the target pill), or from
  a playlist of your own: its More menu > **Set as target** (it says **Clear target** on the one
  that is the target now).
- **The pill** under the artist in the player bar shows the target's name. Click it (or Tab to it
  and press Enter) to reopen the picker and change the target. Settings > Playlists > "Show target
  playlist in player bar" hides it; the target and the key stay.
- **The picker, in target mode** says "Target playlist" in its caption. Enter, or a click, on a
  playlist makes it the target (and adds the song you pressed A on, if any); a typed new name
  makes a playlist and targets it; **Clear** (top right, only while there is a target) takes the
  target away. The target's row carries a star, in every picker. There is nothing to mark there,
  so Space does nothing.
- **Adding** is silent. A song the playlist already holds is skipped, never added twice. If the
  service refuses, the add is taken back and the usual "Couldn't add to playlist" shows.
- **Remembered** between runs, per account: another account does not see it. Deleting the target
  playlist clears it; renaming it keeps it under the new name.

## Learning the keys

The help reads the live keymap (never a copy of it), so a key bound anywhere shows up in all of
these without further work:

- **Tooltips end with their key**: "Add to Crate  C", "Mute  M", "Next  Shift+→". A button in the
  player bar or Now Playing shows only keys that work wherever focus is; a row's actions also
  show their list's key (L, C, P, A in Search, Library, Queue and the Crate).
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
**K** (Skim from here, Keep while skimming) and **A**dd (target playlist) follow the same rule. Chords for what
reaches further: Ctrl+K / Ctrl+F search, Ctrl+L lyrics, Ctrl+Z undo (queue and playlists), Ctrl+Y redo of a queue change.
**Plain arrows are for moving around and nothing else**; every playback key on an arrow wears a
modifier: Shift changes song, Ctrl seeks (Left / Right) and sets the volume (Up / Down), Alt
moves a row. Space, Esc and F11 are the ones every player has.

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
