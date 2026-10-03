# M2 media adapter — findings and limits (Windows, libmpv)

Crate: `crates/media` (`apollo-media`). Tested with mpv v0.41.0-1092-g3186d369f (shinchiro 20261002,
client API 2.5) and the owned fixtures in `fixtures/audio/`.

## What it does

- Implements `apollo_runtime::MediaEngine`. One libmpv handle on a dedicated thread; commands over a
  channel; events out through an `EventSink` (never the UI).
- Events: `Opened{duration}`, `Buffering`, `Position` (coalesced to <= 4 Hz, only while time moves),
  `Seeked{gen, position}` (generation echoed exactly), `Eof` (once), `Error(Failure)`.
- Only play URLs the resolver validated: the engine takes a `LeaseId` and looks it up in the
  memory-only `LeaseStore`. A missing/stale lease fails fast with `LeaseExpired` and never opens a
  connection (tested: zero requests reach the server).
- Replaced / closed loads are silent: events are ignored until mpv announces the new file, and
  commands for a non-active load are dropped.
- Hardening (read back from mpv in a test): no user config, no scripts, no extractor (`ytdl=no`),
  no video, no disk cache, no mpv media-key handling (SMTC belongs to the app), no unsafe
  playlists, bounded in-memory demuxer buffer (24 MiB + 8 MiB back), **`tls-verify=yes`**.

## Findings you should know

1. **Failed loads are invisible through libmpv2's `wait_event`.** The wrapper turns an END_FILE
   *error* into `Err(..)` and discards the reason and error code. The engine therefore reads events
   straight from `mpv_wait_event` (sys bindings). Anything built on `wait_event` would hang on a 403.
2. **mpv 0.41 uses libcurl for HTTP here, not ffmpeg's http.** Diagnostics read like
   `HTTP error 403`, `error: Could not connect to server`, `transfer failed: Transferred a partial
   file`. Classification patterns are based on those real messages (see `classify.rs` tests).
3. **Header scoping limitation (RES-03).** libmpv has one header list per load. On a cross-origin
   redirect it follows the redirect and forwards that list. Recorded by
   `cross_origin_redirect_header_behaviour_is_recorded`: `User-Agent` and `Referer` reached the
   second origin; `Cookie`/`Authorization` cannot be present because `UrlPolicy` drops everything
   except `user-agent, accept, accept-language, referer, origin` and the engine filters again.
   So the leak is limited to non-secret headers. mpv cannot validate redirect targets against
   `UrlPolicy` either. If real googlevideo URLs turn out to redirect, a narrow local media broker
   (BUILD_SPEC RES-03) is the fix; not built. Not yet observed on real YouTube URLs (owner-run).
4. **TLS verification works** with the hardened options (examples/tls_probe.rs): valid cert passes
   the handshake; expired and self-signed certs are rejected.
5. **No engine-level preload.** The `MediaEngine` port has `open/pause/resume/seek/close/stop` only,
   so `EngineEvent::Transitioned` is never emitted and the "at most one lookahead" budget is met
   trivially (zero). Gapless needs a port change in `crates/runtime` (M4).
6. **Silent-output guard.** mpv will keep "playing" with no sound if no audio output opens. On the
   first `PlaybackRestart` the engine reads `current-ao`; empty means `Error(Device)`. Mid-play,
   output-device log lines classified as device trouble emit `Error(Device)` once.
7. `LoadId` has a private constructor, so tests mint ids by driving `Coordinator`
   (`tests/support::mint_loads`). A `LoadId::for_tests` would be simpler (domain change, not made).

## Test-harness lesson

An early run of the failure tests was flaky (up to 8 of 10 runs). The cause was the loopback test
server, not the engine: on Windows an accepted socket inherits the listener's non-blocking mode,
so a read that raced the request hung up without answering, which mpv reported as "Failure when
receiving data from the peer". Fixed in `tests/support` (`set_nonblocking(false)`); 0 failures in
15 consecutive runs afterwards.

## Tests (all against real libmpv, `ao=null`, loopback server)

| Test | Covers |
|---|---|
| `aac_and_opus_open_play_seek_and_reach_eof` | TEST-21 (local): AAC/M4A, Opus/Ogg, Opus/WebM decode, ~4 Hz positions, seek with generation, single EOF, silence after |
| `paused_open_and_paused_seek_never_advance` | TEST-10 (engine half) |
| `replaced_loads_stay_silent` | TEST-05/06 (engine half): slow load replaced; 20 rapid opens, one owner |
| `close_and_stop_silence_the_engine` | stale commands ignored |
| `failures_are_classified` + `connection_refused...` | NET-02: 403/404/410 -> LeaseExpired, 429 -> Throttled, 503/refused/truncated -> Network, garbage -> Unsupported |
| `stale_or_missing_leases_fail_fast_without_a_request` | TEST-13 (engine half) |
| `only_allowed_headers_reach_the_server` | RES-03 / TEST-14: canary Cookie/Authorization/X-Custom never sent |
| `cross_origin_redirect_header_behaviour_is_recorded` | limitation 3 |
| `engine_is_hardened_against_user_config_and_disk` | MEDIA-01 |
| `dropping_the_engine_mid_playback_is_prompt` | bounded shutdown (< 3 s) |

## not_run

- Output-device loss / sleep-resume (TEST-20): needs hardware. Only the classifier and the
  `current-ao` guard are unit/integration tested.
- Real googlevideo URLs (redirects, 403 on genuine expiry): owner-run with the resolver.
- Audible output (`ao=wasapi`): M0 verified WASAPI opens; these tests are silent by design.
- Linux/macOS: the crate compiles its platform-independent parts only (`engine` is `cfg(windows)`).
