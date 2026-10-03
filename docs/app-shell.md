# App shell (M3, slice 1)

`crates/app` is the `apollo` binary: one GPUI window with a custom Mica title bar, a nav rail
(Home, Search, Library; each routes to an empty page with only its title) and the player bar,
driven by the existing playback coordinator through the runtime `Driver`.

Everything visual comes from `crates/theme` (`docs/design/DIRECTION.md` is binding). Views never
hold a color, size, duration or type size of their own; `src/style.rs` is the only place theme
tokens meet GPUI types.

## Running

```text
cargo run -p apollo                       # simulated backend: no account, tools or network
cargo run -p apollo --features real-services -- --real   # Windows, from the repo root
```

`--real` options: `--source auto|stored|firefox|login|file:<path>` (default `auto`: stored
session, else the default Firefox profile), `--tracks N` (Liked Music queue length, default 25),
`--silent` (mpv null output, for screenshot runs), `--tools <dir>` (default `./tools`),
`--select-identity K` (only needed when the login can act as several channels). The queue loads
paused; nothing plays until asked.

The simulated backend has six fixture tracks covering long titles, grayscale art (accent
fallback), no art, and a 25 s track to watch the end-of-track advance.

## Keyboard

| Key | Action |
|---|---|
| Space | play / pause (anywhere in the window) |
| Ctrl+Right / Ctrl+Left | next / previous |
| Tab / Shift+Tab | move between controls (accent focus ring, keyboard focus only) |
| Enter | activate the focused control |
| Left / Right on a slider | seek 5 s / volume 5% |
| Media keys (play/pause, next, previous) | the same as the buttons, even with Apollo in the background (`docs/os-media.md`) |
| Ctrl+K or Ctrl+F | open Search and focus its field |
| Down / Up in the search field | move the selection through results |
| Enter in the search field | search now, or play the selected song |
| Ctrl+Enter in the search field | add the selected song to the queue |
| Escape in the search field | clear the field, then leave it |

Space is bound at the root (`Shell` key context); the text field claims it in its own context
(`InsertSpace`), so typing a space never toggles playback.

## Layout of the code

| Module | Role |
|---|---|
| `backend/` | `Prepared` = ports + starting queue + artwork, search and thumbnail sources. `sim` and `real` implement it. |
| `settings.rs` | Saved settings (volume) in the SQLite store under `%LOCALAPPDATA%\Apollo\app`. |
| `player.rs` | `PlayerModel`: owns the `Driver`, turns adapter completions into coordinator events, keeps volume (a user setting sent straight to the engine port; default 70, saved through `settings.rs`), exposes a `PlayerView` snapshot. |
| `art.rs`, `accent_anim.rs` | artwork decode / square crop, `accent_from_artwork`, 400 ms OKLCH crossfade |
| `search.rs` | `SearchModel`: normalise, 400 ms debounce, 2-character minimum, newest query wins, answered queries are not re-asked. `SearchSource` is the port. |
| `thumbs.rs` | `ThumbStore`: bounded, deduplicated list artwork by URL. |
| `ui/` | `shell`, `titlebar`, `nav`, `player_bar`, `button`, `slider`, `tooltip`, `search_page`, `text_field`, `text_button` |

## Tests

`cargo test -p apollo-app` runs unit tests (formatting, slider math, art/accent, the simulated
engine against the real coordinator) and headless GPUI tests (`tests/ui.rs`): the real views on
GPUI's test platform with the deterministic mock adapters, driving Space and Ctrl+Left/Right.
They do not cover pixels; screenshots on Windows do.

## Not in this slice

Tray. (SMTC and the media keys are wired: `docs/os-media.md`. Accessibility:
`docs/accessibility.md`.) The account is re-validated by a background thread (about every 30
minutes) in real mode; re-validating immediately after a
`SessionExpired` playback failure is not wired yet.

## Search

Search runs through the signed-in account: anonymous InnerTube search is not usable, and the
service records signed-in searches in the account's history, hence the debounce and minimum
length. Songs play now (inserted next, then advance, so the queue is kept) or are added to the
queue; albums, artists and playlists are shown but do nothing until their pages exist.

## Library page (M3, slice 2)

`crates/app/src/ui/library.rs` (view) over `crates/app/src/library/` (model, `LibrarySource`
port, simulated source, InnerTube-backed `live` source behind the `provider` feature, which
`real-services` turns on).

- **Lists:** Playlists (Liked Music always first) and Albums as tabs; opening one shows its
  tracks. Both lists are `uniform_list`s, so only the rows in view exist (tested with a
  3,000-track simulated playlist). Covers load lazily for visible rows only (4 at a time, 400
  cached).
- **Playing:** Play (shuffle off), Shuffle (shuffle on, random first track), or a row (keeps the
  player's shuffle setting) replace the queue through `Event::PlayContext`. A play request made
  while pages are still loading waits for the last page, so the queue is never a truncated first
  page; the button shows a spinner meanwhile. A page failure part-way shows "Couldn't load all
  tracks" with Retry and drops the pending request.
- **Keyboard:** Up/Down/PageUp/PageDown/Home/End move one selection per list, Enter opens or
  plays, Escape or Alt+Left goes back. Choosing Library in the nav while on it returns to the
  lists.
- **Real data:** `InnerTubeLibrary` builds a signed-in InnerTube session from the account
  service's cookie snapshot and returns rotated cookies through `offer_rotation`. Not yet verified
  against a live account: the selected channel's `page_id` is used as the data-sync id, and
  `FEmusic_liked_playlists` / `FEmusic_liked_albums` card shapes come from the vendored parser.
- **Not here:** library caching in the store, write actions (like, add to playlist), artist pages,
  a scrollbar, the queue panel.
