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

Sign-in/account UI, library pages, album/artist/playlist pages (their search cards are inert), queue panel (the queue button only toggles its own
state), Now Playing, tray, SMTC wiring beyond what exists. The account is re-validated by a
background thread (about every 30 minutes) in real mode; re-validating immediately after a
`SessionExpired` playback failure is not wired yet.

## Search

Search runs through the signed-in account: anonymous InnerTube search is not usable, and the
service records signed-in searches in the account's history, hence the debounce and minimum
length. Songs play now (inserted next, then advance, so the queue is kept) or are added to the
queue; albums, artists and playlists are shown but do nothing until their pages exist.
