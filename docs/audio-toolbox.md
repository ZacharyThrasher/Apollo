# Audio toolbox: output device, equalizer, playback speed

Three settings that act on the sound itself. All three are engine features (`MediaEngine` in
`crates/runtime/src/ports.rs`, implemented in `crates/media/src/engine.rs`) with their screens in
Settings > Audio and Now Playing.

## Output device

- Settings > Audio > **Output device** is a select row: the device that is playing and a chevron.
  It opens a popover (`ui/choice_menu.rs`) listing "System default" and the devices, with a check
  on the one in force.
- The saved choice is the mpv device id (`wasapi/{guid}`, `AUDIO_DEVICE` in `settings.rs`);
  empty means the system default (`auto`).
- The engine lists devices from `audio-device-list` (`media/src/device.rs` filters it to WASAPI,
  without duplicates) and re-reads it every 2 s and whenever mpv logs an audio-device error.
- **Unplugged device:** the engine plays on `auto` while the chosen device is missing, and
  moves back when it returns. The choice is never cleared. The row reads "System default"
  meanwhile, because that is what is playing (`audio::resolve_device`).

## Equalizer

- Settings > Audio > **Equalizer** is a switch (off by default) with an Edit button; Edit opens
  the editor under the card (button reads Done while open).
- Ten bands at 31, 62, 125, 250, 500, 1k, 2k, 4k, 8k, 16 kHz, whole dB from -12 to +12
  (`domain/src/audio.rs`). Presets: Flat, Bass boost, Vocal, Treble boost; Reset flattens. A
  slider or preset used while the switch is off turns it on, so the change is heard.
- Saved as `EQUALIZER_ENABLED` and `EQUALIZER_GAINS` (comma-separated dB). A drag changes the
  sound live and writes the settings when the pointer is let go (`audio::save_equalizer`).
- **How it sounds right:** mpv's `af` chain has ten labelled `lavfi=[equalizer=...]` filters
  (`media/src/eq.rs`). The chain is installed once; a band change is an `af-command ... gain`
  sent to that filter, glided in 80 ms steps (`GainRamp`) so a slider makes no click. Flat or off
  removes nothing mid-song: the bands go to 0 dB first.
- **Clip guard:** boosting can exceed full scale, so the engine lowers the volume by the
  loudest boosted band (`Equalizer::preamp_db`). It rides on the same `volume-gain` as the
  per-track normalization gain (`desired_gain_db` = normalization + guard), so the two stack
  and neither fights the other. Gapless: the chain and gain belong to the engine, not the
  track, so the next track keeps both.
- A new filter graph (new sample rate on a track change) is rebuilt on `AUDIO_RECONFIG` with the
  current curve, at most three times in a row.

## Playback speed

- Now Playing, on the title line beside add-to-playlist: a text button showing the speed
  (`1.0×`). It opens a popover with 0.75, 1.0, 1.25 and 1.5.
- mpv `speed` with `audio-pitch-correction=yes` (set in `media/src/config.rs`), so the pitch
  stays. It is **not saved**: every launch starts at 1.0×. The button is in full strength
  while the speed is not normal.

## Tests

- `crates/domain/src/audio.rs`, `crates/media/src/{eq,device}.rs`, `crates/app/src/audio.rs`:
  unit tests for the arithmetic.
- `crates/media/tests/audio.rs` (Windows only, needs the real libmpv): speed, pitch correction,
  flat/off, the guard, normalization stacking, the next track, EQ set while idle, the device
  falling back. On Linux the same file ran against a libmpv 0.37 shim during development.
- `crates/app/tests/audio.rs`: the Settings rows, popovers, editor, live drag and the speed
  button on GPUI's test platform.
- By ear, on the PC: `windows-check-todo.md` ("Audio toolbox").
