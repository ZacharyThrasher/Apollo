# Resolver speed: how a song starts quickly

Design record: `docs/adr/0004-warm-resolver-workers.md`.

## Where the time went (per track, cold `yt-dlp`)
1. Start the PyInstaller exe (bootloader + Python child).
2. New YouTube session, TLS handshakes, watch page fetch.
3. Download and parse the player JS, solve challenges in Deno (largest part).
4. Pick the format, print JSON.
Warm workers remove 1-3 for every track after the first and let the preprocessed player be reused;
the URL cache and prefetch remove the wait entirely for tracks resolved ahead of time.

## What the app does now
| Situation | What happens |
|---|---|
| Queue/playlist/album loaded | head + next 2 (play order) resolved in the background |
| Pointer or keyboard cursor rests on a track for 250 ms, or a page opens | nothing, unless Settings > Playback > **Preload songs on hover** is on (it is off by default); then that track (or the page's first song) is resolved speculatively, within the prefetch limits of ADR 0005 |
| Click play | cached URL is used (no resolve), else a warm worker resolves it |
| Next track | the engine's preload makes it near-instant |
| Warm workers unavailable | old process-per-track path, reason shown in diagnostics |

## Measuring on the PC
1. Close Apollo. Rebuild with `scripts\package.ps1` (it now also builds `apollo-resolve-bench.exe`).
2. Run `C:\Apollo\apollo-resolve-bench.exe` (uses Apollo's stored session; Liked Music tracks).
3. It prints a SUMMARY and saves `%LOCALAPPDATA%\Apollo\app\bench-resolve.txt`:
   - [1] legacy per-process (the BEFORE number)
   - [2] warm: first click, then steady state
   - [3] cached URL
   - [4] prefetch then click
   - [5] cached URL to libmpv open and first audio (null output)
4. In the app, every start appends a line to `%LOCALAPPDATA%\Apollo\app\latency.log`:
   `t=<ms> kind=.. resolve_ms=.. open_ms=.. audio_ms=.. total_ms=..` (`resolve_ms=-` means a
   preloaded track was adopted; durations only, no ids or URLs). The resolver adds its own lines to the
   same file (source: warm/cache/cold, worker time, per-phase milliseconds).
   Every request that goes out to YouTube also leaves one line, `yt_request kind=<what> video_id=<id>`
   (`resolve`, `resolve_ahead`, `player`, `next`, `browse`, `search`, `ping`, ...; `-` when the request
   is not about one video). These are the only lines that carry a video id. They are written to the
   log file only, never to the diagnostics report. When YouTube pushes back again, count them per
   minute by kind, and look for the same id coming back.

## Switches
- `APOLLO_RESOLVER=legacy`: always use a process per track (A/B comparison, or if workers misbehave).
- Tunables live in `ResolverConfig::production()` (`WarmConfig`, `CacheConfig`).

## Measured on the owner's PC (main with the warm workers, real account, 2026-10-03)
| Click to playable stream URL | Median |
|---|---|
| Before: one yt-dlp per track | 5.1 s |
| First click after launch (starts a worker) | 4.6 s |
| Cold click, warm worker | 2.5 to 3.7 s |
| URL already held, or prefetched then clicked | 0.00 s |
| Then libmpv: file open / audio moving | 24 ms / about 0.3 s |

Where a cold 3 s goes (worker phases): watch-page download about 1.0 s (this is also where yt-dlp
learns the account is Premium, which makes it add the `web_creator` client), the extra client
lookups about 0.4 s, the music player API 0.7 to 1.1 s, JS solve about 0.15 s.

## Round 2: trimming a cold click
`WarmConfig::youtube_args` hands `--extractor-args youtube:...` to the workers. Skipping the page
and the initial-data request (`player_skip=webpage,initial_data`) removes the ~1 s page step;
the cost is that yt-dlp no longer sees the Premium marker, so it asks `web_embedded`,
`tv_downgraded` and `web_music` instead of `web_creator`, `tv_downgraded`, `web_music`. The bench
section [6] runs each setting on a fresh worker and prints which client served the stream
(`client=` in the worker lines: `c=` of the URL) so the choice is made on data, not on guesses.
Home songs now also resolve ahead when the pointer rests on them.

## Why not call the player API directly from Rust ("what the browser does")?
It would still pay the one player-API round trip (0.7 to 1.1 s here) and save the JS solve
process (about 0.15 s) and any yt-dlp bookkeeping: an estimated 0.3 to 0.4 s over a trimmed
worker. It would also make us own the client quirks yt-dlp tracks (visitor data, signature
timestamp, PO-token rules) and drop its multi-client fallback. A web client turning SABR-only
would break both paths and is not bypassed in either (spec). Revisit only if cold clicks stay
above roughly 1.5 s after the trim; if built, keep the workers as the fallback.

## Round 2 results (bench section [6], owner's PC)
| Setting | Cold click | Stream |
|---|---|---|
| yt-dlp defaults | 3.3 s | WEB_REMIX opus itag 774, about 256 kbps (Premium) |
| skip page + initial data | 1.9 s | WEB_EMBEDDED_PLAYER opus itag 251, about 130 kbps |
| skip page + initial data, embedded and tv clients only | 2.65 s | same as above, slower |
| also skip client configs / only web_music, tv or web_creator / Premium client set without the page | no playable format | refused |

Production now uses the skip setting for clicks only and the defaults for everything resolved
ahead (`WarmConfig::fast_clicks`). A click whose fast resolve fails is retried once with the
defaults. The remaining cold-click time is mostly the web music player API call (0.9 to 1.4 s).

## Round 3 results (bench section [6], owner's PC): the web music call was the cost
With a music.youtube.com URL yt-dlp always adds the web music client, a 0.9 to 1.5 s call, even
when another client's stream is the one used. Resolving the plain watch URL instead removes it.

| Setting (all skip page + initial data) | Cold click | Stream |
|---|---|---|
| yt-dlp defaults (reference) | 4.1 s | Premium opus 774, ~256 kbps |
| music URL | 2.6 s | opus 251, ~130 kbps |
| watch URL | 1.05 s | opus 251, ~130 kbps |
| watch URL, embedded + tv clients | 0.81 s | opus 251, ~130 kbps |
| watch URL, embedded client only | 0.63 s | opus 251, ~130 kbps |

(Superseded by the owner's "always best" choice on 2026-10-03, see below; this profile is
`WarmConfig::fast_clicks()` and stays opt-in.) Production clicks used "watch URL, embedded + tv clients" (tv covers some embedding-restricted
tracks for 0.2 s); a click with no stream is retried with the defaults. Anything resolved ahead
keeps the defaults (Premium stream). The remaining time is one embedded player-API call (about
0.15 s), the tv call and the JS solve (about 0.15 s).

## Swapping to the Premium stream after a fast start (considered, not built)
A reopen in libmpv takes 25 to 70 ms to open and the first audio shows within one 250 ms position
tick (about 0.3 s measured), plus a connect to googlevideo, so a mid-song swap would drop out
audibly for roughly 0.1 to 0.4 s; loadfile/start= cannot crossfade. A seamless swap needs a
second engine with a crossfade (or an EDL timeline) and a coordinator effect so it does not look
like a track change. It would only help the one cold-clicked track per start. Not measured on the
PC; revisit if the ~130 kbps stream bothers the owner.

## Owner decision: always best (2026-10-03)
Every request, clicks included, is resolved with yt-dlp's defaults for the Premium stream
(opus 774, about 256 kbps). Cold click is back to about 3.3 s; prepared tracks stay at 0 s. Bench
section [6] now compares settings that might keep the Premium stream faster (tv client only,
watch URL, no page); the winner, if any, becomes the production default.

## Round 4 results (bench section [6], owner's PC, main fdb7734): no faster way to the Premium stream
Looked for a setting that keeps the Premium stream (opus 774, about 256 kbps) and beats the defaults.

| Setting | Cold click (median) | Stream |
|---|---|---|
| yt-dlp defaults | 3.09 s | 774 on 3 of 4 tracks (first on a fresh worker: 251) |
| page + tv,music clients | 2.81 s | 774 on 3 of 4 tracks (first on a fresh worker: 251) |
| watch URL only | 1.97 s | 251 (about 130 kbps) every time |
| skip + watch URL + tv only | no stream | refused in 0.1 to 0.3 s |
| skip + watch URL + tv,creator | no stream | unsupported-format |
| skip + music URL + tv,music | no stream | unsupported-format |

Nothing keeps the Premium stream and is meaningfully faster (2.81 s vs 3.09 s is within the noise),
so production stays on the defaults. Premium needs the web music client, and that call plus the
watch page is the time.

Open observations:
- The first resolve of a bench run served itag 251 from WEB_CREATOR in every variant that used the
  page, even with the defaults, and later requests got 774. PR #33 assumed a fresh worker was the
  cause and asked again once; that did not help and was reverted (#34). Bench section [7] (tracks in
  reverse order on a fresh worker) settled it: the first resolve, a different track, got 774, and
  track 1, resolved last on a warmed worker, still got 251/133 kbps. It is the track (that video
  only offers the plain stream through yt-dlp), not the worker. Nothing to fix.
- One cold click took 8.6 s with about 6.7 s after the last diagnostic phase. Cause unknown (one sample).
