# Accessibility (M3)

**TL;DR:**

- GPUI at our pinned revision **does** expose every control to UI Automation, with names and roles
  (AccessKit → UIA). The M0 note "only the top-level window is visible" is out of date: it was
  written before the app gave its controls roles.
- This slice audited the live tree, fixed the gaps it found, and added scripts to re-check it.
- Not run: a real screen reader (Narrator / NVDA). Everything below is what UI Automation reports,
  which is what they read.
- Nothing visual changed. No taste calls were needed (see "Decisions for the coordinator").

## How it works in GPUI

| Fact | Consequence |
|---|---|
| The tree is built **lazily**, on the first UI Automation request | A quiet app shows only the window until something asks. `scripts/uia-dump.ps1` is a client, so it switches the tree on. |
| A node needs an element `id` **and** a `role` | Plain `div`s and text are not in the tree. A control without both is invisible. |
| `on_click` registers an accessible Click | Click becomes UIA **Invoke**. `aria_selected` turns a node into a *selection item* instead, which removed Invoke (see nav fix below). |
| GPUI's **test platform has no accessibility adapter** | `Window::debug_a11y_tree_json` is `None` in headless tests, so the tree cannot be asserted in `cargo test`. The live check is a script (below). |

## Check it yourself

Windows PowerShell 5.1, with Apollo running (`cargo run -p apollo-app`):

```powershell
powershell.exe -NoProfile -File scripts\uia-dump.ps1 -Check            # dump the tree; exit 1 on an unnamed control
powershell.exe -NoProfile -File scripts\uia-dump.ps1 -Invoke Library   # drive it through UIA, then dump
powershell.exe -NoProfile -File scripts\window-shot.ps1 -Keys Tab,Tab -Out ring.png   # real Tab presses, window-only PNG
```

`-Check` fails if a keyboard-focusable control has no name or only a generic role, or if
anything actionable has no name. `-Invoke` uses Invoke / SelectionItem / Toggle patterns, so it
works with another window in front and sends no input to it. `window-shot.ps1` posts keys to
Apollo's window handle and captures that window alone (never the desktop).

Latest result, simulated backend, every page and panel reachable without a keyboard:

| State | Nodes | Focusable | Problems |
|---|---|---|---|
| Home | 20 | 17 | 0 |
| Search | 22 | 18 | 0 |
| Library: Playlists, Albums, an open playlist | 32 / 28 / 36 | 19 / 19 / 20 | 0 |
| Settings (signed in) | 21 | 17 | 0 |
| Now Playing open | 50 | 29 | 0 |
| Queue panel open | 53 | 23 | 0 |
| Real account: Liked Music open, then playing (`--real --silent`) | 40 | 20 | 0 |

## What UI Automation sees

Read from the live tree where a state could be reached through UI Automation; the cards and the
playlist picker (they need a typed query or a hover) are from the code's roles and labels.

| Control | Role / pattern | Name |
|---|---|---|
| Caption buttons | Button / Invoke | Minimize, Maximize or Restore, Close |
| Nav toggle, nav items | Button, Link / Invoke | Expand, Home, Search, Library, Settings (description "Current page" on the active one) |
| Player bar | Buttons (Invoke or Toggle), Sliders (Range + Value) | Now playing, Shuffle, Previous, Play, Next, Repeat off/all/one, Seek, Like, Mute, Volume, Queue |
| Seek, Volume sliders | Slider / RangeValue + Value | value text "1:23 of 3:12", "70%"; Increment, Decrement and SetValue actions |
| Search field | Edit / Value | Search (holds the typed text) |
| Search results | ListBox → options / Selection | "Title, Artist"; the option itself plays |
| Library | Tab list → tabs, List → items | Library; Playlists / Albums; list named after the tab or "Tracks" |
| Cards (albums, artists) | Button / Invoke | title |
| Queue | List → items, Buttons | Queue; track title; Like, Remove, Shuffle, Repeat, Clear up next |
| Playlist picker | Dialog → ListBox → options | Add to playlist; playlist titles |
| Toast | Alert | its text |

## What changed in this slice

| Gap found | Fix |
|---|---|
| Nav links had no Invoke (voice control / UIA could not activate them) | `aria_selected` replaced by a "Current page" description. |
| Caption buttons absent from the tree (Windows draws no system caption here) | Named buttons with an accessible Click that minimizes, maximizes/restores or closes. Mouse clicks still go through Windows' own hit-testing, so nothing fires twice. |
| Search field absent | `TextInput` role, name from its placeholder, value = typed text. |
| Library list, artist page and queue list unnamed or unrolled | Named; library tabs sit in a tab list. |
| Search results played only on a click of their art or text | The row answers accessible Click. |
| Sliders had a number between 0 and 1 and no actions | Value text, Increment / Decrement / SetValue. |
| Two controls called "Close" (window, Now Playing) | Now Playing's is "Close Now Playing" to assistive technology; its tooltip is unchanged. |
| The heart's name flipped Like ↔ Unlike *and* it reported on/off | Name stays "Like"; the toggle state carries on/off. Tooltip unchanged. |

