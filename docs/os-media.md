# Media keys and the Windows media overlay (M3)

**TL;DR:** the media keys, the volume-flyout media card and the lock-screen controls (all
"System Media Transport Controls", SMTC) drive Apollo through Souvlaki 0.8.3, bound to GPUI's own
window handle. They send the same events as the on-screen buttons.

## Where the code is

| File | Job |
|---|---|
| `crates/app/src/os_media.rs` | Platform-free: `OsCommand`, `MediaState`, and `MediaBridge`, which decides *when* to call the OS. Unit-tested. |
| `crates/app/src/smtc.rs` | Windows only: Souvlaki sink, event mapping, cover files. Installed by `app::run` right after the window opens. |
| `PlayerModel::media_state` / `os_command` | The two seams into the player. `os_command` calls the same `Event`s the buttons do. |
| `crates/app/tests/os_media.rs` | Commands against the real coordinator (mock adapters). |

## What a key or button does

| From the OS | Apollo does |
|---|---|
| Play | `Event::Play`, dropped if already playing |
| Pause, Stop | `Event::Pause`, dropped if already paused |
| Toggle | Pause if playing, else Play |
| Next, Previous | `Event::Next` / `Previous` (Next while paused starts playing, as the in-app buttons do) |
| Scrubber (`SetPosition`) | `Event::Seek`, clamped to the track length |
| Fast-forward, rewind | Seek ±10 s |
| Volume, Open, Raise, Quit | Ignored |

Play and Pause are never mapped to a toggle. Windows turns the play/pause *key* into Play or
Pause from the status the app last reported, so the reported status has to follow the player
exactly; a toggle would make a stale key do the opposite of what was pressed.

## What the overlay shows

Title, artist, album, cover, length, status, position. `MediaBridge` sends as little as it can:

| Change | Call |
|---|---|
| New track, new length (the engine reports the true one), or the cover finishing loading | metadata, then playback |
| Play / pause | playback |
| A seek (position off the OS's own extrapolation by more than 1.5 s) | playback |
| While playing | playback restated every 10 s |
| Nothing changed | nothing |

Details that matter:

- **Cover.** Souvlaki takes a `file://` path or an https URL. The art is already downloaded and
  decoded for the player bar, so it is encoded once off the UI thread (300 px PNG) and written to
  `%LOCALAPPDATA%\Apollo\app\media-art\<video id>.png`. Newest 16 files are kept.
- **No art / stale art.** Souvlaki cannot clear a thumbnail and only writes fields it is given.
  A track with no art gets a 1×1 transparent PNG (`none.png`) so the previous track's picture
  does not linger, and missing album/artist go as empty strings for the same reason.
- **Window closed.** Apollo reports Stopped and the session disappears (checked: after the
  close button, `GlobalSystemMediaTransportControlsSessionManager` lists no Apollo session).
- **Name.** The overlay's app name is the exe name (`apollo.exe`); there is no Start-menu
  shortcut or AUMID registered yet.

## How it was checked

`scripts/smtc-probe.ps1` reads Apollo's SMTC session the way another process (the volume flyout)
does, and sends requests addressed to that session alone. Windows PowerShell 5.1:

```powershell
powershell.exe -NoProfile -File scripts\smtc-probe.ps1 -Action status     # title, artist, album, cover bytes, status, timeline
powershell.exe -NoProfile -File scripts\smtc-probe.ps1 -Action others     # other apps' sessions: id and status only
powershell.exe -NoProfile -File scripts\smtc-probe.ps1 -Action next       # play | pause | toggle | next | previous | seek -Seconds 120
```

| Check | Simulated backend | Real account (Liked Music, `--silent`) |
|---|---|---|
| Title / artist / album | pass | pass (non-empty, change on Next) |
| Cover | pass (cover bytes differ per track) | pass (175 KB, changes with the track) |
| No-cover track clears the previous cover | pass (73-byte blank) | n/a (every liked track had art) |
| Play, Pause, Toggle | pass | pass |
| Next / Previous | pass | pass (Previous within 3 s of a start goes back; after that it restarts, the coordinator's rule) |
| Scrubber seek | pass (position = 100 s) | pass (position = 120 s) |
| Length follows the engine | n/a | pass (231.0 s listed → 230.1 s once opened) |
| Position restated | pass | pass (every ~10 s) |
| Session gone after close | pass | pass |

**Not done: a physical media key.** `scripts/smtc-probe.ps1 -Action key` sends a real
`VK_MEDIA_*` key, but it refuses unless Apollo is the foreground window (a remote-desktop client
in front would forward the key to the other machine). Windows would not let a background process
bring Apollo forward on this PC, so the key → Windows → SMTC leg was not re-run. It was proven in
M0 (`docs/build-baseline.md`, step 4) on the same window handle; everything after that point is
what the probe exercised.

## Known limits

- The overlay position is extrapolated by Windows between restatements; a long buffering stall
  can be off by up to 10 s until the next restatement.
- The Windows volume flyout shows the cover at whatever size it likes; the 300 px PNG is
  square-cropped by `art.rs`, not styled for the overlay.
- Nothing is listened for on other platforms; `smtc.rs` is `cfg(windows)`.
