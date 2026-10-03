# Volume normalization

Settings > Playback > **Normalize volume** (on by default, applies live, remembered).

## How it works

1. **Loudness comes from YouTube.** One metadata-only InnerTube `player` request per track
   (`crates/innertube`, `InnerTube::player_loudness`) reads `playerConfig.audioConfig.loudnessDb`:
   dB against YouTube's reference level, `+4` = 4 dB louder. Streams still come from yt-dlp, and
   nothing else in the response is read (no play reporting, no history).
2. **It is fetched beside the resolver, off the UI thread.** `LoudnessResolver` (app) wraps the
   resolver: every `start`, preload and `prefetch` also queues the track on the
   `LoudnessFetcher` (services-common), one worker thread, one track at a time, the track about
   to play ahead of the lookahead. Answers land in a `LoudnessTable` the engine reads, and in the
   SQLite `loudness` table (store, migration 2) so each track is asked about once. "No
   measurement" is remembered for a day; a failed request is not remembered and is not retried
   for a minute.
3. **The policy** (`apollo_domain::loudness::gain_db`) is YouTube's: attenuate a loud track by
   its excess (a `+4` track gets `-4 dB`), never boost a quiet one (`0 dB`), unknown = unity,
   floor `-24 dB`.
4. **libmpv applies it** as `volume-gain`, which is separate from the user's `volume`. The gain
   is a per-track property: `Pump::track_changed` looks it up and sets it at once when a load
   opens (the old track is being replaced, so there is nothing to click against). A gapless
   advance should call the same function at the boundary. A change while a track plays (the
   switch flipped, or its loudness arriving after it started) glides over 400 ms, linear in dB
   (`GainRamp`).

## Not verified without the signed-in Windows app

- The real `player` response for a signed-in WEB_REMIX session (an anonymous request answers
  `UNPLAYABLE` with no `playerConfig`). If the field is missing there, every track plays at
  unity: the failure mode is "no normalization", never silence.
- `volume-gain` on the shipped libmpv (mpv 0.41) with the WASAPI output, and the glide.
  `crates/media/tests/normalize.rs` covers both on Windows.
