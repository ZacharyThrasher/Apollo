# Gapless playback

Settings > Playback > **Gapless playback** (on by default, remembered). Off means the next track
opens when the current one ends, as before.

## How it works

1. The coordinator already resolves the next track while the current one plays (the preload). When
   that lease is ready it emits `Effect::Preload { load, lease }`; the driver passes it to
   `MediaEngine::preload`.
2. `MpvEngine::preload` appends the stream to libmpv's playlist (`loadfile <url> append`).
   `gapless-audio=yes` keeps the output running across the file change and `prefetch-playlist`
   makes mpv open the next stream in the background before the current one ends.
3. When the current file ends with a track lined up, the engine does **not** report `Eof`. It
   makes the lined-up load the active one, applies that track's loudness gain at once
   (`track_changed`, the same call a normal open makes), and when mpv reports the new file loaded
   it sends `Transitioned` and then `Opened` (with the duration) for the new load. The
   coordinator adopts the preload, moves the queue on, and starts resolving the track after it.
4. Anything the engine cannot or should not line up is simply declined, and the track opens at the
   end of the previous one exactly as without gapless: gapless off, nothing playing yet, a stale
   lease, or a stream that needs different request headers (mpv has one global header list;
   changing it would change the stream already playing).

## Keeping the queue honest

- A queue edit that changes what plays next (play next, remove, reorder, shuffle, repeat-one)
  already cancels the coordinator's preload; the `Cancel` reaches `MpvEngine::close`, which takes
  the lined-up track out of the playlist (`playlist-clear` keeps the playing file) and a new
  preload is resolved and lined up.
- Skip, jump, seek, pause, autoplay radio and queue restore are untouched: a user navigation opens
  with `loadfile replace`, which empties the playlist.
- If mpv has already moved on by itself when the coordinator drops or never hears about that
  track (a queue edit at the very moment of the boundary, or the stream fails to open), the engine
  reports the *previous* load ended (`Eof`) so the coordinator carries on from the queue; a
  failed track is then tried again from scratch.
- Turning the switch off takes back what was lined up. Turning it on lines up a next track that
  was already resolved (`Driver::set_gapless`).

## Not verified without the Windows app

`crates/media/tests/gapless.rs` (real libmpv, `ao=null`) covers the event sequence, the gain at the
boundary, gapless off, taking a track back, the header decline and a skip. What it cannot show is
the silence itself: whether `prefetch-playlist` on the shipped mpv 0.41 removes the audible gap on
real YouTube streams. That is the by-ear check in `windows-check-todo.md`.