## Keyboard

Tab order is one flat sort by tab index (ties go to paint order). Pages that never show together
share indices, which is why the numbers repeat.

| Index | Who |
|---|---|
| 1–5 | Nav toggle, Home, Search, Library, Settings |
| 5–7 | Search page: field + clear (5), results + add action (6), cards (7). *Ties with Settings (5): nav paints first, so it wins.* |
| 6–10 | Library (back, tabs 6–7, play, shuffle, list 9, retry/cards 10) and Settings (6) |
| 6–12 | Now Playing (close 6, then like, playlist and transport from 7). It replaces the page, so it shares 6–10 |
| 13–17 | Queue panel: shuffle, repeat, clear, list, search |
| 19–29 | Player bar, left to right: Now playing, Shuffle, Previous, Play, Next, Repeat, Seek, Like, Mute, Volume, Queue |

Seek is a stop only while the track has a length; Retry takes its place on an error.

`crates/app/tests/tab_order.rs` walks Tab on Home, Search, Library, Settings, with the queue open
and with Now Playing open, and asserts: nav first, player bar last and in order, only page stops
between, no trap (the walk returns to its start), and Shift+Tab is the same walk reversed.
**No new tab stops were added**, so no existing index moved.

Reached by keyboard but not by Tab: Space (play/pause), Ctrl+←/→, Ctrl+K / Ctrl+F, `l` (like) and
`p` (add to playlist) on a selected row, Esc / Alt+← back, Alt+↑/↓ to reorder the queue
(`docs/app-shell.md` has the full list).

Not reachable by keyboard: the caption buttons (like any window: Alt+Space for the system menu,
Win+arrows) and the per-row "Add to queue" button in Search (Ctrl+Enter does it).

## Focus rings

Every tab stop draws the accent ring when focus came from the keyboard (`keyboard_focused` in
`ui/button.rs`, 2 px, offset): icon buttons, text buttons, nav items, tabs, cards, sliders (thumb
ring), the search field, the player bar's art block, Retry, the avatar button in Settings.
Lists (Library, Search results, Queue) show it as the selected-row marker plus a ring on the
cursor row. Mouse-only focus never shows one.

Checked by posting real Tab key presses to the window and capturing it: Now playing block (6th
Tab), then Next (10th) both show the ring.

## Reduced motion

Windows "Animation effects" off → `cx.set_reduce_motion(true)`. Every animation site is covered:

| Site | How it follows |
|---|---|
| Hover fades (buttons, rows, cards) | `HoverFade::set(.., animations)`: instant when off |
| Nav rail width, queue panel slide | GPUI `with_animation` / `PanelSlide`: single static frame / instant |
| Accent crossfade | `AccentAnim::retarget(.., animate)` |
| Skeleton pulse | `skeleton::pulse(.., animate)`: static |
| Button spinners | GPUI `with_animation`: static frame |
| Toast fade | `animate` flag: appears and goes at once |

**New:** the setting used to be read once at start. `app::follow_reduced_motion` re-reads it every
2 s, so turning "Animation effects" on or off in Windows takes effect without a restart
(`tests/reduced_motion.rs`).

## Known limits

- **No screen-reader pass.** UIA output was checked, Narrator / NVDA were not run.
- **The tree is not asserted in CI** (GPUI's test platform has no adapter). `uia-dump.ps1 -Check`
  is the gate; run it after UI changes.
- **Text input.** The search field reports its value, but not the caret or selection, so a screen
  reader will not echo characters or cursor moves. That needs synthetic text-run children
  (`a11y_synthetic_children`), a real piece of work.
- **Row actions mount on hover or selection.** The Like / Add-to-playlist buttons inside list rows
  exist only for the hovered or selected row, so UIA lists them only there. Keys `l` / `p` do the
  same for the selected row.
- **No headings or landmarks.** Page titles are plain text; they have no heading role.
- **High contrast / forced colours** is not handled.

## Decisions for the coordinator

None needed. Everything above is accessible names, roles, actions and timing; no colour, size,
copy or layout was touched. Two wording choices were made for assistive technology only:
"Current page" (nav description) and "Close Now Playing". Say if you want different words.
