# Plays nicely with the PC

Three things: Apollo keeps its place when the sound output goes away, a sleep timer fades the
music out and pauses, and (optionally) it pauses when another app starts playing sound.

## Output lost (TEST-20)

Triggers (all end in `Event::OutputLost`, `crates/domain/src/playback.rs`):

- the default playback device's id changed or there is none now (headphones unplugged, Bluetooth
  dropped, a dock removed, a new device became the default);
- the PC suspends or resumes (`PowerRegisterSuspendResumeNotification`), and as a backstop the
  wall clock jumped more than ~4 s between two 250 ms looks;
- libmpv itself reports a lost device (`Failure::Device`, as before).

What happens: the player **pauses where it is**. The queue, the position and the track stay, and
nothing is skipped. No error toast for the first three (the third keeps its old one). The
coordinator remembers `reopen_on_play`: the next **Play** drops the old stream and opens the
current track again at the paused position (a fresh resolve, usually served from the URL cache),
so mpv opens its audio output on whatever device exists then. Pausing and playing again after that
is an ordinary resume. A load that is still in flight just opens on the current output.

Why reopen instead of resume: after an unplug or a wake mpv's output may be dead, or may have
quietly moved to the speakers. Reopening is the one path that cannot resume into silence or into
the wrong device.

## Sleep timer

`crates/app/src/sleep_timer.rs` (pure) and `PlayerModel::{set_sleep, sleep_tick}`.

- **Where:** a moon button in Now Playing's title-line actions opens a popover (the context menu
  look) with Off, 15, 30, 45, 60 min, End of track; the moon fills and a small "23m" label hangs
  off its corner while a countdown runs. The tray right-click menu has a **Sleep timer** submenu
  with the same entries (check mark on the active one).
- **Minutes:** counts down on the wall clock. At zero the volume falls to silence over 5 s, then
  pauses. If nothing is playing at zero the timer just ends.
- **End of track:** the fade covers the last 5 s of the track and the pause lands 250 ms before
  the end, so the next track (gapless or not) never starts. It follows the playhead: seeking back
  restores the level; skipping carries the timer on to the next track.
- The fade is a factor on the engine's volume (`effective_volume = volume * fade`). The volume
  slider and the saved volume never change. After a fade ends in a pause the engine volume is held
  down until music is wanted again, then restored **before** the stream resumes.
- Pausing during a fade restores the volume (a countdown timer ends; End of track stays armed).
- Not remembered between runs.

## Pause when other audio plays

Settings > Playback > "Pause when other audio plays" (`pause_on_other_audio`, off by default).

`pc_watch_win.rs` polls Core Audio's session list for the default playback device every 250 ms:
other processes (not Apollo, not the system-sounds session) whose session is active **and** whose
peak level is above silence count as playing. `pc_watch.rs` (pure) turns that into events:

- a process must keep playing for **2 s** before it counts, so notification dings and error
  sounds never pause the music; gaps of silence under 3 s do not end a stretch;
- each stretch of sound reports **once**, so pressing Play while the movie is still going is not
  fought, and nothing resumes on its own;
- only a playing Apollo pauses; the pause is a plain pause (Play just resumes).

Known behaviour: a Remote Desktop session (mstsc) playing remote audio is another process playing
sound, so it counts like any app. There is no ignore list yet.

## Tests

- `crates/domain`: output-lost cases (pause in place, reopen once, queue kept, in-flight load,
  seek, engine device failure), plus output-lost in the fuzz mix.
- `crates/app/src/sleep_timer.rs`, `pc_watch.rs`: pure unit tests with an injected clock.
- `crates/app/tests/pc_friendly.rs`: the player model against recording ports (engine call order:
  silent before pause, volume back before resume).
- `crates/app/tests/ui.rs`: the Settings row, the moon, the popover (size, rows, keyboard).
- Windows-only code (`pc_watch_win.rs`, the tray submenu) is cross-checked with
  `cargo check --target x86_64-pc-windows-gnu -p apollo-app --tests` and verified by hand using the
  steps in `windows-check-todo.md`.
