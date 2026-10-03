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

Space is bound at the root (`Shell` key context), so a future text field has to claim it in its
own context.

## Layout of the code

| Module | Role |
|---|---|
| `backend/` | `Prepared` = ports + starting queue + artwork source. `sim` and `real` implement it. |
| `player.rs` | `PlayerModel`: owns the `Driver`, turns adapter completions into coordinator events, keeps volume (a user setting sent straight to the engine port), exposes a `PlayerView` snapshot. |
| `art.rs`, `accent_anim.rs` | artwork decode / square crop, `accent_from_artwork`, 400 ms OKLCH crossfade |
| `ui/` | `shell`, `titlebar`, `nav`, `player_bar`, `button`, `slider`, `tooltip` |

## Tests

`cargo test -p apollo-app` runs unit tests (formatting, slider math, art/accent, the simulated
engine against the real coordinator) and headless GPUI tests (`tests/ui.rs`): the real views on
GPUI's test platform with the deterministic mock adapters, driving Space and Ctrl+Left/Right.
They do not cover pixels; screenshots on Windows do.

## Not in this slice

Sign-in/account UI, search, library pages, queue panel (the queue button only toggles its own
state), Now Playing, tray, SMTC wiring beyond what exists. The account is re-validated by a
background thread (about every 30 minutes) in real mode; re-validating immediately after a
`SessionExpired` playback failure is not wired yet.
